use std::process::Command;

use crate::config::DisplayServer;

/// Type text into the currently focused window.
pub fn type_text(text: &str, display_server: DisplayServer) {
    if text.is_empty() {
        return;
    }

    let result = match display_server {
        DisplayServer::X11 => type_text_x11(text),
        DisplayServer::Wayland => type_text_wayland(text),
    };

    if let Err(e) = result {
        eprintln!("Failed to type text: {}", e);
    }
}

fn type_text_x11(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    // --clearmodifiers temporarily releases held modifier keys so characters type
    // correctly (otherwise Ctrl+Space held → every char becomes Ctrl+<char>).
    // IMPORTANT: This causes `global_hotkey` to see the modifier release as the
    // hotkey being released. In push-to-talk mode, the coordinator buffers text
    // and only calls type_text after the key is released to avoid this conflict.
    let status = Command::new("xdotool")
        .args(["type", "--clearmodifiers", "--delay", "0", "--", text])
        .status()?;
    if !status.success() {
        return Err(format!("xdotool exited with status {}", status).into());
    }
    Ok(())
}

fn type_text_wayland(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Try wtype first, fall back to ydotool
    let wtype_result = Command::new("wtype").arg("--").arg(text).status();

    match wtype_result {
        Ok(status) if status.success() => Ok(()),
        _ => {
            // Fallback to ydotool
            let status = Command::new("ydotool")
                .args(["type", "--", text])
                .status()?;
            if !status.success() {
                return Err(format!("ydotool exited with status {}", status).into());
            }
            Ok(())
        }
    }
}

/// Copy text to the system clipboard.
pub fn copy_to_clipboard(text: &str) {
    if text.is_empty() {
        return;
    }

    // Try arboard first (works on both X11 and Wayland)
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if clipboard.set_text(text).is_ok() {
            eprintln!("Copied to clipboard ({} chars)", text.len());
            return;
        }
    }

    // Fallback to CLI tools
    let cmds: &[&[&str]] = &[
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
    ];

    for cmd in cmds {
        let result = Command::new(cmd[0])
            .args(&cmd[1..])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(ref mut stdin) = child.stdin {
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait()
            });
        if result.is_ok() {
            eprintln!("Copied to clipboard via {} ({} chars)", cmd[0], text.len());
            return;
        }
    }

    eprintln!("WARNING: Could not copy to clipboard. Install wl-copy or xclip.");
}

/// Send a desktop notification via notify-send (non-blocking).
pub fn send_notification(summary: &str, body: &str, icon: &str) {
    let _ = Command::new("notify-send")
        .args([
            "--app-name=Voice to Text",
            &format!("--icon={}", icon),
            summary,
            body,
        ])
        .spawn();
}
