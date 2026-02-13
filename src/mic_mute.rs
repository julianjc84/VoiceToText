use std::process::Command;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::config::AppCommand;
use crate::output;

/// Query PulseAudio for the default source mute state.
/// Returns `Some(true)` if muted, `Some(false)` if unmuted, `None` if pactl unavailable.
pub fn check_muted() -> Option<bool> {
    let output = Command::new("pactl")
        .args(["get-source-mute", "@DEFAULT_SOURCE@"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("yes") {
        Some(true)
    } else if stdout.contains("no") {
        Some(false)
    } else {
        None
    }
}

/// Send a mic-muted notification (non-blocking).
pub fn send_notification(summary: &str, body: &str) {
    output::send_notification(summary, body, "microphone-sensitivity-muted");
}

/// Poll system mic mute state every second, sending `MicMuteChanged` on transitions.
/// Exits gracefully if `pactl` is unavailable.
pub fn mic_mute_thread(cmd_tx: Sender<AppCommand>) {
    // Check initial state — if pactl isn't available, disable this feature
    let mut prev = match check_muted() {
        Some(muted) => {
            let _ = cmd_tx.send(AppCommand::MicMuteChanged(muted));
            muted
        }
        None => {
            eprintln!("Mic mute detection: pactl not available, feature disabled");
            return;
        }
    };

    eprintln!("Mic mute detection: active (muted={})", prev);

    loop {
        thread::sleep(Duration::from_secs(1));

        match check_muted() {
            Some(muted) if muted != prev => {
                eprintln!("Mic mute changed: {}", if muted { "muted" } else { "unmuted" });
                let _ = cmd_tx.send(AppCommand::MicMuteChanged(muted));
                prev = muted;
            }
            Some(_) => {} // No change
            None => {}    // pactl failed this time, keep going
        }
    }
}
