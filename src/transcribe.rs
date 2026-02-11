use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config;

pub enum TranscribeCommand {
    ReloadModel(String),
}

/// Result from the transcription thread: text + how long whisper took.
pub struct TranscribeResult {
    pub text: String,
    pub process_time_ms: u64,
}

fn try_load_model(path: &str) -> Option<whisper_rs::WhisperState> {
    if !std::path::Path::new(path).exists() {
        return None;
    }
    eprintln!("Loading whisper model from: {}", path);
    match WhisperContext::new_with_params(path, WhisperContextParameters::default()) {
        Ok(ctx) => match ctx.create_state() {
            Ok(state) => {
                eprintln!("Whisper model loaded");
                Some(state)
            }
            Err(e) => {
                eprintln!("Failed to create whisper state: {}", e);
                None
            }
        },
        Err(e) => {
            eprintln!("Failed to load whisper model: {}", e);
            None
        }
    }
}

pub fn transcription_loop(
    model_path: &str,
    chunk_rx: Receiver<Vec<f32>>,
    text_tx: Sender<TranscribeResult>,
    ctrl_rx: Receiver<TranscribeCommand>,
) {
    let mut state = try_load_model(model_path);
    if state.is_none() {
        eprintln!("Whisper model not found — download it in Settings");
    }
    eprintln!("Transcription thread ready");

    loop {
        crossbeam_channel::select! {
            recv(chunk_rx) -> chunk => {
                match chunk {
                    Ok(c) if c.is_empty() => {
                        // Sentinel from VAD flush — forward to coordinator
                        let _ = text_tx.send(TranscribeResult {
                            text: String::new(),
                            process_time_ms: 0,
                        });
                    }
                    Ok(chunk) => {
                        if let Some(s) = state.as_mut() {
                            transcribe_chunk(s, &chunk, &text_tx);
                        }
                    }
                    Err(_) => break,
                }
            }
            recv(ctrl_rx) -> cmd => {
                match cmd {
                    Ok(TranscribeCommand::ReloadModel(filename)) => {
                        let new_path = config::model_path_for(&filename);
                        let path_str = new_path.to_string_lossy().to_string();
                        if let Some(new_state) = try_load_model(&path_str) {
                            state = Some(new_state);
                            eprintln!("Model reloaded successfully: {}", filename);
                        } else if state.is_some() {
                            eprintln!("Failed to load new model, keeping current");
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    eprintln!("Transcription thread exiting");
}

fn transcribe_chunk(
    state: &mut whisper_rs::WhisperState,
    chunk: &[f32],
    text_tx: &Sender<TranscribeResult>,
) {
    // Whisper needs minimum 1s (16000 samples). Pad with silence if needed.
    const MIN_SAMPLES: usize = config::SAMPLE_RATE as usize;
    let audio: std::borrow::Cow<[f32]> = if chunk.len() < MIN_SAMPLES {
        let mut padded = chunk.to_vec();
        padded.resize(MIN_SAMPLES, 0.0);
        std::borrow::Cow::Owned(padded)
    } else {
        std::borrow::Cow::Borrowed(chunk)
    };

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(config::whisper_threads());
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    let start = Instant::now();
    match state.full(params, &audio) {
        Ok(_) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let n = state.full_n_segments().unwrap_or(0);
            let mut text = String::new();
            for i in 0..n {
                if let Ok(seg) = state.full_get_segment_text(i) {
                    text.push_str(&seg);
                }
            }
            let text = text.trim().to_string();

            if !text.is_empty() && !is_hallucination(&text) {
                let _ = text_tx.send(TranscribeResult {
                    text,
                    process_time_ms: elapsed_ms,
                });
            }
        }
        Err(e) => {
            eprintln!("Transcription error: {}", e);
        }
    }
}

fn is_hallucination(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('[') && t.ends_with(']')
        || t.starts_with('(') && t.ends_with(')')
        || t == "you"
        || t == "Thank you."
        || t == "Thanks for watching!"
        || t == "Bye."
        || t == "..."
}
