use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;

use crate::config::SAMPLE_RATE;

pub struct AudioCapture {
    stream: cpal::Stream,
}

// cpal::Stream is Send but not Sync; we only access AudioCapture from the coordinator thread
unsafe impl Sync for AudioCapture {}

impl AudioCapture {
    pub fn new(raw_tx: Sender<Vec<f32>>) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No audio input device found")?;

        eprintln!(
            "Audio device: {}",
            device.name().unwrap_or_else(|_| "unknown".into())
        );

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = raw_tx.try_send(data.to_vec());
            },
            |err| eprintln!("Audio stream error: {}", err),
            None,
        )?;

        // Some backends auto-start the stream on creation; pause until explicitly started
        stream.pause()?;

        Ok(Self { stream })
    }

    pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream.play()?;
        eprintln!("Audio capture started");
        Ok(())
    }

    pub fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream.pause()?;
        eprintln!("Audio capture stopped");
        Ok(())
    }
}
