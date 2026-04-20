//! Microphone / audio-stack diagnostic for Linux.
//!
//! This is **not** an example of how to use `voice-to-text` — it doesn't
//! import the crate and nothing in it demonstrates the public API. It's a
//! standalone probe that uses `cpal` directly to isolate "is my mic
//! broken?" from "is the transcription pipeline broken?". It lives under
//! `examples/` purely for the Cargo ergonomics: any file there becomes
//! `cargo run --example <name>` with no `Cargo.toml` changes needed.
//!
//! # When to reach for it
//!
//! Run this when transcription comes out as silence or garbage on a Linux
//! box and you need to bisect where the fault lies:
//!
//! - If every config reports `0%` non-zero samples, the problem is below
//!   `cpal` — ALSA/PipeWire routing (often fixed on Arch by installing
//!   `pipewire-alsa`), a muted hardware capture channel, or permissions on
//!   `/dev/snd/*`.
//! - If you see healthy non-zero samples at `1ch 16000Hz`, `cpal` is fine
//!   and the bug lives higher up in VAD / whisper / the coordinator.
//!
//! # Running
//!
//! ```sh
//! cargo run --example diagnose_mic --release
//! # then make some noise for ~8 seconds
//! ```
//!
//! Probes four configs in sequence: 1ch@16kHz (what VoiceToText actually
//! uses for whisper), 1ch@48kHz, 2ch@48kHz, 4ch@48kHz. For each it opens
//! an input stream for 2 seconds and reports total samples, non-zero count,
//! and non-zero percentage. A healthy mic shows 1-100% non-zero depending
//! on room noise; a broken stream shows 0%.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn test_config(device: &cpal::Device, channels: u16, rate: u32) {
    let sc = Arc::new(AtomicUsize::new(0));
    let nz = Arc::new(AtomicUsize::new(0));
    let sc2 = sc.clone();
    let nz2 = nz.clone();

    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(rate),
        buffer_size: cpal::BufferSize::Default,
    };

    print!("  {}ch {}Hz ... ", channels, rate);
    match device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            sc2.fetch_add(data.len(), Ordering::Relaxed);
            nz2.fetch_add(
                data.iter().filter(|&&s| s != 0.0).count(),
                Ordering::Relaxed,
            );
        },
        |err| eprintln!("err: {}", err),
        None,
    ) {
        Ok(stream) => {
            stream.play().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(2));
            stream.pause().ok();
            let total = sc.load(Ordering::Relaxed);
            let nonzero = nz.load(Ordering::Relaxed);
            println!(
                "OK  samples={}, non-zero={} ({}%)",
                total,
                nonzero,
                if total > 0 { nonzero * 100 / total } else { 0 }
            );
        }
        Err(e) => println!("FAIL: {}", e),
    }
}

fn main() {
    let host = cpal::default_host();
    let device = host.default_input_device().expect("no input device");
    println!("Device: {}", device.name().unwrap_or("unknown".into()));

    if let Ok(configs) = device.supported_input_configs() {
        let configs: Vec<_> = configs.collect();
        if configs.is_empty() {
            println!("Supported configs: (none returned)");
        } else {
            println!("Supported configs:");
            for c in &configs {
                println!(
                    "  ch={} rate={}-{} fmt={:?}",
                    c.channels(),
                    c.min_sample_rate().0,
                    c.max_sample_rate().0,
                    c.sample_format()
                );
            }
        }
    }

    if let Ok(d) = device.default_input_config() {
        println!(
            "Default config: ch={} rate={} fmt={:?}",
            d.channels(),
            d.sample_rate().0,
            d.sample_format()
        );
    } else {
        println!("Default config: unavailable");
    }

    println!("\nTesting configs (make noise!):");
    test_config(&device, 1, 16000);
    test_config(&device, 1, 48000);
    test_config(&device, 2, 48000);
    test_config(&device, 4, 48000);
}
