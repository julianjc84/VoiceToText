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
    // Try wtype first (wlroots compositors), then ydotool (KDE/GNOME/etc)
    let wtype_result = Command::new("wtype")
        .arg("--")
        .arg(text)
        .stderr(std::process::Stdio::null())
        .status();

    match wtype_result {
        Ok(status) if status.success() => return Ok(()),
        _ => {}
    }

    // Fallback to ydotool (needs ydotoold system service running)
    let result = Command::new("ydotool")
        .args(["type", "--", text])
        .env("YDOTOOL_SOCKET", "/run/ydotoold/socket")
        .status();
    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("ydotool exited with status {}", status).into()),
        Err(e) => Err(format!("Failed to type text: install wtype or ydotool ({})", e).into()),
    }
}

/// Copy text to the system clipboard.
/// On Wayland, prefers CLI tools (wl-copy) which keep contents alive in the
/// background. arboard's clipboard object is short-lived and gets dropped before
/// clipboard managers can read it.
pub fn copy_to_clipboard(text: &str) {
    if text.is_empty() {
        return;
    }

    // On Wayland, prefer wl-copy which forks into the background and keeps
    // clipboard contents alive until another copy replaces them.
    // arboard's clipboard object is short-lived and gets dropped before
    // clipboard managers can read it on Wayland.
    if matches!(DisplayServer::detect(), DisplayServer::Wayland) {
        let result = Command::new("wl-copy")
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
            eprintln!("Copied to clipboard ({} chars)", text.len());
            return;
        }
    }

    // arboard (works on X11; Wayland fallback)
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if clipboard.set_text(text).is_ok() {
            eprintln!("Copied to clipboard ({} chars)", text.len());
            return;
        }
    }

    // Last resort
    let result = Command::new("xclip")
        .args(["-selection", "clipboard"])
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
        eprintln!("Copied to clipboard ({} chars)", text.len());
        return;
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
