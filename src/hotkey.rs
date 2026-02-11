use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::config::{ActiveBackend, AppCommand, Config, RecordingMode};

/// How long to wait after a release before sending StopRecording.
/// If a new press arrives within this window, the release was auto-repeat and is ignored.
const PTT_RELEASE_DEBOUNCE: Duration = Duration::from_millis(50);

/// Grace period after typing ends during which release events are ignored.
/// Covers the case where xdotool --clearmodifiers releases modifier keys and the
/// event is queued but not processed until after typing finishes.
const TYPING_GRACE_MS: u64 = 200;

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug)]
pub enum HotkeyCommand {
    ReloadConfig,
}

fn parse_and_register(manager: &GlobalHotKeyManager, shortcut: &str) -> Result<HotKey, String> {
    let hotkey: HotKey = shortcut
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut, e))?;
    manager
        .register(hotkey)
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut, e))?;
    Ok(hotkey)
}

/// Map a shortcut string (e.g. "ctrl+space") to a set of evdev key codes.
/// Returns None if any part is unrecognized.
fn shortcut_to_evdev_keys(shortcut: &str) -> Option<HashSet<evdev::Key>> {
    let mut keys = HashSet::new();
    for part in shortcut.to_lowercase().split('+') {
        let key = match part.trim() {
            // Modifiers
            "ctrl" => evdev::Key::KEY_LEFTCTRL,
            "alt" => evdev::Key::KEY_LEFTALT,
            "super" => evdev::Key::KEY_LEFTMETA,
            "shift" => evdev::Key::KEY_LEFTSHIFT,
            // Letters
            "a" => evdev::Key::KEY_A,
            "b" => evdev::Key::KEY_B,
            "c" => evdev::Key::KEY_C,
            "d" => evdev::Key::KEY_D,
            "e" => evdev::Key::KEY_E,
            "f" => evdev::Key::KEY_F,
            "g" => evdev::Key::KEY_G,
            "h" => evdev::Key::KEY_H,
            "i" => evdev::Key::KEY_I,
            "j" => evdev::Key::KEY_J,
            "k" => evdev::Key::KEY_K,
            "l" => evdev::Key::KEY_L,
            "m" => evdev::Key::KEY_M,
            "n" => evdev::Key::KEY_N,
            "o" => evdev::Key::KEY_O,
            "p" => evdev::Key::KEY_P,
            "q" => evdev::Key::KEY_Q,
            "r" => evdev::Key::KEY_R,
            "s" => evdev::Key::KEY_S,
            "t" => evdev::Key::KEY_T,
            "u" => evdev::Key::KEY_U,
            "v" => evdev::Key::KEY_V,
            "w" => evdev::Key::KEY_W,
            "x" => evdev::Key::KEY_X,
            "y" => evdev::Key::KEY_Y,
            "z" => evdev::Key::KEY_Z,
            // Digits
            "0" => evdev::Key::KEY_0,
            "1" => evdev::Key::KEY_1,
            "2" => evdev::Key::KEY_2,
            "3" => evdev::Key::KEY_3,
            "4" => evdev::Key::KEY_4,
            "5" => evdev::Key::KEY_5,
            "6" => evdev::Key::KEY_6,
            "7" => evdev::Key::KEY_7,
            "8" => evdev::Key::KEY_8,
            "9" => evdev::Key::KEY_9,
            // Function keys
            "f1" => evdev::Key::KEY_F1,
            "f2" => evdev::Key::KEY_F2,
            "f3" => evdev::Key::KEY_F3,
            "f4" => evdev::Key::KEY_F4,
            "f5" => evdev::Key::KEY_F5,
            "f6" => evdev::Key::KEY_F6,
            "f7" => evdev::Key::KEY_F7,
            "f8" => evdev::Key::KEY_F8,
            "f9" => evdev::Key::KEY_F9,
            "f10" => evdev::Key::KEY_F10,
            "f11" => evdev::Key::KEY_F11,
            "f12" => evdev::Key::KEY_F12,
            // Navigation
            "enter" => evdev::Key::KEY_ENTER,
            "escape" => evdev::Key::KEY_ESC,
            "tab" => evdev::Key::KEY_TAB,
            "backspace" => evdev::Key::KEY_BACKSPACE,
            "delete" => evdev::Key::KEY_DELETE,
            "insert" => evdev::Key::KEY_INSERT,
            "home" => evdev::Key::KEY_HOME,
            "end" => evdev::Key::KEY_END,
            "pageup" => evdev::Key::KEY_PAGEUP,
            "pagedown" => evdev::Key::KEY_PAGEDOWN,
            // Arrows
            "up" => evdev::Key::KEY_UP,
            "down" => evdev::Key::KEY_DOWN,
            "left" => evdev::Key::KEY_LEFT,
            "right" => evdev::Key::KEY_RIGHT,
            // Misc
            "space" => evdev::Key::KEY_SPACE,
            "capslock" => evdev::Key::KEY_CAPSLOCK,
            "numlock" => evdev::Key::KEY_NUMLOCK,
            "printscreen" => evdev::Key::KEY_SYSRQ,
            "scrolllock" | "scroll_lock" => evdev::Key::KEY_SCROLLLOCK,
            "pause" => evdev::Key::KEY_PAUSE,
            "minus" => evdev::Key::KEY_MINUS,
            "equal" => evdev::Key::KEY_EQUAL,
            "leftbracket" => evdev::Key::KEY_LEFTBRACE,
            "rightbracket" => evdev::Key::KEY_RIGHTBRACE,
            "backslash" => evdev::Key::KEY_BACKSLASH,
            "semicolon" => evdev::Key::KEY_SEMICOLON,
            "apostrophe" => evdev::Key::KEY_APOSTROPHE,
            "grave" => evdev::Key::KEY_GRAVE,
            "comma" => evdev::Key::KEY_COMMA,
            "period" => evdev::Key::KEY_DOT,
            "slash" => evdev::Key::KEY_SLASH,
            _ => return None,
        };
        keys.insert(key);
    }
    if keys.is_empty() {
        None
    } else {
        Some(keys)
    }
}

/// Normalize right-side modifiers to left-side equivalents, so the target set
/// only needs to contain left-side variants.
fn normalize_key(key: evdev::Key) -> evdev::Key {
    match key {
        evdev::Key::KEY_RIGHTCTRL => evdev::Key::KEY_LEFTCTRL,
        evdev::Key::KEY_RIGHTALT => evdev::Key::KEY_LEFTALT,
        evdev::Key::KEY_RIGHTSHIFT => evdev::Key::KEY_LEFTSHIFT,
        evdev::Key::KEY_RIGHTMETA => evdev::Key::KEY_LEFTMETA,
        other => other,
    }
}

/// Try to open evdev keyboard devices. Returns Some(devices) if at least one
/// keyboard device is accessible, None otherwise.
fn try_open_evdev_devices() -> Option<Vec<evdev::Device>> {
    let mut keyboards = Vec::new();

    for (_path, device) in evdev::enumerate() {
        // Filter to devices that support EV_KEY and have typical keyboard keys
        if let Some(supported) = device.supported_keys() {
            if supported.contains(evdev::Key::KEY_SPACE) {
                keyboards.push(device);
            }
        }
    }

    if keyboards.is_empty() {
        eprintln!("evdev: no keyboard devices found");
        return None;
    }

    // Verify we can actually read from at least one device
    let mut accessible = Vec::new();
    for mut dev in keyboards {
        let dev_name = dev.name().unwrap_or("unknown").to_string();
        // Try a non-blocking fetch to check permissions.
        // We must drop the result before moving dev.
        let result = dev.fetch_events();
        let ok = match &result {
            Ok(_) => true,
            Err(e) if e.raw_os_error() == Some(11) => {
                // EAGAIN — device is accessible, just no events pending
                true
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    eprintln!(
                        "evdev: permission denied for {} — add user to 'input' group",
                        dev_name
                    );
                }
                false
            }
        };
        drop(result);
        if ok {
            accessible.push(dev);
        }
    }

    if accessible.is_empty() {
        eprintln!("evdev: no accessible keyboard devices (permission denied)");
        eprintln!("evdev: run 'sudo usermod -aG input $USER' and log out/in");
        None
    } else {
        eprintln!(
            "evdev: opened {} keyboard device(s)",
            accessible.len()
        );
        Some(accessible)
    }
}

/// Hotkey monitoring thread — tries evdev first, falls back to global_hotkey.
///
/// evdev reads physical key state from /dev/input/event*, which is immune to
/// xdotool's X11-level key manipulation. With evdev active, PTT can type text
/// live as each segment is transcribed.
pub fn hotkey_thread(
    cmd_tx: Sender<AppCommand>,
    hotkey_rx: Receiver<HotkeyCommand>,
    typing_ended_at: Arc<AtomicU64>,
) {
    if let Some(devices) = try_open_evdev_devices() {
        let _ = cmd_tx.send(AppCommand::HotkeyBackendResolved(ActiveBackend::Evdev));
        evdev_loop(devices, cmd_tx, hotkey_rx);
    } else {
        let _ = cmd_tx.send(AppCommand::HotkeyBackendResolved(ActiveBackend::GlobalHotkey));
        global_hotkey_loop(cmd_tx, hotkey_rx, typing_ended_at);
    }
}

/// evdev-based hotkey loop — polls physical key state at 100Hz.
///
/// Passive monitoring only (no grab). Tracks pressed keys across all devices,
/// normalizing left/right modifier variants. Supports push-to-talk with simple
/// debounce (no typing grace period needed since evdev is immune to xdotool).
fn evdev_loop(
    mut devices: Vec<evdev::Device>,
    cmd_tx: Sender<AppCommand>,
    hotkey_rx: Receiver<HotkeyCommand>,
) {
    let cfg = Config::load();
    let mut mode = cfg.recording_mode;

    let mut target_keys = match shortcut_to_evdev_keys(&cfg.shortcut) {
        Some(k) => k,
        None => {
            eprintln!("evdev: failed to parse shortcut '{}', using ctrl+space", cfg.shortcut);
            shortcut_to_evdev_keys("ctrl+space").unwrap()
        }
    };

    let mut transcript_keys = match shortcut_to_evdev_keys(&cfg.transcript_shortcut) {
        Some(k) => k,
        None => {
            eprintln!("evdev: failed to parse transcript shortcut '{}', using ctrl+shift+t", cfg.transcript_shortcut);
            shortcut_to_evdev_keys("ctrl+shift+t").unwrap()
        }
    };

    let mut listen_keys = match shortcut_to_evdev_keys(&cfg.always_listen_shortcut) {
        Some(k) => k,
        None => {
            eprintln!("evdev: failed to parse always-listen shortcut '{}', using ctrl+shift+l", cfg.always_listen_shortcut);
            shortcut_to_evdev_keys("ctrl+shift+l").unwrap()
        }
    };

    let mut pressed_keys: HashSet<evdev::Key> = HashSet::new();
    let mut was_matched = false;
    let mut transcript_was_matched = false;
    let mut listen_was_matched = false;

    // Simple debounce for PTT release (same concept as global_hotkey version)
    let mut pending_release: Option<Instant> = None;

    eprintln!("evdev: hotkey thread ready (shortcut: {}, mode: {:?})", cfg.shortcut, mode);

    loop {
        // Check for config reload commands (non-blocking)
        match hotkey_rx.try_recv() {
            Ok(HotkeyCommand::ReloadConfig) => {
                let new_cfg = Config::load();
                mode = new_cfg.recording_mode;

                match shortcut_to_evdev_keys(&new_cfg.shortcut) {
                    Some(k) => {
                        target_keys = k;
                        let mode_label = match mode {
                            RecordingMode::PushToTalk => "push-to-talk",
                            RecordingMode::Toggle => "toggle",
                        };
                        eprintln!("evdev: updated {} ({})", new_cfg.shortcut, mode_label);
                    }
                    None => {
                        eprintln!("evdev: failed to parse shortcut '{}'", new_cfg.shortcut);
                    }
                }

                match shortcut_to_evdev_keys(&new_cfg.transcript_shortcut) {
                    Some(k) => {
                        transcript_keys = k;
                        eprintln!("evdev: updated transcript shortcut {}", new_cfg.transcript_shortcut);
                    }
                    None => {
                        eprintln!("evdev: failed to parse transcript shortcut '{}'", new_cfg.transcript_shortcut);
                    }
                }

                match shortcut_to_evdev_keys(&new_cfg.always_listen_shortcut) {
                    Some(k) => {
                        listen_keys = k;
                        eprintln!("evdev: updated always-listen shortcut {}", new_cfg.always_listen_shortcut);
                    }
                    None => {
                        eprintln!("evdev: failed to parse always-listen shortcut '{}'", new_cfg.always_listen_shortcut);
                    }
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        // Read events from all devices (non-blocking)
        for device in &mut devices {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if event.event_type() == evdev::EventType::KEY {
                            let key = evdev::Key(event.code());
                            let normalized = normalize_key(key);

                            match event.value() {
                                1 | 2 => {
                                    // Key down (1) or key hold/repeat (2)
                                    pressed_keys.insert(normalized);
                                }
                                0 => {
                                    // Key up
                                    pressed_keys.remove(&normalized);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(e) if e.raw_os_error() == Some(11) => {
                    // EAGAIN — no events pending, normal
                }
                Err(e) => {
                    eprintln!("evdev: read error: {}", e);
                }
            }
        }

        // Check transcript hotkey
        let transcript_matched = transcript_keys.iter().all(|k| pressed_keys.contains(k));
        if transcript_matched && !transcript_was_matched {
            let _ = cmd_tx.send(AppCommand::OpenTranscripts);
        }
        transcript_was_matched = transcript_matched;

        // Check always-listen hotkey
        let listen_matched = listen_keys.iter().all(|k| pressed_keys.contains(k));
        if listen_matched && !listen_was_matched {
            let _ = cmd_tx.send(AppCommand::ToggleAlwaysListen);
        }
        listen_was_matched = listen_matched;

        // Check recording hotkey
        let matched = target_keys.iter().all(|k| pressed_keys.contains(k));

        match mode {
            RecordingMode::PushToTalk => {
                if matched && !was_matched {
                    // Keys just pressed
                    if pending_release.is_some() {
                        eprintln!("evdev: PRESS (cancelled pending release)");
                    } else {
                        eprintln!("evdev: PRESS");
                    }
                    pending_release = None;
                    let _ = cmd_tx.send(AppCommand::StartRecording);
                } else if !matched && was_matched {
                    // Keys just released — start debounce
                    eprintln!("evdev: RELEASE (debounce {}ms)", PTT_RELEASE_DEBOUNCE.as_millis());
                    pending_release = Some(Instant::now() + PTT_RELEASE_DEBOUNCE);
                }
            }
            RecordingMode::Toggle => {
                if matched && !was_matched {
                    let _ = cmd_tx.send(AppCommand::ToggleRecording);
                }
            }
        }

        was_matched = matched;

        // Fire pending release if deadline has passed with no new press
        if let Some(deadline) = pending_release {
            if Instant::now() >= deadline {
                pending_release = None;
                eprintln!("evdev: pending release FIRED -> StopRecording");
                let _ = cmd_tx.send(AppCommand::StopRecording);
            }
        }

        // 100Hz polling
        std::thread::sleep(Duration::from_millis(10));
    }

    eprintln!("evdev: hotkey thread exiting");
}

/// global_hotkey-based hotkey loop (fallback when evdev is not available).
///
/// This is the original hotkey implementation. In PTT mode, text output is
/// buffered by the coordinator because xdotool --clearmodifiers causes false
/// release events at the X11 level.
fn global_hotkey_loop(
    cmd_tx: Sender<AppCommand>,
    hotkey_rx: Receiver<HotkeyCommand>,
    typing_ended_at: Arc<AtomicU64>,
) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Hotkey: failed to create manager: {}", e);
            eprintln!("Hotkey: keyboard shortcut will not be available");
            loop {
                match hotkey_rx.recv() {
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        }
    };

    let cfg = Config::load();
    let mut mode = cfg.recording_mode;
    let mut current_hotkey: Option<HotKey> = None;

    match parse_and_register(&manager, &cfg.shortcut) {
        Ok(hk) => {
            let mode_label = match mode {
                RecordingMode::PushToTalk => "push-to-talk",
                RecordingMode::Toggle => "toggle",
            };
            eprintln!("Hotkey: registered {} ({})", cfg.shortcut, mode_label);
            current_hotkey = Some(hk);
        }
        Err(e) => {
            eprintln!("Hotkey: {}", e);
        }
    }

    let mut current_transcript_hotkey: Option<HotKey> = match parse_and_register(&manager, &cfg.transcript_shortcut) {
        Ok(hk) => {
            eprintln!("Hotkey: registered {} (open transcripts)", cfg.transcript_shortcut);
            Some(hk)
        }
        Err(e) => {
            eprintln!("Hotkey: {}", e);
            None
        }
    };

    let mut current_listen_hotkey: Option<HotKey> = match parse_and_register(&manager, &cfg.always_listen_shortcut) {
        Ok(hk) => {
            eprintln!("Hotkey: registered {} (always listen)", cfg.always_listen_shortcut);
            Some(hk)
        }
        Err(e) => {
            eprintln!("Hotkey: {}", e);
            None
        }
    };

    let event_rx = GlobalHotKeyEvent::receiver();

    // Debounce state for push-to-talk release (suppresses OS key auto-repeat)
    let mut pending_release: Option<Instant> = None;

    eprintln!("Hotkey thread ready (global_hotkey fallback)");

    loop {
        // Check for config reload commands (non-blocking)
        match hotkey_rx.try_recv() {
            Ok(HotkeyCommand::ReloadConfig) => {
                let new_cfg = Config::load();

                // Unregister old recording hotkey if any
                if let Some(hk) = current_hotkey.take() {
                    let _ = manager.unregister(hk);
                }

                mode = new_cfg.recording_mode;
                match parse_and_register(&manager, &new_cfg.shortcut) {
                    Ok(hk) => {
                        let mode_label = match mode {
                            RecordingMode::PushToTalk => "push-to-talk",
                            RecordingMode::Toggle => "toggle",
                        };
                        eprintln!("Hotkey: updated {} ({})", new_cfg.shortcut, mode_label);
                        current_hotkey = Some(hk);
                    }
                    Err(e) => {
                        eprintln!("Hotkey: {}", e);
                    }
                }

                // Unregister old transcript hotkey if any, re-register new one
                if let Some(hk) = current_transcript_hotkey.take() {
                    let _ = manager.unregister(hk);
                }
                match parse_and_register(&manager, &new_cfg.transcript_shortcut) {
                    Ok(hk) => {
                        eprintln!("Hotkey: updated transcript shortcut {}", new_cfg.transcript_shortcut);
                        current_transcript_hotkey = Some(hk);
                    }
                    Err(e) => {
                        eprintln!("Hotkey: {}", e);
                    }
                }

                // Unregister old always-listen hotkey if any, re-register new one
                if let Some(hk) = current_listen_hotkey.take() {
                    let _ = manager.unregister(hk);
                }
                match parse_and_register(&manager, &new_cfg.always_listen_shortcut) {
                    Ok(hk) => {
                        eprintln!("Hotkey: updated always-listen shortcut {}", new_cfg.always_listen_shortcut);
                        current_listen_hotkey = Some(hk);
                    }
                    Err(e) => {
                        eprintln!("Hotkey: {}", e);
                    }
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        // Poll for hotkey events (non-blocking)
        while let Ok(event) = event_rx.try_recv() {
            // Transcript hotkey (reloadable)
            if let Some(ref thk) = current_transcript_hotkey {
                if event.id() == thk.id() && event.state() == HotKeyState::Pressed {
                    let _ = cmd_tx.send(AppCommand::OpenTranscripts);
                    continue;
                }
            }
            // Always-listen hotkey (reloadable)
            if let Some(ref lhk) = current_listen_hotkey {
                if event.id() == lhk.id() && event.state() == HotKeyState::Pressed {
                    let _ = cmd_tx.send(AppCommand::ToggleAlwaysListen);
                    continue;
                }
            }
            // Recording hotkey (reloadable)
            if let Some(ref hk) = current_hotkey {
                if event.id() == hk.id() {
                    match mode {
                        RecordingMode::PushToTalk => match event.state() {
                            HotKeyState::Pressed => {
                                if pending_release.is_some() {
                                    eprintln!("Hotkey: PRESS (cancelled pending release)");
                                } else {
                                    eprintln!("Hotkey: PRESS");
                                }
                                pending_release = None;
                                let _ = cmd_tx.send(AppCommand::StartRecording);
                            }
                            HotKeyState::Released => {
                                let since_typing = epoch_ms().saturating_sub(
                                    typing_ended_at.load(Ordering::SeqCst),
                                );
                                let deadline = if since_typing < TYPING_GRACE_MS {
                                    let remaining = Duration::from_millis(TYPING_GRACE_MS - since_typing);
                                    eprintln!("Hotkey: RELEASE (deferred {}ms, typing {}ms ago)",
                                        remaining.as_millis() + PTT_RELEASE_DEBOUNCE.as_millis() as u128,
                                        since_typing);
                                    Instant::now() + remaining + PTT_RELEASE_DEBOUNCE
                                } else {
                                    eprintln!("Hotkey: RELEASE (debounce {}ms, typing {}ms ago)",
                                        PTT_RELEASE_DEBOUNCE.as_millis(), since_typing);
                                    Instant::now() + PTT_RELEASE_DEBOUNCE
                                };
                                pending_release = Some(deadline);
                            }
                        },
                        RecordingMode::Toggle => {
                            if event.state() == HotKeyState::Pressed {
                                let _ = cmd_tx.send(AppCommand::ToggleRecording);
                            }
                        }
                    }
                }
            }
        }

        // Fire pending release if deadline has passed with no new press
        if let Some(deadline) = pending_release {
            if Instant::now() >= deadline {
                pending_release = None;
                eprintln!("Hotkey: pending release FIRED -> StopRecording");
                let _ = cmd_tx.send(AppCommand::StopRecording);
            }
        }

        // 100Hz polling — sufficient for responsive input
        std::thread::sleep(Duration::from_millis(10));
    }

    eprintln!("Hotkey thread exiting");
}
