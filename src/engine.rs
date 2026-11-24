//! The transcription worker: one thread that keeps the loaded model, takes
//! one file at a time, reads its sound in long chunks cut at quiet moments,
//! and streams Whisper's segments back to the window as they are heard.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::decode::{self, DecodeError, Decoder};
use crate::model::Model;
use crate::transcript::Segment;

/// How far one file has got, and whether to stop. Shared with the window.
#[derive(Default, Debug)]
pub struct JobProgress {
    /// Thousandths done.
    permille: AtomicU32,
    /// Whether the length is known, so `permille` means something.
    known: AtomicBool,
    cancel: AtomicBool,
}

impl JobProgress {
    pub fn fraction(&self) -> Option<f32> {
        self.known
            .load(Ordering::Relaxed)
            .then(|| self.permille.load(Ordering::Relaxed) as f32 / 1000.0)
    }

    pub fn set_fraction(&self, fraction: f32) {
        self.known.store(true, Ordering::Relaxed);
        self.permille
            .store((fraction.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub struct Job {
    pub id: u64,
    pub path: PathBuf,
    pub model: Model,
    /// A Whisper language code, or `None` to detect it.
    pub language: Option<String>,
    pub progress: Arc<JobProgress>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    NoAudio,
    Unreadable,
    /// The model file would not load; it is probably damaged.
    Model,
    Other(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::NoAudio => "This file has no sound to transcribe.".to_owned(),
            Failure::Unreadable => "Windows can't read the sound in this file. It may be damaged, or need a video extension from the Microsoft Store (such as HEVC or AV1).".to_owned(),
            Failure::Model => "The speech model wouldn't load, so it was removed. Press Transcribe to download it again.".to_owned(),
            Failure::Other(e) => format!("Transcription stopped with an error: {e}"),
        }
    }
}

impl From<DecodeError> for Failure {
    fn from(e: DecodeError) -> Self {
        match e {
            DecodeError::NoAudio => Failure::NoAudio,
            DecodeError::Unreadable => Failure::Unreadable,
            DecodeError::Other(e) => Failure::Other(e),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// Stopped on request; the text so far is kept.
    Stopped,
    Failed(Failure),
}

pub enum Event {
    Started { id: u64, seconds: Option<f64> },
    Language { id: u64, code: String },
    Segment { id: u64, segment: Segment },
    Finished { id: u64, outcome: Outcome },
}

/// Starts the worker. Jobs go in one end, events come out the other; every
/// event asks the window to redraw.
pub fn spawn(ctx: eframe::egui::Context) -> (Sender<Job>, Receiver<Event>) {
    let (job_tx, job_rx) = channel::<Job>();
    let (event_tx, event_rx) = channel::<Event>();
    std::thread::spawn(move || {
        let mut loaded: Option<(Model, WhisperContext)> = None;
        while let Ok(job) = job_rx.recv() {
            let id = job.id;
            let outcome = run(&job, &mut loaded, &event_tx, &ctx);
            let _ = event_tx.send(Event::Finished { id, outcome });
            ctx.request_repaint();
        }
    });
    (job_tx, event_rx)
}

/// Seconds of sound handed to Whisper at once. Long enough that cuts are
/// rare, short enough to keep memory low on recordings of many hours.
const CHUNK_SECS: usize = 10 * 60;
/// How far back from a chunk's end to look for a quiet place to cut.
const CUT_SEARCH_SECS: usize = 20;

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8) as i32
}

fn load(model: Model) -> Result<WhisperContext, Failure> {
    // Read into memory first: whisper.cpp opens paths with narrow C APIs
    // that fail on user folders with non-ASCII names.
    let bytes = std::fs::read(model.path()).map_err(|_| Failure::Model)?;
    WhisperContext::new_from_buffer_with_params(&bytes, WhisperContextParameters::default())
        .map_err(|_| Failure::Model)
}

unsafe extern "C" fn should_abort(data: *mut c_void) -> bool {
    // SAFETY: `data` points at the job's cancel flag, which outlives the
    // `full` call this is registered for.
    unsafe { (*(data as *const AtomicBool)).load(Ordering::Relaxed) }
}

fn run(
    job: &Job,
    loaded: &mut Option<(Model, WhisperContext)>,
    events: &Sender<Event>,
    ctx: &eframe::egui::Context,
) -> Outcome {
    match run_inner(job, loaded, events, ctx) {
        Ok(()) if job.progress.cancelled() => Outcome::Stopped,
        Ok(()) => Outcome::Done,
        Err(_) if job.progress.cancelled() => Outcome::Stopped,
        Err(f) => Outcome::Failed(f),
    }
}

fn run_inner(
    job: &Job,
    loaded: &mut Option<(Model, WhisperContext)>,
    events: &Sender<Event>,
    ctx: &eframe::egui::Context,
) -> Result<(), Failure> {
    let mut decoder = Decoder::open(&job.path)?;
    let seconds = decoder.seconds();
    let _ = events.send(Event::Started { id: job.id, seconds });
    ctx.request_repaint();
    if job.progress.cancelled() {
        return Ok(());
    }
    if loaded.as_ref().is_none_or(|(m, _)| *m != job.model) {
        *loaded = None;
        *loaded = Some((job.model, load(job.model)?));
    }
    let Some((_, whisper)) = loaded.as_ref() else {
        return Err(Failure::Model);
    };
    let mut state = whisper
        .create_state()
        .map_err(|e| Failure::Other(e.to_string()))?;

    let total_samples = seconds.map(|s| s * decode::RATE as f64);
    let chunk_len = CHUNK_SECS * decode::RATE as usize;
    let mut language = job.language.clone();
    let mut buffer: Vec<f32> = Vec::with_capacity(chunk_len + decode::RATE as usize * 30);
    // Samples already handed to Whisper.
    let mut offset: usize = 0;
    let mut ended = false;
    let mut heard_any = false;
    while !ended || !buffer.is_empty() {
        while !ended && buffer.len() < chunk_len {
            if job.progress.cancelled() {
                return Ok(());
            }
            ended = !decoder.read(&mut buffer)?;
        }
        if buffer.is_empty() {
            break;
        }
        heard_any = true;
        let cut = if ended {
            buffer.len()
        } else {
            quiet_cut(&buffer, CUT_SEARCH_SECS * decode::RATE as usize)
        };
        let chunk = &buffer[..cut];

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_n_threads(threads());
        params.set_language(Some(language.as_deref().unwrap_or("auto")));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_suppress_nst(true);
        let offset_ms = (offset as u64) * 1000 / decode::RATE as u64;
        {
            let events = events.clone();
            let ctx = ctx.clone();
            let id = job.id;
            params.set_segment_callback_safe_lossy(move |data: whisper_rs::SegmentCallbackData| {
                let segment = Segment {
                    start_ms: offset_ms + data.start_timestamp.max(0) as u64 * 10,
                    end_ms: offset_ms + data.end_timestamp.max(0) as u64 * 10,
                    text: data.text,
                };
                let _ = events.send(Event::Segment { id, segment });
                ctx.request_repaint();
            });
        }
        {
            let progress = Arc::clone(&job.progress);
            let (done, len) = (offset as f64, chunk.len() as f64);
            params.set_progress_callback_safe(move |percent: i32| {
                if let Some(total) = total_samples {
                    let at = done + len * percent.clamp(0, 100) as f64 / 100.0;
                    progress.set_fraction((at / total.max(1.0)) as f32);
                }
            });
        }
        // SAFETY: the flag lives in `job.progress`, which outlives `full`.
        unsafe {
            params.set_abort_callback(Some(should_abort));
            params.set_abort_callback_user_data(
                &job.progress.cancel as *const AtomicBool as *mut c_void,
            );
        }
        let result = state.full(params, chunk);
        if job.progress.cancelled() {
            return Ok(());
        }
        result.map_err(|e| Failure::Other(e.to_string()))?;

        if language.is_none() {
            let id = state.full_lang_id_from_state();
            if let Some(code) = whisper_rs::get_lang_str(id) {
                // Keep later chunks in the language heard first.
                language = Some(code.to_owned());
                let _ = events.send(Event::Language { id: job.id, code: code.to_owned() });
            }
        }
        offset += cut;
        buffer.drain(..cut);
        if let Some(total) = total_samples {
            job.progress.set_fraction((offset as f64 / total.max(1.0)) as f32);
        }
    }
    if !heard_any {
        return Err(Failure::NoAudio);
    }
    job.progress.set_fraction(1.0);
    Ok(())
}

/// Where to end a chunk: the middle of the quietest tenth of a second in
/// the last `search` samples, so a cut rarely falls inside a word.
pub fn quiet_cut(samples: &[f32], search: usize) -> usize {
    const FRAME: usize = decode::RATE as usize / 10;
    let len = samples.len();
    if len <= FRAME * 2 {
        return len;
    }
    let from = len.saturating_sub(search.max(FRAME * 2));
    let mut best = (f32::MAX, len);
    let mut start = from;
    while start + FRAME <= len {
        let energy: f32 = samples[start..start + FRAME].iter().map(|s| s * s).sum();
        if energy < best.0 {
            best = (energy, start + FRAME / 2);
        }
        start += FRAME;
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_cut_in_the_quietest_place() {
        let rate = decode::RATE as usize;
        // Ten seconds of "speech" with a silent gap from 7.0 s to 7.3 s.
        let mut samples: Vec<f32> = (0..rate * 10).map(|i| ((i % 50) as f32 / 50.0) - 0.5).collect();
        for s in &mut samples[rate * 7..rate * 7 + rate * 3 / 10] {
            *s = 0.0;
        }
        let cut = quiet_cut(&samples, rate * 5);
        assert!(cut > rate * 7 && cut < rate * 7 + rate * 3 / 10, "{cut}");
    }

    #[test]
    fn very_short_audio_is_not_cut() {
        assert_eq!(quiet_cut(&[0.1; 100], 16_000), 100);
        assert_eq!(quiet_cut(&[], 16_000), 0);
    }

    #[test]
    fn job_progress_is_unknown_until_set_and_clamped() {
        let p = JobProgress::default();
        assert_eq!(p.fraction(), None);
        p.set_fraction(0.42);
        assert_eq!(p.fraction(), Some(0.42));
        p.set_fraction(3.0);
        assert_eq!(p.fraction(), Some(1.0));
        assert!(!p.cancelled());
        p.cancel();
        assert!(p.cancelled());
    }

    #[test]
    fn every_failure_says_what_to_do() {
        for f in [Failure::NoAudio, Failure::Unreadable, Failure::Model, Failure::Other("x".into())] {
            assert!(f.message().ends_with('.') || f.message().ends_with(')') || f.message().ends_with('x'));
        }
        assert_eq!(Failure::from(DecodeError::NoAudio), Failure::NoAudio);
    }
}
