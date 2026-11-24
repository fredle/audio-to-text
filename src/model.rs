//! The speech models: which file each is, where it lives, and the one-off
//! download from Hugging Face. A download resumes where it stopped, checks
//! its size against what the server said and the file's ggml header, and
//! only then takes the model's real name, so a half-written file is never
//! used.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Small,
    Large,
}

impl Model {
    pub const ALL: [Model; 2] = [Model::Small, Model::Large];

    pub fn file_name(self) -> &'static str {
        match self {
            Model::Small => "ggml-small-q5_1.bin",
            Model::Large => "ggml-large-v3-turbo-q5_0.bin",
        }
    }

    pub fn url(self) -> String {
        format!("{BASE_URL}{}", self.file_name())
    }

    /// The published size, for showing before the download and as a sanity
    /// check on what arrives.
    pub fn approx_bytes(self) -> u64 {
        match self {
            Model::Small => 181 * MIB,
            Model::Large => 547 * MIB,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Model::Small => "Small",
            Model::Large => "Large",
        }
    }

    /// How it is saved in settings.
    pub fn key(self) -> &'static str {
        match self {
            Model::Small => "small",
            Model::Large => "large",
        }
    }

    pub fn from_key(key: &str) -> Model {
        match key {
            "large" => Model::Large,
            _ => Model::Small,
        }
    }

    pub fn path(self) -> PathBuf {
        models_dir().join(self.file_name())
    }

    fn part_path(self) -> PathBuf {
        models_dir().join(format!("{}.part", self.file_name()))
    }

    /// Records the size the server gave, so a later check knows the file is
    /// whole.
    fn size_path(self) -> PathBuf {
        models_dir().join(format!("{}.size", self.file_name()))
    }

    /// Whether the model is on disk, whole and looks like a ggml model.
    pub fn is_ready(self) -> bool {
        let Ok(meta) = std::fs::metadata(self.path()) else {
            return false;
        };
        let recorded = std::fs::read_to_string(self.size_path())
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let mut head = [0u8; 4];
        let header_ok = File::open(self.path())
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok()
            && is_ggml(&head);
        header_ok && size_ok(meta.len(), recorded, self.approx_bytes())
    }

    /// Deletes the model, so the next use downloads it again.
    pub fn remove(self) {
        let _ = std::fs::remove_file(self.path());
        let _ = std::fs::remove_file(self.size_path());
    }

    /// Bytes of an unfinished download already on disk.
    pub fn partial_bytes(self) -> u64 {
        std::fs::metadata(self.part_path()).map(|m| m.len()).unwrap_or(0)
    }
}

pub const MIB: u64 = 1024 * 1024;

pub fn models_dir() -> PathBuf {
    crate::settings::data_dir().join("models")
}

/// A ggml model file starts with the magic number 0x67676d6c, little-endian.
pub fn is_ggml(head: &[u8]) -> bool {
    head.len() >= 4 && head[..4] == 0x6767_6d6cu32.to_le_bytes()
}

/// Whether a file `len` bytes long is the whole model: exactly the size the
/// server reported when known, and within a tenth of the published size.
pub fn size_ok(len: u64, recorded: Option<u64>, approx: u64) -> bool {
    let near = len.abs_diff(approx) <= approx / 10;
    near && recorded.is_none_or(|r| r == len)
}

/// The total from a `Content-Range: bytes 100-199/1000` header.
pub fn total_from_content_range(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let (_, total) = rest.split_once('/')?;
    total.trim().parse().ok()
}

/// How far a download has got, and whether to stop. Shared between the
/// downloading thread and the window.
#[derive(Default, Debug)]
pub struct DownloadProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    cancel: AtomicBool,
}

impl DownloadProgress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 0.0 to 1.0, or `None` before the size is known.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// No connection, or the server could not be reached.
    Network(String),
    /// Could not write to the app's data folder.
    Disk(String),
    /// What arrived is not a whole model; it was deleted.
    Damaged,
    Cancelled,
}

impl DownloadError {
    pub fn message(&self) -> String {
        match self {
            DownloadError::Network(_) => {
                "The download stopped. Check your internet connection and try again; it picks up where it left off.".to_owned()
            }
            DownloadError::Disk(e) => format!("Couldn't save the model to this PC: {e}. Free some disk space and try again."),
            DownloadError::Damaged => "The downloaded model was damaged, so it was deleted. Try again to download it fresh.".to_owned(),
            DownloadError::Cancelled => "Download stopped. Try again to pick up where it left off.".to_owned(),
        }
    }
}

fn disk(e: std::io::Error) -> DownloadError {
    DownloadError::Disk(e.to_string())
}

/// Downloads `model`, resuming an earlier partial download. Blocks; run it
/// on its own thread.
pub fn download(model: Model, progress: &DownloadProgress) -> Result<(), DownloadError> {
    std::fs::create_dir_all(models_dir()).map_err(disk)?;
    let result = download_once(model, progress);
    // A server that refuses the resume point: start clean once.
    if let Err(DownloadError::Network(ref e)) = result
        && e.contains("416")
    {
        let _ = std::fs::remove_file(model.part_path());
        return download_once(model, progress);
    }
    result
}

fn download_once(model: Model, progress: &DownloadProgress) -> Result<(), DownloadError> {
    let part = model.part_path();
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut request = agent.get(model.url());
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let mut response = request
        .call()
        .map_err(|e| DownloadError::Network(e.to_string()))?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let resumed = response.status().as_u16() == 206;
    let total = if resumed {
        header("content-range").as_deref().and_then(total_from_content_range)
    } else {
        header("content-length").and_then(|v| v.trim().parse().ok())
    };
    let start = if resumed { have } else { 0 };
    if let Some(total) = total {
        progress.total.store(total, Ordering::Relaxed);
        std::fs::write(model.size_path(), total.to_string()).map_err(disk)?;
    }
    progress.done.store(start, Ordering::Relaxed);

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .map_err(disk)?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut done = start;
    loop {
        if progress.cancelled() {
            // Keep what arrived, to resume from next time.
            file.flush().map_err(disk)?;
            return Err(DownloadError::Cancelled);
        }
        let n = reader
            .read(&mut buffer)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(disk)?;
        done += n as u64;
        progress.done.store(done, Ordering::Relaxed);
    }
    file.flush().map_err(disk)?;
    drop(file);

    if total.is_some_and(|t| t != done) {
        return Err(DownloadError::Network("the connection closed early".to_owned()));
    }
    let mut head = [0u8; 4];
    let header_ok = File::open(&part)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && is_ggml(&head);
    if !header_ok || !size_ok(done, total, model.approx_bytes()) {
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(model.size_path());
        return Err(DownloadError::Damaged);
    }
    if total.is_none() {
        std::fs::write(model.size_path(), done.to_string()).map_err(disk)?;
    }
    std::fs::rename(&part, model.path()).map_err(disk)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_ggml_header() {
        assert!(is_ggml(b"lmggrest"));
        assert!(!is_ggml(b"<!DOCTYPE html>"));
        assert!(!is_ggml(b"lm"));
    }

    #[test]
    fn size_must_match_the_server_and_be_near_the_published_size() {
        let approx = 181 * MIB;
        assert!(size_ok(190_085_487, Some(190_085_487), approx));
        assert!(size_ok(190_085_487, None, approx));
        // Cut short.
        assert!(!size_ok(190_000_000, Some(190_085_487), approx));
        // An error page, or the wrong file.
        assert!(!size_ok(15_000, None, approx));
        assert!(!size_ok(547 * MIB, Some(547 * MIB), approx));
    }

    #[test]
    fn reads_the_total_from_content_range() {
        assert_eq!(total_from_content_range("bytes 100-199/1000"), Some(1000));
        assert_eq!(total_from_content_range(" bytes 0-0/190085487 "), Some(190_085_487));
        assert_eq!(total_from_content_range("bytes */1000"), Some(1000));
        assert_eq!(total_from_content_range("bytes 0-10/*"), None);
        assert_eq!(total_from_content_range("items 1-2/3"), None);
    }

    #[test]
    fn models_have_distinct_files_and_round_trip_their_keys() {
        assert_ne!(Model::Small.file_name(), Model::Large.file_name());
        for model in Model::ALL {
            assert_eq!(Model::from_key(model.key()), model);
            assert!(model.url().starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
            assert!(model.url().ends_with(".bin"));
        }
        assert_eq!(Model::from_key("something else"), Model::Small);
    }

    #[test]
    fn download_progress_fraction() {
        let p = DownloadProgress::default();
        assert_eq!(p.fraction(), None);
        p.total.store(200, Ordering::Relaxed);
        p.done.store(50, Ordering::Relaxed);
        assert_eq!(p.fraction(), Some(0.25));
        assert!(!p.cancelled());
        p.cancel();
        assert!(p.cancelled());
    }
}
