use std::process::Command;

/// Parse the volume percentage from `pactl get-sink-volume` output.
/// Looks for a pattern like "/ 75% /" and returns the first percentage found.
fn parse_volume(output: &str) -> Option<u32> {
    for part in output.split('/') {
        let trimmed = part.trim();
        if let Some(pct_str) = trimmed.strip_suffix('%') {
            if let Ok(val) = pct_str.trim().parse::<u32>() {
                return Some(val);
            }
        }
    }
    None
}

/// Get the current default sink volume as a percentage (0–100+).
/// Returns None if pactl is unavailable or output can't be parsed.
pub fn get_volume() -> Option<u32> {
    let output = Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_volume(&stdout)
}

/// Set the default sink volume to the given percentage.
pub fn set_volume(percent: u32) {
    let _ = Command::new("pactl")
        .args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{}%", percent)])
        .status();
}

/// Lower system volume to `target_percent`, returning the previous volume.
/// Returns None if pactl is unavailable (feature degrades gracefully).
pub fn duck(target_percent: u32) -> Option<u32> {
    let saved = get_volume()?;
    set_volume(target_percent);
    eprintln!("Audio ducked: {}% → {}%", saved, target_percent);
    Some(saved)
}

/// Restore system volume to a previously saved level.
pub fn restore(saved_percent: u32) {
    set_volume(saved_percent);
    eprintln!("Audio restored: {}%", saved_percent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_volume() {
        // Typical PulseAudio output
        let output = "Volume: front-left: 49152 /  75% / -7.50 dB,   front-right: 49152 /  75% / -7.50 dB";
        assert_eq!(parse_volume(output), Some(75));

        // 100%
        let output = "Volume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB";
        assert_eq!(parse_volume(output), Some(100));

        // 0%
        let output = "Volume: front-left: 0 /   0% / -inf dB,   front-right: 0 /   0% / -inf dB";
        assert_eq!(parse_volume(output), Some(0));
    }
}
