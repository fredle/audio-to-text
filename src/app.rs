//! The window: the files to transcribe and the options on the left, the
//! transcript on the right, and the worker that does the listening. Colours,
//! type and painted pieces come from `theme.rs`; this file only lays out.
//! The footer with the auto-update toggle and version is required on every
//! factory app.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Key, KeyboardShortcut, Layout, Margin, Modifiers, Rect, RichText, Sense,
    Stroke, pos2, vec2,
};

use crate::decode::{self, DecodeError};
use crate::engine::{self, Event, Failure, Job, JobProgress, Outcome};
use crate::license;
use crate::model::{self, DownloadError, DownloadProgress, Model};
use crate::shots::Shots;
use crate::theme::{self, Icon, Palette, Tone};
use crate::transcript::{self, Paragraph, Segment};

const APP_NAME: &str = "Transcribe Audio & Video to Text";
const WIDTH: f32 = 980.0;
const HEIGHT: f32 = 680.0;
const MIN_SIZE: [f32; 2] = [820.0, 560.0];
/// Width of the left column's cards.
const SIDE_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 52.0;
/// Width of the time column beside each paragraph.
const TIME_WIDTH: f32 = 64.0;
/// How many scenes screenshot mode captures (see `show_scene`). Scenes 5
/// and 6 (the empty window, a download and a failed file) exist for checking
/// those states by raising this; the Store gets the first four.
const SHOT_SCENES: usize = 4;
/// How often the window redraws while working.
const BUSY_REPAINT: Duration = Duration::from_millis(250);

/// The app's own icon for the title bar and taskbar, drawn by
/// `assets/make-icon.ps1`. Without it the window shows egui's logo.
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon-64.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

pub fn run(args: Vec<String>) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([WIDTH, HEIGHT])
        .with_min_inner_size(MIN_SIZE)
        .with_drag_and_drop(true)
        .with_icon(window_icon());
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(App::new(&args, &cc.egui_ctx)))),
    )
}

#[derive(Clone, Debug, PartialEq)]
enum ItemState {
    Waiting,
    Running,
    Done,
    /// Stopped on request; the text so far is kept.
    Stopped,
    Failed(String),
}

struct Item {
    /// Stable across removals, unlike the position in the list.
    id: u64,
    path: PathBuf,
    name: String,
    video: bool,
    /// Length, once known.
    seconds: Option<f64>,
    state: ItemState,
    segments: Vec<Segment>,
    paragraphs: Vec<Paragraph>,
    /// Whisper language code heard or chosen.
    language: Option<String>,
    /// Whether the language was detected rather than chosen.
    detected: bool,
    progress: Arc<JobProgress>,
    started: Option<Instant>,
    /// Screenshot mode: the time left to show.
    shot_left: Option<f64>,
}

impl Item {
    fn new(id: u64, path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let video = is_video(&path);
        Self {
            id,
            path,
            name,
            video,
            seconds: None,
            state: ItemState::Waiting,
            segments: Vec::new(),
            paragraphs: Vec::new(),
            language: None,
            detected: false,
            progress: Arc::new(JobProgress::default()),
            started: None,
            shot_left: None,
        }
    }

    fn set_segments(&mut self, segments: Vec<Segment>) {
        self.segments = segments;
        self.paragraphs = transcript::paragraphs(&self.segments);
    }

    fn has_text(&self) -> bool {
        !self.paragraphs.is_empty()
    }

    fn time_left(&self) -> Option<f64> {
        if self.shot_left.is_some() {
            return self.shot_left;
        }
        let started = self.started?;
        transcript::remaining(started.elapsed().as_secs_f64(), self.progress.fraction()?)
    }
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "3gp" | "mpeg"
            )
        })
}

/// "english" -> "English", "haitian creole" -> "Haitian creole".
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Whisper's languages as (code, name), sorted by name.
fn languages() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = (0..=whisper_rs::get_lang_max_id())
        .filter_map(|id| {
            let code = whisper_rs::get_lang_str(id)?;
            let name = whisper_rs::get_lang_str_full(id)?;
            Some((code.to_owned(), capitalize(name)))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// A finished model download, still downloading or failed.
struct DownloadRun {
    model: Model,
    progress: Arc<DownloadProgress>,
    result: Receiver<Result<(), DownloadError>>,
}

/// The last export, for the line under the transcript.
struct Saved {
    path: PathBuf,
}

/// A short message about files that were not added.
struct Notice {
    text: String,
}

pub struct App {
    settings: crate::settings::Settings,
    version: String,
    shots: Option<Shots>,
    items: Vec<Item>,
    next_id: u64,
    selected: Option<u64>,
    jobs: Sender<Job>,
    events: Receiver<Event>,
    probe_tx: Sender<(u64, Result<Option<f64>, DecodeError>)>,
    probe_rx: Receiver<(u64, Result<Option<f64>, DecodeError>)>,
    /// The file being transcribed.
    running: Option<u64>,
    /// Whether to go on to the next waiting file when one finishes.
    run_active: bool,
    download: Option<DownloadRun>,
    download_error: Option<String>,
    /// Start transcribing once the download finishes.
    start_after_download: bool,
    /// Whether each model is on disk and whole: [Small, Large].
    ready: [bool; 2],
    languages: Vec<(String, String)>,
    search: String,
    match_index: usize,
    scroll_to_match: bool,
    saved: Option<Saved>,
    save_error: Option<String>,
    notice: Option<Notice>,
    more_open: bool,
    licensed: bool,
}

impl App {
    fn new(args: &[String], ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        theme::install_fonts(ctx);
        // For checking both looks without changing Windows' setting.
        if let Some(theme) = crate::shots::theme_from_args(args) {
            ctx.set_theme(theme);
        }
        let shots = Shots::from_args(args);
        let (jobs, events) = engine::spawn(ctx.clone());
        let (probe_tx, probe_rx) = channel();
        let settings = crate::settings::load();
        let licensed = license::verify(&settings.license_key).is_some();
        let mut app = Self {
            settings,
            version: crate::update::current_version(),
            shots,
            items: Vec::new(),
            next_id: 0,
            selected: None,
            jobs,
            events,
            probe_tx,
            probe_rx,
            running: None,
            run_active: false,
            download: None,
            download_error: None,
            start_after_download: false,
            ready: [Model::Small.is_ready(), Model::Large.is_ready()],
            languages: languages(),
            search: String::new(),
            match_index: 0,
            scroll_to_match: false,
            saved: None,
            save_error: None,
            notice: None,
            more_open: false,
            licensed,
        };
        if app.shots.is_some() {
            // Screenshots show the app as a new user first sees it.
            ctx.set_pixels_per_point(1.0);
            app.settings = crate::settings::Settings::default();
            app.version = env!("CARGO_PKG_VERSION").to_owned();
            app.licensed = false;
            return app;
        }
        // Files passed on the command line ("Open with").
        let paths: Vec<PathBuf> = args
            .iter()
            .filter(|a| !a.starts_with("--"))
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .collect();
        app.add_paths(paths, ctx);
        app
    }

    /// Saves settings, except in screenshot mode.
    fn save(&self) {
        if self.shots.is_none() {
            crate::settings::save(&self.settings);
        }
    }

    fn model(&self) -> Model {
        let model = Model::from_key(&self.settings.model);
        if model == Model::Large && !license::unlocked(self.licensed) {
            Model::Small
        } else {
            model
        }
    }

    fn model_ready(&self, model: Model) -> bool {
        self.ready[model as usize]
    }

    fn refresh_ready(&mut self) {
        if self.shots.is_none() {
            self.ready = [Model::Small.is_ready(), Model::Large.is_ready()];
        }
    }

    fn language_name(&self, code: &str) -> String {
        self.languages
            .iter()
            .find(|(c, _)| c == code)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| code.to_owned())
    }

    fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    fn busy(&self) -> bool {
        self.running.is_some() || self.download.is_some()
    }

    fn waiting(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.state == ItemState::Waiting)
            .count()
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let (media, skipped): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| transcript::is_media(p));
        self.notice = match skipped.as_slice() {
            [] => None,
            [one] => Some(Notice {
                text: format!(
                    "Skipped {}: not an audio or video file.",
                    one.file_name().unwrap_or_default().to_string_lossy()
                ),
            }),
            many => Some(Notice {
                text: format!("Skipped {} files that aren't audio or video.", many.len()),
            }),
        };
        let mut media: Vec<PathBuf> = media
            .into_iter()
            .filter(|p| !self.items.iter().any(|i| &i.path == p))
            .collect();
        if media.is_empty() {
            return;
        }
        if !license::unlocked(self.licensed) {
            // One file at a time without the unlock: the new file replaces
            // whatever is not being transcribed.
            media.truncate(1);
            let running = self.running;
            self.items.retain(|i| Some(i.id) == running);
        }
        let mut added = Vec::new();
        for path in media {
            let id = self.next_id;
            self.next_id += 1;
            added.push((id, path.clone()));
            self.items.push(Item::new(id, path));
        }
        if let Some((first, _)) = added.first()
            && self.running.is_none()
        {
            self.selected = Some(*first);
        }
        let tx = self.probe_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (id, path) in added {
                let _ = tx.send((id, decode::probe(&path)));
                ctx.request_repaint();
            }
        });
    }

    fn choose_files(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Choose recordings or videos")
            .add_filter("Audio and video", transcript::media_extensions())
            .pick_files();
        if let Some(paths) = picked {
            self.add_paths(paths, ctx);
        }
    }

    fn remove(&mut self, id: u64) {
        if self.running == Some(id) {
            return;
        }
        self.items.retain(|i| i.id != id);
        if self.selected == Some(id) {
            self.selected = self.items.first().map(|i| i.id);
            self.saved = None;
        }
    }

    fn retry(&mut self, id: u64) {
        if let Some(item) = self.item_mut(id) {
            item.state = ItemState::Waiting;
            item.set_segments(Vec::new());
            item.language = None;
            item.progress = Arc::new(JobProgress::default());
            item.started = None;
        }
    }

    /// The primary action: download the model if needed, then transcribe
    /// every waiting file in turn.
    fn start(&mut self, ctx: &egui::Context) {
        if self.busy() || self.waiting() == 0 || self.shots.is_some() {
            return;
        }
        let model = self.model();
        if !self.model_ready(model) {
            self.start_after_download = true;
            self.begin_download(model, ctx);
            return;
        }
        self.run_active = true;
        self.start_next();
    }

    fn start_next(&mut self) {
        let Some(item) = self.items.iter_mut().find(|i| i.state == ItemState::Waiting) else {
            self.run_active = false;
            return;
        };
        let language = (self.settings.language != "auto").then(|| self.settings.language.clone());
        let model = Model::from_key(&self.settings.model);
        let model = if license::unlocked(self.licensed) { model } else { Model::Small };
        item.state = ItemState::Running;
        item.started = Some(Instant::now());
        item.progress = Arc::new(JobProgress::default());
        item.set_segments(Vec::new());
        item.detected = language.is_none();
        item.language = language.clone();
        let job = Job {
            id: item.id,
            path: item.path.clone(),
            model,
            language,
            progress: Arc::clone(&item.progress),
        };
        let id = item.id;
        self.running = Some(id);
        self.selected = Some(id);
        self.saved = None;
        let _ = self.jobs.send(job);
    }

    fn stop(&mut self) {
        self.run_active = false;
        self.start_after_download = false;
        if let Some(id) = self.running
            && let Some(item) = self.item(id)
        {
            item.progress.cancel();
        }
        if let Some(download) = &self.download {
            download.progress.cancel();
        }
    }

    fn begin_download(&mut self, model: Model, ctx: &egui::Context) {
        let progress = Arc::new(DownloadProgress::default());
        progress.done.store(model.partial_bytes(), std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = channel();
        let worker_progress = Arc::clone(&progress);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = model::download(model, &worker_progress);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.download_error = None;
        self.download = Some(DownloadRun {
            model,
            progress,
            result: rx,
        });
    }

    /// Takes in what the workers have sent since the last frame.
    fn poll(&mut self) {
        while let Ok((id, probe)) = self.probe_rx.try_recv() {
            if let Some(item) = self.item_mut(id) {
                match probe {
                    Ok(seconds) => item.seconds = seconds,
                    Err(e) if item.state == ItemState::Waiting => {
                        item.state = ItemState::Failed(Failure::from(e).message());
                    }
                    Err(_) => {}
                }
            }
        }
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Started { id, seconds } => {
                    if let Some(item) = self.item_mut(id)
                        && seconds.is_some()
                    {
                        item.seconds = seconds;
                    }
                }
                Event::Language { id, code } => {
                    if let Some(item) = self.item_mut(id) {
                        item.language = Some(code);
                    }
                }
                Event::Segment { id, segment } => {
                    if let Some(item) = self.item_mut(id) {
                        let mut segments = std::mem::take(&mut item.segments);
                        segments.push(segment);
                        item.set_segments(segments);
                    }
                }
                Event::Finished { id, outcome } => {
                    if outcome == Outcome::Failed(Failure::Model) {
                        Model::from_key(&self.settings.model).remove();
                        self.refresh_ready();
                        self.run_active = false;
                    }
                    if let Some(item) = self.item_mut(id) {
                        item.state = match outcome {
                            Outcome::Done => ItemState::Done,
                            Outcome::Stopped => ItemState::Stopped,
                            Outcome::Failed(f) => ItemState::Failed(f.message()),
                        };
                    }
                    if self.running == Some(id) {
                        self.running = None;
                    }
                    if self.run_active {
                        self.start_next();
                    }
                }
            }
        }
        if let Some(download) = &self.download
            && let Ok(result) = download.result.try_recv()
        {
            let model = download.model;
            self.download = None;
            self.refresh_ready();
            match result {
                Ok(()) if self.model_ready(model) => {
                    if std::mem::take(&mut self.start_after_download) {
                        self.run_active = true;
                        self.start_next();
                    }
                }
                Ok(()) => self.download_error = Some(DownloadError::Damaged.message()),
                Err(e) => {
                    self.start_after_download = false;
                    self.download_error = Some(e.message());
                }
            }
        }
    }

    /// The selected item's matches as (paragraph, start, end).
    fn matches(&self) -> Vec<(usize, usize, usize)> {
        let Some(item) = self.selected.and_then(|id| self.item(id)) else {
            return Vec::new();
        };
        if self.search.trim().is_empty() {
            return Vec::new();
        }
        item.paragraphs
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                transcript::find(&p.text, &self.search)
                    .into_iter()
                    .map(move |(s, e)| (i, s, e))
            })
            .collect()
    }

    fn copy_all(&self, ctx: &egui::Context) {
        if let Some(item) = self.selected.and_then(|id| self.item(id))
            && item.has_text()
        {
            ctx.copy_text(transcript::plain_text(&item.paragraphs));
        }
    }

    /// Writes the transcript next to the recording (or in Documents when that
    /// folder can't be written), never over an existing file.
    fn export(&mut self, ext: &str) {
        let Some(item) = self.selected.and_then(|id| self.item(id)) else {
            return;
        };
        if !item.has_text() {
            return;
        }
        let contents = match ext {
            "srt" => transcript::to_srt(&item.segments),
            "vtt" => transcript::to_vtt(&item.segments),
            _ => transcript::to_txt(&item.paragraphs),
        };
        let stem = item
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Transcript".to_owned());
        let documents = std::env::var_os("USERPROFILE")
            .map(|h| PathBuf::from(h).join("Documents"))
            .unwrap_or_else(|| PathBuf::from("."));
        let dirs = [item.path.parent().map(Path::to_path_buf), Some(documents)];
        let mut last_error = None;
        for dir in dirs.into_iter().flatten() {
            let path = transcript::unique_path(&dir, &stem, ext, Path::exists);
            // create_new: even a file that appears in the meantime is safe.
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, contents.as_bytes()));
            match written {
                Ok(()) => {
                    self.saved = Some(Saved { path });
                    self.save_error = None;
                    return;
                }
                Err(e) => last_error = Some(e),
            }
        }
        self.saved = None;
        self.save_error = Some(format!(
            "Couldn't save the file: {}.",
            last_error.map(|e| e.to_string()).unwrap_or_default()
        ));
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let command = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let shift_command = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        if ctx.input_mut(|i| i.consume_shortcut(&command(Key::O))) && self.shots.is_none() {
            self.choose_files(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&command(Key::Enter))) {
            self.start(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&command(Key::F))) {
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&shift_command(Key::C))) {
            self.copy_all(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&command(Key::S))) {
            self.export("txt");
        }
    }

    /// Puts the app into made-up state number `scene` (0-based) for a Store
    /// screenshot. Fill the window with believable content; start nothing
    /// real.
    fn show_scene(&mut self, scene: usize) {
        self.items.clear();
        self.running = None;
        self.search.clear();
        self.saved = None;
        self.notice = None;
        self.download = None;
        self.download_error = None;
        self.ready = [true, true];
        self.settings.model = "small".to_owned();
        self.settings.language = "auto".to_owned();
        let docs = PathBuf::from("C:\\Users\\Sam\\Documents\\Recordings");
        let add = |app: &mut App, name: &str, minutes: f64, state: ItemState| -> u64 {
            let id = app.next_id;
            app.next_id += 1;
            let mut item = Item::new(id, docs.join(name));
            item.seconds = Some(minutes * 60.0);
            item.state = state;
            app.items.push(item);
            id
        };
        match scene {
            // Finished: a meeting as readable paragraphs.
            0 => {
                let id = add(self, "Team meeting 18 Nov.m4a", 58.0, ItemState::Done);
                add(self, "Client call - Hartley & Co.mp3", 34.0, ItemState::Waiting);
                self.fill(id, MEETING, "en", true);
                self.selected = Some(id);
            }
            // Mid-task: a long lecture with text arriving.
            1 => {
                add(self, "Interview with Dr Okafor.wav", 47.0, ItemState::Done);
                let id = add(self, "History lecture, week 7.mp4", 92.0, ItemState::Running);
                add(self, "Seminar Q&A.mp4", 26.0, ItemState::Waiting);
                self.fill(id, LECTURE, "en", true);
                if let Some(item) = self.item_mut(id) {
                    item.progress.set_fraction(0.41);
                    item.shot_left = Some(19.0 * 60.0);
                }
                self.running = Some(id);
                self.selected = Some(id);
            }
            // Search and subtitles.
            2 => {
                let id = add(self, "Interview with Dr Okafor.wav", 47.0, ItemState::Done);
                self.fill(id, INTERVIEW, "en", true);
                self.selected = Some(id);
                self.search = "budget".to_owned();
                self.saved = Some(Saved {
                    path: docs.join("Interview with Dr Okafor.srt"),
                });
            }
            // Not captured for the Store: the empty window, for checking.
            4 => {
                self.ready = [false, false];
                self.selected = None;
            }
            // Not captured for the Store: a download and a failed file.
            5 => {
                self.ready = [false, false];
                let id = add(self, "Screen recording 0921.mp4", 12.0, ItemState::Failed(Failure::NoAudio.message()));
                add(self, "Voice memo.m4a", 3.0, ItemState::Waiting);
                self.selected = Some(id);
                let progress = Arc::new(DownloadProgress::default());
                progress.total.store(Model::Small.approx_bytes(), std::sync::atomic::Ordering::Relaxed);
                progress.done.store(64 * model::MIB, std::sync::atomic::Ordering::Relaxed);
                let (_tx, rx) = channel();
                self.download = Some(DownloadRun { model: Model::Small, progress, result: rx });
                self.notice = Some(Notice { text: "Skipped notes.docx: not an audio or video file.".to_owned() });
            }
            // A queue in other languages with the larger model.
            3 => {
                self.settings.model = "large".to_owned();
                let id = add(self, "Podcast Folge 112.mp3", 71.0, ItemState::Done);
                add(self, "Entrevista radio.m4a", 38.0, ItemState::Done);
                add(self, "Vorlesung Statistik 3.mkv", 84.0, ItemState::Waiting);
                add(self, "Voice note 0412.m4a", 4.0, ItemState::Waiting);
                add(self, "Webinar recording.webm", 55.0, ItemState::Waiting);
                self.fill(id, PODCAST, "de", true);
                if let Some(item) = self.items.get_mut(1) {
                    item.language = Some("es".to_owned());
                    item.detected = true;
                }
                self.selected = Some(id);
            }
            _ => {}
        }
    }

    /// Fills an item with made-up segments. A line starting with `|` is a new
    /// speaker, so the line before it ends with a longer pause.
    fn fill(&mut self, id: u64, lines: &[(u64, &str)], language: &str, detected: bool) {
        let segments = lines
            .iter()
            .enumerate()
            .map(|(i, (start, text))| {
                let (end, pause) = match lines.get(i + 1) {
                    Some((next, t)) if t.starts_with('|') => (*next, 1_600),
                    Some((next, _)) => (*next, 400),
                    None => (start + 4, 400),
                };
                Segment::new(start * 1000, end * 1000 - pause, &format!(" {}", text.trim_start_matches(['|', ' '])))
            })
            .collect();
        if let Some(item) = self.item_mut(id) {
            item.set_segments(segments);
            item.language = Some(language.to_owned());
            item.detected = detected;
        }
    }

    // ------------------------------------------------------------ layout

    fn side_ui(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        egui::Panel::bottom("actions")
            .frame(egui::Frame::new().inner_margin(Margin { top: 12, ..Margin::ZERO }))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| {
                self.options_card(ui, p);
                ui.add_space(12.0);
                self.action_area(ui, p);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| self.files_card(ui, p));
    }

    fn files_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let size = ui.available_size();
        let mut frame = theme::card(p);
        if dragging {
            frame = frame.stroke(Stroke::new(1.5, p.accent));
        }
        frame.show(ui, |ui| {
            ui.set_min_size(size - vec2(34.0, 26.0));
            ui.set_max_width(size.x - 34.0);
            ui.horizontal(|ui| {
                theme::section_label(ui, "Files", p);
                if !self.items.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::link(ui, "Add files", p)
                            .on_hover_text("Ctrl+O")
                            .clicked()
                            && self.shots.is_none()
                        {
                            self.choose_files(&ctx);
                        }
                    });
                }
            });
            ui.add_space(4.0);
            if self.items.is_empty() {
                self.drop_zone(ui, p, dragging);
            } else {
                self.file_list(ui, p);
            }
        });
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, p: Palette, dragging: bool) {
        let height = (ui.available_height() - 4.0).max(120.0);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let hot = dragging || response.hovered();
        if hot {
            ui.painter().rect_filled(rect, theme::CARD_RADIUS, p.accent_soft);
        }
        theme::dashed_outline(ui.painter(), rect, if hot { p.accent } else { p.border });
        let c = rect.center();
        let disc = Rect::from_center_size(c - vec2(0.0, 34.0), vec2(44.0, 44.0));
        ui.painter().rect_filled(disc, 22, if hot { p.surface } else { p.accent_soft });
        theme::paint_icon(ui.painter(), disc.center(), Icon::Plus, p.accent_text);
        let title = if dragging { "Drop to add" } else { "Drop recordings or videos here" };
        ui.painter().text(c + vec2(0.0, 6.0), Align2::CENTER_CENTER, title, theme::strong(), p.text);
        ui.painter().text(
            c + vec2(0.0, 28.0),
            Align2::CENTER_CENTER,
            "or click to choose files",
            theme::small(),
            p.accent_text,
        );
        ui.painter().text(
            c + vec2(0.0, 52.0),
            Align2::CENTER_CENTER,
            "MP3, M4A, WAV, MP4, MOV, MKV and more",
            theme::small(),
            p.muted,
        );
        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && self.shots.is_none() {
            let ctx = ui.ctx().clone();
            self.choose_files(&ctx);
        }
    }

    fn file_list(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut select = None;
        let mut remove = None;
        let mut retry = None;
        let time = ui.input(|i| i.time);
        let notice_height = if self.notice.is_some() { 40.0 } else { 0.0 };
        egui::ScrollArea::vertical()
            .id_salt("files")
            .auto_shrink([false, true])
            .max_height(ui.available_height() - notice_height)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for item in &self.items {
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                    let selected = self.selected == Some(item.id);
                    let painter = ui.painter();
                    if selected {
                        painter.rect_filled(rect, theme::CONTROL_RADIUS, p.accent_soft);
                    } else if response.hovered() {
                        painter.rect_filled(rect, theme::CONTROL_RADIUS, p.hover);
                    }
                    let (tone, status) = row_status(item);
                    let tile = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 16.0), vec2(32.0, 32.0));
                    theme::file_tile(painter, tile, item.video, tone, p);
                    let text_left = tile.right() + 10.0;
                    let actions = if response.hovered() && item.state != ItemState::Running { 60.0 } else { 8.0 };
                    let text_width = rect.right() - actions - text_left;
                    let name = theme::truncated(painter, &item.name, theme::body(), p.text, text_width);
                    painter.galley(pos2(text_left, rect.top() + 8.0), name, p.text);
                    let status_color = match tone {
                        Tone::Error => p.error,
                        Tone::Warn => p.warn,
                        Tone::Accent => p.accent_text,
                        _ => p.muted,
                    };
                    let mut status_left = text_left;
                    if item.state == ItemState::Running {
                        // A short bar in front of the percentage.
                        let track = Rect::from_min_size(pos2(text_left, rect.top() + 35.0), vec2(72.0, 4.0));
                        theme::progress_track(painter, track, item.progress.fraction(), time, p);
                        status_left += 80.0;
                    }
                    let status = theme::truncated(painter, &status, theme::small(), status_color, text_width - (status_left - text_left));
                    painter.galley(pos2(status_left, rect.top() + 29.0), status, status_color);
                    if response.hovered() && item.state != ItemState::Running && !self.busy_with(item.id) {
                        let mut x = rect.right() - 20.0;
                        let close = Rect::from_center_size(pos2(x, rect.center().y), vec2(28.0, 28.0));
                        if ui
                            .put(close, |ui: &mut egui::Ui| theme::icon_button(ui, Icon::Close, p))
                            .on_hover_text("Remove from the list")
                            .clicked()
                        {
                            remove = Some(item.id);
                        }
                        x -= 28.0;
                        if item.state != ItemState::Waiting {
                            let again = Rect::from_center_size(pos2(x, rect.center().y), vec2(28.0, 28.0));
                            if ui
                                .put(again, |ui: &mut egui::Ui| theme::icon_button(ui, Icon::Retry, p))
                                .on_hover_text("Transcribe again")
                                .clicked()
                            {
                                retry = Some(item.id);
                            }
                        }
                    }
                    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        select = Some(item.id);
                    }
                }
            });
        if let Some(notice) = &self.notice {
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                theme::paint_icon(ui.painter(), rect.center(), Icon::Alert, p.warn);
                let width = ui.available_width();
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    ui.add(egui::Label::new(RichText::new(&notice.text).font(theme::small()).color(p.muted)).wrap());
                });
            });
        }
        if let Some(id) = remove {
            self.remove(id);
        } else if let Some(id) = retry {
            self.retry(id);
            self.selected = Some(id);
        } else if let Some(id) = select {
            if self.selected != Some(id) {
                self.saved = None;
                self.save_error = None;
                self.match_index = 0;
            }
            self.selected = Some(id);
        }
    }

    /// Whether `id` is tied up in a run (it will be picked up next).
    fn busy_with(&self, id: u64) -> bool {
        self.running == Some(id)
    }

    fn options_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let busy = self.busy();
        theme::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_label(ui, "Language", p);
            let current = if self.settings.language == "auto" {
                "Detect automatically".to_owned()
            } else {
                self.language_name(&self.settings.language)
            };
            let mut chosen = None;
            ui.add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_id_salt("language")
                    .width(ui.available_width())
                    .height(320.0)
                    .selected_text(RichText::new(current).color(p.text))
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.settings.language == "auto", "Detect automatically")
                            .clicked()
                        {
                            chosen = Some("auto".to_owned());
                        }
                        for (code, name) in &self.languages {
                            if ui
                                .selectable_label(&self.settings.language == code, name)
                                .clicked()
                            {
                                chosen = Some(code.clone());
                            }
                        }
                    });
            });
            if let Some(code) = chosen {
                self.settings.language = code;
                self.save();
            }

            ui.add_space(6.0);
            theme::section_label(ui, "Speech model", p);
            let unlocked = license::unlocked(self.licensed);
            let labels: Vec<String> = Model::ALL.iter().map(|m| m.label().to_owned()).collect();
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let model = self.model();
            let index = Model::ALL.iter().position(|m| *m == model).unwrap_or(0);
            let picked = ui
                .add_enabled_ui(!busy && unlocked, |ui| {
                    theme::segmented(ui, egui::Id::new("model"), &label_refs, index, p)
                })
                .inner;
            if let Some(i) = picked {
                self.settings.model = Model::ALL[i].key().to_owned();
                self.save();
            }
            let size = transcript::format_mb(model.approx_bytes());
            let about = match model {
                Model::Small => "Faster",
                Model::Large => "Slower, more accurate",
            };
            let state = if self.model_ready(model) {
                format!("{about}  ·  {size}, downloaded")
            } else {
                format!("{about}  ·  Downloads once, {size}")
            };
            ui.label(RichText::new(state).font(theme::small()).color(p.muted));

            if theme::disclosure(ui, self.more_open, "More options", p) {
                self.more_open = !self.more_open;
            }
            if self.more_open {
                let on_disk: u64 = Model::ALL
                    .iter()
                    .filter(|m| self.model_ready(**m))
                    .map(|m| std::fs::metadata(m.path()).map(|d| d.len()).unwrap_or(0))
                    .sum();
                ui.horizontal(|ui| {
                    if theme::link(ui, "Open models folder", p).clicked() {
                        let dir = model::models_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                    }
                    if on_disk > 0 && !busy {
                        ui.add_space(8.0);
                        let label = format!("Delete models ({})", transcript::format_mb(on_disk));
                        if theme::link(ui, &label, p)
                            .on_hover_text("They download again the next time you transcribe.")
                            .clicked()
                        {
                            for m in Model::ALL {
                                m.remove();
                            }
                            self.refresh_ready();
                        }
                    }
                });
            }
        });
    }

    /// The primary button, or what is happening instead of it.
    fn action_area(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        let width = ui.available_width();
        let time = ui.input(|i| i.time);
        if let Some(download) = &self.download {
            let done = download.progress.done.load(std::sync::atomic::Ordering::Relaxed);
            let total = download.progress.total.load(std::sync::atomic::Ordering::Relaxed);
            let total = if total > 0 { total } else { download.model.approx_bytes() };
            let title = format!("Downloading the {} model", download.model.label());
            let detail = format!(
                "{} of {}",
                transcript::format_mb(done).trim_end_matches(" MB"),
                transcript::format_mb(total)
            );
            let fraction = download.progress.fraction();
            if self.status_panel(ui, p, &title, &detail, fraction, time, "Stop download") {
                self.stop();
            }
            return;
        }
        if let Some(id) = self.running
            && let Some(item) = self.item(id)
        {
            let position = self.items.iter().filter(|i| i.state != ItemState::Waiting).count();
            let total = position + self.waiting();
            let title = if total > 1 {
                format!("Transcribing file {position} of {total}")
            } else {
                "Transcribing".to_owned()
            };
            let detail = match (item.time_left(), item.progress.fraction()) {
                (Some(left), _) => transcript::format_remaining(left),
                (None, Some(f)) if f > 0.0 => "Working out the time left".to_owned(),
                _ => "Loading the model and listening".to_owned(),
            };
            let fraction = item.progress.fraction();
            if self.status_panel(ui, p, &title, &detail, fraction, time, "Stop") {
                self.stop();
            }
            return;
        }
        if let Some(error) = self.download_error.clone() {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                theme::paint_icon(ui.painter(), rect.center(), Icon::Alert, p.error);
                ui.label(RichText::new(error).font(theme::small()).color(p.error));
            });
            ui.add_space(8.0);
        }
        let waiting = self.waiting();
        let model = self.model();
        let label = if self.download_error.is_some() {
            "Try again".to_owned()
        } else if !self.model_ready(model) && waiting > 0 {
            "Download model and transcribe".to_owned()
        } else if waiting > 1 {
            format!("Transcribe {waiting} files")
        } else {
            "Transcribe".to_owned()
        };
        let enabled = waiting > 0;
        let response = theme::primary(ui, &label, enabled, width, p);
        if response.on_hover_text("Ctrl+Enter").clicked() {
            self.start(&ctx);
        }
    }

    /// Progress in the place of the primary button. Returns true when its
    /// stop button is pressed.
    #[allow(clippy::too_many_arguments)]
    fn status_panel(
        &self,
        ui: &mut egui::Ui,
        p: Palette,
        title: &str,
        detail: &str,
        fraction: Option<f32>,
        time: f64,
        stop: &str,
    ) -> bool {
        let mut stopped = false;
        theme::card(p).stroke(Stroke::new(1.0, p.accent)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).font(theme::strong()).color(p.text));
                if let Some(f) = fraction {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}%", (f * 100.0).floor() as u32))
                                .font(theme::small())
                                .color(p.accent_text),
                        );
                    });
                }
            });
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
            theme::progress_track(ui.painter(), rect, fraction, time, p);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(detail).font(theme::small()).color(p.muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_text_button(ui, Icon::Stop, stop, p)
                        .on_hover_text("The text so far is kept.")
                        .clicked()
                    {
                        stopped = true;
                    }
                });
            });
        });
        stopped
    }

    fn transcript_ui(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        let size = ui.available_size();
        theme::card(p)
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_min_size(size - vec2(42.0, 34.0));
                ui.set_max_size(size - vec2(42.0, 34.0));
                let Some(id) = self.selected.filter(|id| self.item(*id).is_some()) else {
                    self.empty_state(
                        ui,
                        p,
                        "Your transcript appears here",
                        "Add a recording or video on the left, then press Transcribe. It all happens on this PC, with no time limit.",
                    );
                    return;
                };
                self.transcript_header(ui, p, id);
                ui.add_space(10.0);
                theme::rule(ui, p);
                ui.add_space(4.0);
                let Some(item) = self.item(id) else { return };
                let has_text = item.has_text();
                match (&item.state, has_text) {
                    (ItemState::Failed(message), false) => {
                        let message = message.clone();
                        self.failed_state(ui, p, &message);
                    }
                    (ItemState::Waiting, _) => self.empty_state(
                        ui,
                        p,
                        "Ready to transcribe",
                        "Press Transcribe to start. The text appears here as it is heard, and you can stop at any point and keep it.",
                    ),
                    (ItemState::Running, false) => self.empty_state(
                        ui,
                        p,
                        "Listening",
                        "The first lines appear here once the first stretch of speech has been heard.",
                    ),
                    (ItemState::Done | ItemState::Stopped, false) => self.empty_state(
                        ui,
                        p,
                        "No speech found",
                        "Nothing in this file sounded like speech. If it does have speech, try choosing its language.",
                    ),
                    _ => {
                        let bar_height = 46.0;
                        let body_height = ui.available_height() - bar_height;
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), body_height),
                            Layout::top_down(Align::Min),
                            |ui| self.paragraphs_ui(ui, p, id),
                        );
                        theme::rule(ui, p);
                        ui.add_space(12.0);
                        self.export_bar(ui, p);
                    }
                }
            });
    }

    fn transcript_header(&mut self, ui: &mut egui::Ui, p: Palette, id: u64) {
        let Some(item) = self.item(id) else { return };
        let (tone, pill) = match &item.state {
            ItemState::Waiting => (Tone::Neutral, "Waiting"),
            ItemState::Running => (Tone::Accent, "Transcribing"),
            ItemState::Done => (Tone::Success, "Done"),
            ItemState::Stopped => (Tone::Warn, "Stopped early"),
            ItemState::Failed(_) => (Tone::Error, "Failed"),
        };
        let mut details = Vec::new();
        if let Some(seconds) = item.seconds {
            details.push(transcript::format_length(seconds));
        }
        if let Some(code) = &item.language {
            let name = self.language_name(code);
            details.push(if item.detected { format!("{name}, detected") } else { name });
        }
        if item.has_text() {
            let words: usize = item.paragraphs.iter().map(|p| p.text.split_whitespace().count()).sum();
            details.push(format!("{} words", group_thousands(words)));
        }
        let name = item.name.clone();
        let has_text = item.has_text();
        let matches = self.matches().len();
        ui.horizontal(|ui| {
            let search_width = if has_text { 220.0 } else { 0.0 };
            let left_width = ui.available_width() - search_width - 12.0;
            ui.allocate_ui_with_layout(vec2(left_width, 44.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left_width);
                ui.horizontal(|ui| {
                    let pill_width = 130.0;
                    let galley = theme::truncated(
                        ui.painter(),
                        &name,
                        theme::card_title(),
                        p.text,
                        ui.available_width() - pill_width,
                    );
                    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                    ui.painter().galley(rect.min, galley, p.text);
                    ui.add_space(4.0);
                    theme::tone_pill(ui, pill, tone, p);
                });
                ui.label(RichText::new(details.join("  ·  ")).font(theme::small()).color(p.muted));
            });
            if has_text {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    ui.vertical(|ui| {
                        let response = theme::search_field(ui, search_id(), &mut self.search, search_width, p);
                        if response.changed() {
                            self.match_index = 0;
                            self.scroll_to_match = true;
                        }
                        if response.has_focus() || response.lost_focus() {
                            let (enter, shift, escape) = ui.input(|i| {
                                (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape))
                            });
                            if enter && matches > 0 {
                                self.match_index = if shift {
                                    (self.match_index + matches - 1) % matches
                                } else {
                                    (self.match_index + 1) % matches
                                };
                                self.scroll_to_match = true;
                                response.request_focus();
                            }
                            if escape {
                                self.search.clear();
                            }
                        }
                        if !self.search.trim().is_empty() {
                            let text = if matches == 0 {
                                "No matches".to_owned()
                            } else {
                                format!("{} of {}  ·  Enter for next", self.match_index.min(matches - 1) + 1, matches)
                            };
                            ui.label(RichText::new(text).font(theme::small()).color(p.muted));
                        }
                    });
                });
            }
        });
    }

    fn paragraphs_ui(&mut self, ui: &mut egui::Ui, p: Palette, id: u64) {
        let matches = self.matches();
        let current = (!matches.is_empty()).then(|| matches[self.match_index.min(matches.len() - 1)]);
        let scroll = std::mem::take(&mut self.scroll_to_match);
        let Some(item) = self.item(id) else { return };
        let running = item.state == ItemState::Running;
        egui::ScrollArea::vertical()
            .id_salt(("transcript", id))
            .auto_shrink([false, false])
            .stick_to_bottom(running && matches.is_empty())
            .show(ui, |ui| {
                ui.add_space(8.0);
                for (index, paragraph) in item.paragraphs.iter().enumerate() {
                    let ranges: Vec<(usize, usize)> = matches
                        .iter()
                        .filter(|(i, _, _)| *i == index)
                        .map(|(_, s, e)| (*s, *e))
                        .collect();
                    let current_range = current
                        .filter(|(i, _, _)| *i == index)
                        .map(|(_, s, e)| (s, e));
                    let text_width = ui.available_width() - TIME_WIDTH - ui.spacing().item_spacing.x;
                    let job = paragraph_job(&paragraph.text, &ranges, current_range, text_width, p);
                    let response = ui
                        .horizontal_top(|ui| {
                            let (time_rect, _) = ui.allocate_exact_size(vec2(TIME_WIDTH, theme::READING_LINE), Sense::hover());
                            ui.painter().text(
                                pos2(time_rect.left(), time_rect.center().y),
                                Align2::LEFT_CENTER,
                                transcript::clock(paragraph.start_ms),
                                theme::small(),
                                p.accent_text,
                            );
                            ui.allocate_ui_with_layout(vec2(text_width, 0.0), Layout::top_down(Align::Min), |ui| {
                                ui.set_width(text_width);
                                ui.add(egui::Label::new(job).wrap().selectable(true))
                            })
                        })
                        .response;
                    if scroll && current_range.is_some() {
                        ui.scroll_to_rect(response.rect, Some(Align::Center));
                    }
                    ui.add_space(10.0);
                }
                if running {
                    let time = ui.input(|i| i.time);
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
                    for i in 0..3 {
                        let phase = ((time * 2.0 - i as f64 * 0.25).sin() * 0.5 + 0.5) as f32;
                        ui.painter().circle_filled(
                            pos2(rect.left() + TIME_WIDTH + 4.0 + i as f32 * 12.0, rect.center().y),
                            3.0,
                            p.accent.gamma_multiply(0.3 + 0.7 * phase),
                        );
                    }
                }
            });
    }

    fn export_bar(&mut self, ui: &mut egui::Ui, p: Palette) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if theme::icon_text_button(ui, Icon::Copy, "Copy all", p)
                .on_hover_text("Copies the text without times (Ctrl+Shift+C)")
                .clicked()
            {
                self.copy_all(&ctx);
            }
            ui.add_space(12.0);
            ui.label(RichText::new("Save as").font(theme::small()).color(p.muted));
            for (label, ext, tip) in [
                ("Text", "txt", "Paragraphs with times (Ctrl+S)"),
                ("SRT", "srt", "Subtitles for video players and editors"),
                ("VTT", "vtt", "Subtitles for web video"),
            ] {
                if ui
                    .add(theme::secondary_button(p, label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.export(ext);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(saved) = &self.saved {
                    let path = saved.path.clone();
                    if theme::link(ui, "Show in folder", p).clicked() {
                        show_in_folder(&path);
                    }
                    let kind = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_uppercase())
                        .unwrap_or_default();
                    let galley = theme::truncated(
                        ui.painter(),
                        &format!("Saved as {kind}"),
                        theme::small(),
                        p.success,
                        (ui.available_width() - 24.0).max(40.0),
                    );
                    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                    ui.painter().galley(rect.min, galley, p.success);
                    ui.interact(rect, egui::Id::new("saved-path"), Sense::hover())
                        .on_hover_text(path.display().to_string());
                    let (icon, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    theme::paint_icon(ui.painter(), icon.center(), Icon::Check, p.success);
                } else if let Some(error) = &self.save_error {
                    ui.label(RichText::new(error).font(theme::small()).color(p.error));
                }
            });
        });
    }

    fn empty_state(&self, ui: &mut egui::Ui, p: Palette, title: &str, body: &str) {
        let rect = ui.available_rect_before_wrap();
        let centre = rect.center() - vec2(0.0, 40.0);
        theme::paint_motif(ui.painter(), centre, p);
        ui.painter().text(centre + vec2(0.0, 66.0), Align2::CENTER_CENTER, title, theme::card_title(), p.text);
        let width = rect.width().min(380.0);
        // Centre each wrapped line, as a short note under the title.
        let mut job = egui::text::LayoutJob::simple(body.to_owned(), theme::body(), p.muted, width);
        job.halign = Align::Center;
        let centred = ui.painter().layout_job(job);
        ui.painter().galley(pos2(centre.x, centre.y + 84.0), centred, p.muted);
        ui.allocate_rect(rect, Sense::hover());
    }

    fn failed_state(&self, ui: &mut egui::Ui, p: Palette, message: &str) {
        let rect = ui.available_rect_before_wrap();
        let centre = rect.center() - vec2(0.0, 30.0);
        let disc = Rect::from_center_size(centre, vec2(56.0, 56.0));
        ui.painter().rect_filled(disc, 28, p.error_soft);
        theme::paint_icon(ui.painter(), centre, Icon::Alert, p.error);
        ui.painter().text(
            centre + vec2(0.0, 54.0),
            Align2::CENTER_CENTER,
            "This file couldn't be transcribed",
            theme::card_title(),
            p.text,
        );
        let mut job = egui::text::LayoutJob::simple(message.to_owned(), theme::body(), p.muted, rect.width().min(400.0));
        job.halign = Align::Center;
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(pos2(centre.x, centre.y + 74.0), galley, p.muted);
        ui.allocate_rect(rect, Sense::hover());
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            let mut auto_update = self.settings.auto_update;
            let label = egui::RichText::new("Automatically update").font(theme::small()).color(p.muted);
            if theme::checkbox(ui, &mut auto_update, label)
                .on_hover_text("Checks for new versions about once a day and installs them in the background.")
                .changed()
            {
                self.settings.auto_update = auto_update;
                self.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", self.version))
                        .font(theme::small())
                        .color(p.muted),
                );
            });
        });
    }
}

fn search_id() -> egui::Id {
    egui::Id::new("transcript-search")
}

/// What a file row says under its name.
fn row_status(item: &Item) -> (Tone, String) {
    let length = item.seconds.map(transcript::format_length);
    let with_length = |text: &str| match &length {
        Some(l) => format!("{l}  ·  {text}"),
        None => text.to_owned(),
    };
    match &item.state {
        ItemState::Waiting => (Tone::Neutral, with_length("Waiting")),
        ItemState::Running => {
            let text = match item.progress.fraction() {
                Some(f) => format!("{}%", (f * 100.0).floor() as u32),
                None => "Starting".to_owned(),
            };
            (Tone::Accent, text)
        }
        ItemState::Done => (Tone::Success, with_length("Done")),
        ItemState::Stopped => (Tone::Warn, with_length("Stopped early, text kept")),
        ItemState::Failed(_) => (Tone::Error, with_length("Couldn't transcribe")),
    }
}

/// 12345 -> "12,345".
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A paragraph's text with search matches marked; `current` stands out.
fn paragraph_job(
    text: &str,
    ranges: &[(usize, usize)],
    current: Option<(usize, usize)>,
    width: f32,
    p: Palette,
) -> egui::text::LayoutJob {
    let plain = egui::TextFormat {
        font_id: theme::reading(),
        color: p.text,
        line_height: Some(theme::READING_LINE),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    let mut at = 0;
    for &(start, end) in ranges {
        if start < at {
            continue;
        }
        job.append(&text[at..start], 0.0, plain.clone());
        let mut marked = plain.clone();
        if Some((start, end)) == current {
            marked.background = p.accent;
            marked.color = p.on_accent;
        } else {
            marked.background = p.accent_soft;
            marked.color = p.text;
        }
        job.append(&text[start..end], 0.0, marked);
        at = end;
    }
    job.append(&text[at..], 0.0, plain);
    job
}

fn show_in_folder(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui_root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui_root.ctx().clone();
        if let Some(scene) = self.shots.as_mut().and_then(|shots| shots.drive(&ctx, SHOT_SCENES)) {
            self.show_scene(scene);
        }
        self.poll();
        if self.shots.is_none() {
            let dropped: Vec<PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            });
            if !dropped.is_empty() {
                self.add_paths(dropped, &ctx);
            }
        }
        self.handle_shortcuts(&ctx);
        if self.busy() {
            ctx.request_repaint_after(BUSY_REPAINT);
        }
        let p = theme::palette(&ctx);
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui_root, |ui| self.footer(ui));
        egui::Panel::left("side")
            .exact_size(SIDE_WIDTH + 16.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin {
                left: 16,
                right: 0,
                top: 16,
                bottom: 4,
            }))
            .show(ui_root, |ui| self.side_ui(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin {
                left: 12,
                right: 16,
                top: 16,
                bottom: 4,
            }))
            .show(ui_root, |ui| self.transcript_ui(ui));
    }
}

// Made-up transcripts for the Store screenshots: (seconds, text).

const MEETING: &[(u64, &str)] = &[
    (0, " Okay, I think everyone's here, so let's start."),
    (3, " The main thing today is the move to the new office in January,"),
    (8, " and then a quick look at the numbers from October."),
    (14, " Priya, do you want to begin with the move?"),
    (19, "| Sure. So the lease is signed and the keys come on the sixth of January."),
    (25, " The plan is to move desks over two weekends, the tenth and the seventeenth,"),
    (31, " so nobody loses a working day."),
    (34, " IT need a list of who sits where by the fifteenth of December."),
    (40, " I'll send a sheet round this afternoon."),
    (44, " Does anyone need a quiet room for calls?"),
    (51, "| Yes, the support team, definitely. We're on calls most of the day."),
    (57, "| Okay, noted. There are three small rooms on the second floor."),
    (62, " We could keep two of them for support and book the third."),
    (70, "| That works. Moving on to October."),
    (73, " Sales were up about eight percent on September,"),
    (77, " mostly from the two new accounts in the north."),
    (82, " Costs were flat, apart from travel, which went up with the site visits."),
    (89, " I'd like us to look at travel again before the budget review in December."),
    (96, " Can we put that on the agenda for next week?"),
    (101, "| Yes. I'll add it. Anything else before we finish?"),
];

const LECTURE: &[(u64, &str)] = &[
    (2_210, " So where we left off last week was the harvest failures of the 1840s."),
    (2_216, " And I want to start today by asking why the response was so slow."),
    (2_222, " It wasn't that nobody knew. The reports were arriving every week."),
    (2_228, " Officials were writing back to London with numbers that were, frankly, alarming."),
    (2_236, " The problem was partly ideological."),
    (2_240, " There was a strong belief that the market would correct itself,"),
    (2_245, " and that relief would make people dependent on it."),
    (2_252, " If you read the correspondence from that winter, that word comes up again and again."),
    (2_260, " Dependent. Dependence."),
    (2_266, "| Now, the second part of the answer is practical."),
    (2_270, " The roads in the west were poor, and the stores were in the wrong places."),
    (2_277, " Grain was being shipped out of the very districts that needed it,"),
    (2_283, " because that was where the buyers were."),
    (2_289, " So for your essays, I'd like you to look at both of these together."),
    (2_294, " Not one or the other."),
];

const INTERVIEW: &[(u64, &str)] = &[
    (0, " Thanks for making the time. Could you start by telling us what the project is?"),
    (6, "| Of course. We're looking at how small towns plan their water supply,"),
    (11, " particularly where the population has grown quickly in the last ten years."),
    (19, "| And what made you choose that?"),
    (23, "| Honestly, it started with a budget problem."),
    (27, " One of the councils we work with had planned for eight thousand people and now has twelve."),
    (34, " The pipes were fine. The budget wasn't."),
    (40, " So we wanted to know how often that happens, and what people do about it."),
    (47, "| What have you found so far?"),
    (51, "| That it's common. Far more common than we expected."),
    (56, " Most places don't revisit the budget until something actually breaks,"),
    (62, " and by then the repair costs three or four times what upkeep would have."),
    (70, "| Is there an example you can share?"),
    (74, "| There's a town in the south where a single pump failure took a quarter of the year's budget."),
    (81, " They had to delay a school roof to pay for it."),
];

const PODCAST: &[(u64, &str)] = &[
    (0, " Herzlich willkommen zu Folge 112. Heute geht es um das Thema Ehrenamt."),
    (6, " Mein Gast ist Jana, die seit zehn Jahren eine Tafel in Leipzig leitet."),
    (13, " Jana, schön, dass du da bist."),
    (16, "| Danke für die Einladung. Ich freue mich."),
    (21, "| Wie bist du eigentlich dazu gekommen?"),
    (25, "| Ehrlich gesagt durch Zufall. Eine Freundin hat mich gefragt, ob ich mal aushelfen kann,"),
    (32, " und dann bin ich einfach geblieben."),
    (38, "| Was hat sich in den zehn Jahren verändert?"),
    (42, "| Vor allem die Zahl der Menschen, die zu uns kommen. Sie hat sich fast verdoppelt."),
    (49, " Gleichzeitig haben wir weniger Spenden von den Supermärkten,"),
    (54, " weil die selbst besser planen und weniger übrig bleibt."),
    (60, " Das ist eigentlich gut, aber für uns natürlich schwierig."),
    (67, "| Wie geht ihr damit um?"),
    (70, "| Wir arbeiten jetzt mit Bauernhöfen aus der Region zusammen."),
    (75, " Die geben uns Gemüse, das nicht schön genug für den Handel ist."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names_are_capitalised() {
        assert_eq!(capitalize("english"), "English");
        assert_eq!(capitalize("haitian creole"), "Haitian creole");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn whisper_languages_are_listed_once_by_name() {
        let list = languages();
        assert!(list.len() > 90);
        assert!(list.iter().any(|(c, n)| c == "en" && n == "English"));
        assert!(list.windows(2).all(|w| w[0].1 <= w[1].1));
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(7), "7");
        assert_eq!(group_thousands(1_234), "1,234");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn video_files_get_the_film_tile() {
        assert!(is_video(Path::new("a.MP4")));
        assert!(is_video(Path::new("a.webm")));
        assert!(!is_video(Path::new("a.m4a")));
    }

    #[test]
    fn rows_say_where_each_file_is() {
        let mut item = Item::new(1, PathBuf::from("talk.mp3"));
        item.seconds = Some(42.0 * 60.0);
        assert_eq!(row_status(&item), (Tone::Neutral, "42 min  ·  Waiting".to_owned()));
        item.state = ItemState::Running;
        item.progress.set_fraction(0.415);
        assert_eq!(row_status(&item), (Tone::Accent, "41%".to_owned()));
        item.state = ItemState::Failed("x".into());
        assert_eq!(row_status(&item).0, Tone::Error);
    }

    #[test]
    fn search_marks_are_laid_out_in_order() {
        let p = theme::palette(&egui::Context::default());
        let job = paragraph_job("a budget and a budget", &[(2, 8), (15, 21)], Some((15, 21)), 300.0, p);
        let texts: Vec<&str> = job.sections.iter().map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        assert_eq!(texts, ["a ", "budget", " and a ", "budget", ""]);
        assert_eq!(job.sections[3].format.background, p.accent);
    }
}
