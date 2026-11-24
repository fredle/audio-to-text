//! Small persisted preferences in %LOCALAPPDATA%\AudioToText\settings.json.
//! Add app-specific fields here, always with `#[serde(default)]` so older
//! settings files keep loading.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// Check for updates in the background and install them silently.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Unix seconds of the last background update check, to throttle to once
    /// a day.
    #[serde(default)]
    pub last_update_check: u64,
    /// Whisper language code, or "auto" to detect it.
    #[serde(default = "default_language")]
    pub language: String,
    /// "small" or "large".
    #[serde(default = "default_model")]
    pub model: String,
    /// A licence key, once one is entered (unused while there is no paid
    /// tier).
    #[serde(default)]
    pub license_key: String,
}

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "auto".to_owned()
}

fn default_model() -> String {
    "small".to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_update: true,
            last_update_check: 0,
            language: default_language(),
            model: default_model(),
            license_key: String::new(),
        }
    }
}

pub fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into()))
        .join("AudioToText")
}

fn settings_path() -> std::path::PathBuf {
    data_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(settings_path(), json);
    }
}
