use std::collections::VecDeque;

use crossbeam_channel::{Receiver, Sender};
use ten_vad_rs::{AudioFrameBuffer, TenVad, TARGET_SAMPLE_RATE};

use crate::config::{
    self, CHUNK_MIN_FLUSH_SAMPLES, SILENCE_THRESHOLD, VAD_FRAME_SIZE, VAD_MAX_SPEECH_SAMPLES,
    VAD_MIN_SPEECH_SAMPLES, VAD_POST_SPEECH_SAMPLES, VAD_PRE_SPEECH_SAMPLES, VAD_THRESHOLD,
};

#[derive(Debug)]
pub enum VadCommand {
    Flush,
    ReloadModel,
    ReloadConfig,
}

enum VadState {
    Silence,
    Speech,
}

fn try_load_vad() -> Option<TenVad> {
    let path = config::vad_model_path();
    if !path.exists() {
        return None;
    }
    match TenVad::new(&path.to_string_lossy(), TARGET_SAMPLE_RATE) {
        Ok(v) => {
            eprintln!("VAD model loaded");
            Some(v)
        }
        Err(e) => {
            eprintln!("Failed to load VAD model: {}", e);
            None
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn vad_thread(
    raw_rx: Receiver<Vec<f32>>,
    chunk_tx: Sender<Vec<f32>>,
    cmd_rx: Receiver<VadCommand>,
) {
    let mut vad = try_load_vad();
    let cfg = config::Config::load();
    let mut use_vad = cfg.use_vad;
    let mut chunk_samples = (cfg.chunk_duration_secs * config::SAMPLE_RATE as f32) as usize;

    if use_vad && vad.is_none() {
        eprintln!("VAD thread waiting for model download...");
    }

    // VAD mode state
    let mut state = VadState::Silence;
    let mut frame_buf = AudioFrameBuffer::<i16>::new();
    let mut f32_buf: VecDeque<f32> = VecDeque::new();
    let mut pre_speech: VecDeque<f32> =
        VecDeque::with_capacity(VAD_PRE_SPEECH_SAMPLES + VAD_FRAME_SIZE);
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut silence_count: usize = 0;

    // Chunk mode state
    let mut chunk_buffer: Vec<f32> = Vec::new();

    // Reset all VAD and chunk state to initial values.
    macro_rules! reset_state {
        () => {
            state = VadState::Silence;
            speech_buffer.clear();
            pre_speech.clear();
            silence_count = 0;
            frame_buf = AudioFrameBuffer::new();
            f32_buf.clear();
            chunk_buffer.clear();
            if let Some(ref mut v) = vad {
                v.reset();
            }
        };
    }

    eprintln!(
        "VAD thread ready (mode: {})",
        if use_vad { "VAD" } else { "chunk" }
    );

    loop {
        crossbeam_channel::select! {
            recv(raw_rx) -> raw => {
                match raw {
                    Ok(samples) => {
                        if use_vad {
                            // --- VAD mode ---
                            let vad_ref = match vad.as_mut() {
                                Some(v) => v,
                                None => continue,
                            };

                            let i16_samples: Vec<i16> = samples.iter()
                                .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                                .collect();
                            frame_buf.append_samples(i16_samples);
                            f32_buf.extend(samples.iter());

                            while let Some(frame) = frame_buf.pop_frame(VAD_FRAME_SIZE) {
                                let f32_frame: Vec<f32> = f32_buf
                                    .drain(..VAD_FRAME_SIZE)
                                    .collect();

                                let score = match vad_ref.process_frame(&frame) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        eprintln!("VAD error: {}", e);
                                        continue;
                                    }
                                };

                                let is_speech = score >= VAD_THRESHOLD;

                                match state {
                                    VadState::Silence => {
                                        for &s in &f32_frame {
                                            if pre_speech.len() >= VAD_PRE_SPEECH_SAMPLES {
                                                pre_speech.pop_front();
                                            }
                                            pre_speech.push_back(s);
                                        }

                                        if is_speech {
                                            speech_buffer.clear();
                                            speech_buffer.extend(pre_speech.iter());
                                            pre_speech.clear();
                                            speech_buffer.extend_from_slice(&f32_frame);
                                            silence_count = 0;
                                            state = VadState::Speech;
                                        }
                                    }
                                    VadState::Speech => {
                                        speech_buffer.extend_from_slice(&f32_frame);

                                        if is_speech {
                                            silence_count = 0;
                                        } else {
                                            silence_count += VAD_FRAME_SIZE;
                                        }

                                        if speech_buffer.len() >= VAD_MAX_SPEECH_SAMPLES {
                                            eprintln!("VAD: force-sending at max duration ({:.1}s)",
                                                speech_buffer.len() as f32 / TARGET_SAMPLE_RATE as f32);
                                            send_speech(&mut speech_buffer, &chunk_tx);
                                            silence_count = 0;
                                        } else if silence_count >= VAD_POST_SPEECH_SAMPLES {
                                            send_speech(&mut speech_buffer, &chunk_tx);
                                            silence_count = 0;
                                            state = VadState::Silence;
                                            vad_ref.reset();
                                        }
                                    }
                                }
                            }
                        } else {
                            // --- Chunk mode ---
                            chunk_buffer.extend_from_slice(&samples);

                            while chunk_buffer.len() >= chunk_samples {
                                let chunk: Vec<f32> = chunk_buffer.drain(..chunk_samples).collect();

                                let level = rms(&chunk);

                                if level < SILENCE_THRESHOLD {
                                    eprintln!("Chunk: skipping silent segment (rms={:.4})", level);
                                } else {
                                    eprintln!("Chunk: sending {:.1}s segment (rms={:.4})",
                                        chunk.len() as f32 / config::SAMPLE_RATE as f32, level);
                                    let _ = chunk_tx.send(chunk);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            recv(cmd_rx) -> cmd => {
                match cmd {
                    Ok(VadCommand::Flush) => {
                        if use_vad {
                            // Flush VAD state
                            if !speech_buffer.is_empty() {
                                eprintln!("VAD: flushing final speech segment ({:.1}s)",
                                    speech_buffer.len() as f32 / TARGET_SAMPLE_RATE as f32);
                                send_speech(&mut speech_buffer, &chunk_tx);
                            }
                            reset_state!();
                        } else {
                            // Flush chunk buffer
                            if chunk_buffer.len() >= CHUNK_MIN_FLUSH_SAMPLES {
                                let level = rms(&chunk_buffer);
                                if level >= SILENCE_THRESHOLD {
                                    eprintln!("Chunk: flushing partial buffer ({:.1}s)",
                                        chunk_buffer.len() as f32 / config::SAMPLE_RATE as f32);
                                    let segment = std::mem::take(&mut chunk_buffer);
                                    let _ = chunk_tx.send(segment);
                                } else {
                                    chunk_buffer.clear();
                                }
                            } else {
                                chunk_buffer.clear();
                            }
                        }
                        // Send empty sentinel so the coordinator knows the pipeline is drained
                        let _ = chunk_tx.send(Vec::new());
                    }
                    Ok(VadCommand::ReloadModel) => {
                        if vad.is_none() {
                            vad = try_load_vad();
                        }
                    }
                    Ok(VadCommand::ReloadConfig) => {
                        let new_cfg = config::Config::load();
                        let new_use_vad = new_cfg.use_vad;
                        let new_chunk_samples =
                            (new_cfg.chunk_duration_secs * config::SAMPLE_RATE as f32) as usize;

                        if new_use_vad != use_vad || new_chunk_samples != chunk_samples {
                            eprintln!("VAD thread: config reloaded (mode={}, chunk={:.1}s)",
                                if new_use_vad { "VAD" } else { "chunk" },
                                new_cfg.chunk_duration_secs);
                            use_vad = new_use_vad;
                            chunk_samples = new_chunk_samples;
                            reset_state!();

                            // Try loading VAD model if switching to VAD mode
                            if use_vad && vad.is_none() {
                                vad = try_load_vad();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    eprintln!("VAD thread exiting");
}

fn send_speech(speech_buffer: &mut Vec<f32>, chunk_tx: &Sender<Vec<f32>>) {
    if speech_buffer.len() < VAD_MIN_SPEECH_SAMPLES {
        eprintln!(
            "VAD: discarding short segment ({:.2}s < {:.1}s min)",
            speech_buffer.len() as f32 / TARGET_SAMPLE_RATE as f32,
            VAD_MIN_SPEECH_SAMPLES as f32 / TARGET_SAMPLE_RATE as f32,
        );
        speech_buffer.clear();
        return;
    }

    eprintln!(
        "VAD: sending speech segment ({:.1}s)",
        speech_buffer.len() as f32 / TARGET_SAMPLE_RATE as f32
    );
    let segment = std::mem::take(speech_buffer);
    let _ = chunk_tx.send(segment);
}
