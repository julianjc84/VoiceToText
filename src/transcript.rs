use serde::{Deserialize, Serialize};
use std::fs;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub timestamp: i64,
    pub datetime: String,
    pub text: String,
    #[serde(default)]
    pub process_time_ms: u64,
}

pub fn load_all() -> Vec<Transcript> {
    let path = config::transcripts_path();
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_all(transcripts: &[Transcript]) {
    let path = config::transcripts_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(transcripts) {
        let _ = fs::write(&path, json);
    }
}

pub fn save_transcript(text: &str, max: u32, process_time_ms: u64) {
    if text.is_empty() {
        return;
    }
    let now = chrono::Local::now();
    let entry = Transcript {
        timestamp: now.timestamp(),
        datetime: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        text: text.to_string(),
        process_time_ms,
    };
    let mut all = load_all();
    all.push(entry);
    if max > 0 && all.len() > max as usize {
        let excess = all.len() - max as usize;
        all.drain(..excess);
    }
    save_all(&all);
}

pub fn delete_transcript(timestamp: i64) -> bool {
    let mut all = load_all();
    let before = all.len();
    all.retain(|t| t.timestamp != timestamp);
    if all.len() < before {
        save_all(&all);
        true
    } else {
        false
    }
}

pub fn enforce_max(max: u32) {
    if max == 0 {
        return;
    }
    let mut all = load_all();
    if all.len() > max as usize {
        let excess = all.len() - max as usize;
        all.drain(..excess);
        save_all(&all);
    }
}

pub fn clear_all() {
    save_all(&[]);
}
