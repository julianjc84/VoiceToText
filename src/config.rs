use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const SAMPLE_RATE: u32 = 16000;
pub fn whisper_threads() -> i32 {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    (cpus - 2).max(2)
}

// Chunk mode configuration (fixed-size segments, no VAD model needed)
pub const CHUNK_DURATION_MIN: f32 = 2.0;
pub const CHUNK_DURATION_MAX: f32 = 10.0;
pub const SILENCE_THRESHOLD: f32 = 0.01;
pub const CHUNK_MIN_FLUSH_SAMPLES: usize = (SAMPLE_RATE as f32 * 0.3) as usize; // 0.3s minimum for flush

fn default_chunk_duration() -> f32 {
    3.0
}

// VAD configuration
pub const VAD_FRAME_SIZE: usize = 256; // 16ms at 16kHz (required by TEN VAD)
pub const VAD_THRESHOLD: f32 = 0.5; // Speech probability threshold
pub const VAD_PRE_SPEECH_PAD_MS: usize = 300; // Ring buffer before speech onset
pub const VAD_POST_SPEECH_PAD_MS: usize = 600; // Silence before ending utterance
pub const VAD_MIN_SPEECH_MS: usize = 1000; // Whisper needs >= 1s
pub const VAD_MAX_SPEECH_SECS: f32 = 20.0; // Force-send limit

// Derived sample counts
pub const VAD_PRE_SPEECH_SAMPLES: usize = SAMPLE_RATE as usize * VAD_PRE_SPEECH_PAD_MS / 1000;
pub const VAD_POST_SPEECH_SAMPLES: usize = SAMPLE_RATE as usize * VAD_POST_SPEECH_PAD_MS / 1000;
pub const VAD_MIN_SPEECH_SAMPLES: usize = SAMPLE_RATE as usize * VAD_MIN_SPEECH_MS / 1000;
pub const VAD_MAX_SPEECH_SAMPLES: usize = (SAMPLE_RATE as f32 * VAD_MAX_SPEECH_SECS) as usize;

pub const VAD_MODEL_URL: &str =
    "https://huggingface.co/TEN-framework/ten-vad/resolve/main/src/onnx_model/ten-vad.onnx";
pub const VAD_MODEL_FILENAME: &str = "ten-vad.onnx";

pub fn vad_model_path() -> PathBuf {
    models_dir().join(VAD_MODEL_FILENAME)
}

pub const MODEL_FILENAME: &str = "ggml-base.en-q8_0.bin";

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct ModelInfo {
    pub filename: &'static str,
    pub label: &'static str,
    pub size: &'static str,
    pub description: &'static str,
    pub url: &'static str,
}

pub const AVAILABLE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        filename: "ggml-base.en-q8_0.bin",
        label: "Base",
        size: "82 MB",
        description: "Fast, good for most use",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q8_0.bin",
    },
    ModelInfo {
        filename: "ggml-small.en-q8_0.bin",
        label: "Small",
        size: "180 MB",
        description: "More accurate",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q8_0.bin",
    },
    ModelInfo {
        filename: "ggml-medium.en-q8_0.bin",
        label: "Medium",
        size: "460 MB",
        description: "Most accurate",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en-q8_0.bin",
    },
];

pub const DEFAULT_SHORTCUT: &str = "ctrl+space";
pub const DEFAULT_TRANSCRIPT_SHORTCUT: &str = "ctrl+shift+t";
pub const DEFAULT_ALWAYS_LISTEN_SHORTCUT: &str = "ctrl+shift+l";

/// Convert internal shortcut format to pretty display.
/// e.g. "ctrl+shift+a" → "Ctrl + Shift + A"
pub fn display_shortcut(shortcut: &str) -> String {
    shortcut
        .split('+')
        .map(|part| {
            match part.trim() {
                "ctrl" => "Ctrl".to_string(),
                "alt" => "Alt".to_string(),
                "shift" => "Shift".to_string(),
                "super" => "Super".to_string(),
                "scrolllock" | "scroll_lock" => "Scroll Lock".to_string(),
                "pageup" => "Page Up".to_string(),
                "pagedown" => "Page Down".to_string(),
                "printscreen" => "Print Screen".to_string(),
                "capslock" => "Caps Lock".to_string(),
                "numlock" => "Num Lock".to_string(),
                "backspace" => "Backspace".to_string(),
                "leftbracket" => "[".to_string(),
                "rightbracket" => "]".to_string(),
                "semicolon" => ";".to_string(),
                "apostrophe" => "'".to_string(),
                "grave" => "`".to_string(),
                "comma" => ",".to_string(),
                "period" => ".".to_string(),
                "slash" => "/".to_string(),
                "backslash" => "\\".to_string(),
                "minus" => "-".to_string(),
                "equal" => "=".to_string(),
                other => {
                    // Uppercase single letters, F-keys, etc.
                    let mut c = other.chars();
                    match c.next() {
                        None => String::new(),
                        Some(first) => {
                            first.to_uppercase().collect::<String>() + c.as_str()
                        }
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

const DANGEROUS_SHORTCUTS: &[&str] = &[
    "ctrl+c", "ctrl+v", "ctrl+x", "ctrl+z", "ctrl+y",
    "ctrl+s", "ctrl+w", "ctrl+q", "ctrl+a", "ctrl+f",
    "ctrl+p", "ctrl+n", "ctrl+t", "ctrl+o",
    "alt+f4", "ctrl+shift+t",
];

pub fn is_dangerous_shortcut(shortcut: &str) -> bool {
    let lower = shortcut.to_lowercase();
    DANGEROUS_SHORTCUTS.contains(&lower.as_str())
}

/// Keys allowed without a modifier.
const STANDALONE_KEYS: &[&str] = &[
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
    "scrolllock", "scroll_lock", "pause", "printscreen", "capslock", "numlock",
];

/// Validate a shortcut string. Returns Ok(()) if valid, Err with message otherwise.
pub fn validate_shortcut(shortcut: &str) -> Result<(), &'static str> {
    if shortcut.is_empty() {
        return Err("No shortcut entered");
    }

    let parts: Vec<&str> = shortcut.split('+').collect();
    let modifiers: Vec<&&str> = parts.iter().filter(|p| matches!(**p, "ctrl" | "alt" | "shift" | "super")).collect();
    let non_modifiers: Vec<&&str> = parts.iter().filter(|p| !matches!(**p, "ctrl" | "alt" | "shift" | "super")).collect();

    if non_modifiers.is_empty() {
        return Err("Press a non-modifier key");
    }

    let base_key = non_modifiers[0];
    if modifiers.is_empty() && !STANDALONE_KEYS.contains(base_key) {
        return Err("Add a modifier (Ctrl, Alt, Shift, Super)");
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    Toggle,
    PushToTalk,
}

impl Default for RecordingMode {
    fn default() -> Self {
        RecordingMode::PushToTalk
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub clipboard_auto_copy: bool,
    #[serde(default)]
    pub use_vad: bool,
    #[serde(default = "default_chunk_duration")]
    pub chunk_duration_secs: f32,
    #[serde(default)]
    pub recording_mode: RecordingMode,
    #[serde(default = "default_shortcut")]
    pub shortcut: String,
    #[serde(default = "default_transcript_shortcut")]
    pub transcript_shortcut: String,
    #[serde(default = "default_always_listen_shortcut")]
    pub always_listen_shortcut: String,
    #[serde(default = "default_max_transcripts")]
    pub max_transcripts: u32,
}

fn default_shortcut() -> String {
    DEFAULT_SHORTCUT.to_string()
}

fn default_transcript_shortcut() -> String {
    DEFAULT_TRANSCRIPT_SHORTCUT.to_string()
}

fn default_always_listen_shortcut() -> String {
    DEFAULT_ALWAYS_LISTEN_SHORTCUT.to_string()
}

fn default_max_transcripts() -> u32 {
    0
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: MODEL_FILENAME.to_string(),
            clipboard_auto_copy: true,
            use_vad: false,
            chunk_duration_secs: default_chunk_duration(),
            recording_mode: RecordingMode::default(),
            shortcut: default_shortcut(),
            transcript_shortcut: default_transcript_shortcut(),
            always_listen_shortcut: default_always_listen_shortcut(),
            max_transcripts: default_max_transcripts(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
                eprintln!("WARNING: Failed to parse config: {}", e);
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| String::from("/home")))
}

fn config_path() -> PathBuf {
    home_dir().join(".config/voice-to-text/config.toml")
}

pub fn data_dir() -> PathBuf {
    home_dir().join(".local/share/voice-to-text")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn transcripts_path() -> PathBuf {
    data_dir().join("transcripts.json")
}

pub fn model_path() -> PathBuf {
    let config = Config::load();
    models_dir().join(&config.model)
}

pub fn model_path_for(filename: &str) -> PathBuf {
    models_dir().join(filename)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayServer {
    X11,
    Wayland,
}

impl DisplayServer {
    pub fn detect() -> Self {
        match std::env::var("XDG_SESSION_TYPE").as_deref() {
            Ok("wayland") => DisplayServer::Wayland,
            _ => DisplayServer::X11,
        }
    }
}

impl std::fmt::Display for DisplayServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplayServer::X11 => write!(f, "X11"),
            DisplayServer::Wayland => write!(f, "Wayland"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActiveBackend {
    Evdev,
    GlobalHotkey,
}

#[derive(Debug, Clone)]
pub enum AppCommand {
    ToggleRecording,
    ToggleAlwaysListen,
    StartRecording,
    StopRecording,
    OpenSettings,
    OpenTranscripts,
    ReloadConfig,
    HotkeyBackendResolved(ActiveBackend),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordingState {
    Idle,
    PushToTalk,
    AlwaysListen,
}

impl RecordingState {
    pub fn is_recording(self) -> bool {
        matches!(self, Self::PushToTalk | Self::AlwaysListen)
    }
}
