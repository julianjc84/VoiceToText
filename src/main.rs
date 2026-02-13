mod audio;
mod config;
mod dbus_service;
mod hotkey;
mod mic_mute;
mod output;
mod settings;
mod transcribe;
mod transcript;
mod tray;
mod vad;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{self, Receiver, Sender};

use config::{ActiveBackend, AppCommand, Config, DisplayServer, RecordingState};
use hotkey::HotkeyCommand;

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
use transcribe::{TranscribeCommand, TranscribeResult};
use tray::TrayUpdate;
use vad::VadCommand;

/// Central coordinator loop — receives commands and transcription results, manages recording state.
///
/// ## Text output modes
///
/// - **Toggle mode**: Text is typed live as each VAD segment is transcribed. No modifier
///   key is held during recording, so `xdotool --clearmodifiers` has no side effects.
///
/// - **Push-to-talk + evdev backend**: Text is typed live. evdev reads physical key state
///   from the kernel, immune to xdotool's X11-level key manipulation.
///
/// - **Push-to-talk + global_hotkey backend (fallback)**: Text is buffered in `session_text`
///   and typed all at once on release. `xdotool --clearmodifiers` causes false release
///   events at the X11 level that can't be distinguished from real releases.
fn coordinator_loop(
    cmd_rx: Receiver<AppCommand>,
    text_rx: Receiver<TranscribeResult>,
    raw_tx: Sender<Vec<f32>>,
    tray_tx: Sender<TrayUpdate>,
    ctrl_tx: Sender<TranscribeCommand>,
    vad_cmd_tx: Sender<VadCommand>,
    hotkey_cmd_tx: Sender<HotkeyCommand>,
    display_server: DisplayServer,
    typing_ended_at: Arc<AtomicU64>,
) {
    let audio = match audio::AudioCapture::new(raw_tx) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("FATAL: Failed to initialize audio capture: {}", e);
            return;
        }
    };

    let mut state = RecordingState::Idle;
    let mut session_text: Vec<String> = Vec::new();
    let mut session_process_ms: u64 = 0;
    let mut app_config = Config::load();
    let mut active_backend = ActiveBackend::GlobalHotkey;
    let mut mic_muted = false;
    let mut last_mute_notify = Instant::now() - Duration::from_secs(10);

    eprintln!("Coordinator ready");

    loop {
        crossbeam_channel::select! {
            recv(cmd_rx) -> cmd => {
                match cmd {
                    Ok(AppCommand::ToggleRecording) => {
                        eprintln!("Coord: ToggleRecording (state={:?})", state);
                        if state == RecordingState::Idle {
                            if mic_muted {
                                if last_mute_notify.elapsed() >= Duration::from_secs(3) {
                                    mic_mute::send_notification("Microphone is muted", "Unmute your mic to start recording");
                                    last_mute_notify = Instant::now();
                                }
                            } else {
                                start_recording(&audio, &mut state, RecordingState::AlwaysListen, &mut session_text, &mut session_process_ms, &tray_tx);
                            }
                        } else {
                            stop_recording(
                                &audio, &mut state, &mut session_text, &mut session_process_ms,
                                &tray_tx, &text_rx, &vad_cmd_tx,
                                display_server, &app_config, active_backend,
                            );
                        }
                    }
                    Ok(AppCommand::ToggleAlwaysListen) => {
                        eprintln!("Coord: ToggleAlwaysListen (state={:?})", state);
                        match state {
                            RecordingState::Idle => {
                                if mic_muted {
                                    if last_mute_notify.elapsed() >= Duration::from_secs(3) {
                                        mic_mute::send_notification("Microphone is muted", "Unmute your mic to start recording");
                                        last_mute_notify = Instant::now();
                                    }
                                } else {
                                    start_recording(&audio, &mut state, RecordingState::AlwaysListen, &mut session_text, &mut session_process_ms, &tray_tx);
                                }
                            }
                            RecordingState::AlwaysListen => {
                                stop_recording(
                                    &audio, &mut state, &mut session_text, &mut session_process_ms,
                                    &tray_tx, &text_rx, &vad_cmd_tx,
                                    display_server, &app_config, active_backend,
                                );
                            }
                            RecordingState::PushToTalk => {
                                // Don't interrupt push-to-talk
                            }
                        }
                    }
                    Ok(AppCommand::StartRecording) => {
                        if state == RecordingState::Idle {
                            if mic_muted {
                                if last_mute_notify.elapsed() >= Duration::from_secs(3) {
                                    mic_mute::send_notification("Microphone is muted", "Unmute your mic to start recording");
                                    last_mute_notify = Instant::now();
                                }
                            } else {
                                eprintln!("Coord: StartRecording (PushToTalk)");
                                start_recording(&audio, &mut state, RecordingState::PushToTalk, &mut session_text, &mut session_process_ms, &tray_tx);
                            }
                        }
                        // Ignored if already recording (AlwaysListen or PushToTalk)
                    }
                    Ok(AppCommand::StopRecording) => {
                        if state == RecordingState::PushToTalk {
                            eprintln!("Coord: StopRecording");
                            stop_recording(
                                &audio, &mut state, &mut session_text, &mut session_process_ms,
                                &tray_tx, &text_rx, &vad_cmd_tx,
                                display_server, &app_config, active_backend,
                            );
                        }
                    }
                    Ok(AppCommand::OpenSettings) => {
                        let _ = tray_tx.send(TrayUpdate::OpenSettings);
                    }
                    Ok(AppCommand::OpenTranscripts) => {
                        let _ = tray_tx.send(TrayUpdate::OpenTranscripts);
                    }
                    Ok(AppCommand::ReloadConfig) => {
                        let old_model = app_config.model.clone();
                        app_config = Config::load();
                        eprintln!("Config reloaded: model={}, clipboard={}, use_vad={}, mode={:?}, shortcut={}, max_transcripts={}",
                            app_config.model, app_config.clipboard_auto_copy,
                            app_config.use_vad, app_config.recording_mode, app_config.shortcut,
                            app_config.max_transcripts);
                        if app_config.model != old_model {
                            let _ = ctrl_tx.send(TranscribeCommand::ReloadModel(app_config.model.clone()));
                        }
                        // Tell VAD to try loading its model (in case it was just downloaded)
                        let _ = vad_cmd_tx.send(VadCommand::ReloadModel);
                        // Update segmentation mode + chunk duration
                        let _ = vad_cmd_tx.send(VadCommand::ReloadConfig);
                        // Update hotkey thread (PTT key/enabled)
                        let _ = hotkey_cmd_tx.send(HotkeyCommand::ReloadConfig);
                    }
                    Ok(AppCommand::HotkeyBackendResolved(backend)) => {
                        active_backend = backend;
                        let _ = tray_tx.send(TrayUpdate::BackendInfo(backend));
                        match backend {
                            ActiveBackend::Evdev => eprintln!("Hotkey backend: evdev"),
                            ActiveBackend::GlobalHotkey => eprintln!("Hotkey backend: global_hotkey (fallback)"),
                        }
                    }
                    Ok(AppCommand::CopyTranscript(text)) => {
                        let _ = tray_tx.send(TrayUpdate::CopyToClipboard(text));
                    }
                    Ok(AppCommand::MicMuteChanged(muted)) => {
                        mic_muted = muted;
                        let _ = tray_tx.send(TrayUpdate::MicMuted(muted));
                    }
                    Ok(AppCommand::Quit) => {
                        if state.is_recording() {
                            stop_recording(
                                &audio, &mut state, &mut session_text, &mut session_process_ms,
                                &tray_tx, &text_rx, &vad_cmd_tx,
                                display_server, &app_config, active_backend,
                            );
                        }
                        eprintln!("Quit command received");
                        let _ = tray_tx.send(TrayUpdate::Quit);
                        break;
                    }
                    Err(_) => break,
                }
            }
            recv(text_rx) -> result => {
                if let Ok(result) = result {
                    if state.is_recording() && !result.text.is_empty() {
                        session_text.push(result.text.clone());
                        session_process_ms += result.process_time_ms;

                        let should_buffer = state == RecordingState::PushToTalk
                            && active_backend == ActiveBackend::GlobalHotkey;

                        if should_buffer {
                            // PTT + global_hotkey: buffer text, type all at once on release.
                            // xdotool --clearmodifiers would cause false release events.
                            eprintln!("Live buffered ({}ms): {}", result.process_time_ms, result.text);
                        } else {
                            // Toggle mode or PTT + evdev: type immediately
                            eprintln!("Live output ({}ms): {}", result.process_time_ms, result.text);
                            output::type_text(&result.text, display_server);
                            typing_ended_at.store(epoch_ms(), Ordering::SeqCst);
                        }

                        // In AlwaysListen mode, save each segment as its own transcript
                        if state == RecordingState::AlwaysListen {
                            transcript::save_transcript(&result.text, app_config.max_transcripts, result.process_time_ms);
                            if app_config.clipboard_auto_copy {
                                output::copy_to_clipboard(&result.text);
                            }
                            let _ = tray_tx.send(TrayUpdate::RefreshTranscripts);
                        }
                    }
                }
            }
        }
    }
}

fn start_recording(
    audio: &audio::AudioCapture,
    state: &mut RecordingState,
    target_state: RecordingState,
    session_text: &mut Vec<String>,
    session_process_ms: &mut u64,
    tray_tx: &Sender<TrayUpdate>,
) {
    session_text.clear();
    *session_process_ms = 0;
    if let Err(e) = audio.start() {
        eprintln!("Failed to start recording: {}", e);
        return;
    }
    *state = target_state;
    let _ = tray_tx.send(TrayUpdate::State(target_state));
    eprintln!("=== Recording started ({:?}) ===", target_state);
}

/// Stop recording: pause audio, flush the VAD/transcription pipeline, output text.
///
/// Text output during drain depends on whether live typing was active:
/// - **Live typing** (toggle mode, or PTT + evdev): drain segments are typed immediately.
/// - **Buffered** (PTT + global_hotkey fallback): all text typed at once after drain.
fn stop_recording(
    audio: &audio::AudioCapture,
    state: &mut RecordingState,
    session_text: &mut Vec<String>,
    session_process_ms: &mut u64,
    tray_tx: &Sender<TrayUpdate>,
    text_rx: &Receiver<TranscribeResult>,
    vad_cmd_tx: &Sender<VadCommand>,
    display_server: DisplayServer,
    app_config: &Config,
    active_backend: ActiveBackend,
) {
    let prev_state = *state;
    if let Err(e) = audio.stop() {
        eprintln!("Failed to stop recording: {}", e);
    }
    let _ = vad_cmd_tx.send(VadCommand::Flush);
    *state = RecordingState::Idle;

    let was_buffered = prev_state == RecordingState::PushToTalk
        && active_backend == ActiveBackend::GlobalHotkey;

    let was_always_listen = prev_state == RecordingState::AlwaysListen;

    // Drain remaining transcriptions (wait for pipeline flush sentinel)
    loop {
        match text_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) if result.text.is_empty() => {
                // Sentinel received — pipeline fully drained
                break;
            }
            Ok(result) => {
                eprintln!("Final drain ({}ms): {}", result.process_time_ms, result.text);
                session_text.push(result.text.clone());
                *session_process_ms += result.process_time_ms;
                // Type drain segments immediately if we were typing live
                if !was_buffered {
                    output::type_text(&result.text, display_server);
                }
                // Save drain segments individually for AlwaysListen
                if was_always_listen {
                    transcript::save_transcript(&result.text, app_config.max_transcripts, result.process_time_ms);
                    let _ = tray_tx.send(TrayUpdate::RefreshTranscripts);
                }
            }
            Err(_) => {
                eprintln!("WARNING: drain timeout — transcription may have been lost");
                break;
            }
        }
    }

    if was_always_listen {
        // Segments were already saved individually — just log the full session
        let full = session_text.join(" ");
        if !full.is_empty() {
            eprintln!("Session text: {}", full);
        }
    } else {
        let full = session_text.join(" ");
        if !full.is_empty() {
            // If we were buffering, type everything at once now
            if was_buffered {
                output::type_text(&full, display_server);
            }
            transcript::save_transcript(&full, app_config.max_transcripts, *session_process_ms);
            if app_config.clipboard_auto_copy {
                output::copy_to_clipboard(&full);
            }
            let _ = tray_tx.send(TrayUpdate::RefreshTranscripts);
            eprintln!("Session text: {}", full);
        }
    }

    let _ = tray_tx.send(TrayUpdate::State(RecordingState::Idle));
    eprintln!("=== Recording stopped ===");
}

fn print_usage() {
    eprintln!("Usage: voice-to-text [command]");
    eprintln!();
    eprintln!("Commands (send to running daemon):");
    eprintln!("  toggle  Toggle recording on/off");
    eprintln!("  quit    Quit the daemon");
    eprintln!();
    eprintln!("No command starts the daemon.");
    eprintln!();
    eprintln!("Keyboard shortcut setup:");
    eprintln!("  Go to Settings → Keyboard Shortcuts and add:");
    eprintln!("    voice-to-text toggle   → bind to your preferred key");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Client mode: send command to running daemon
    if args.len() > 1 {
        let method = match args[1].as_str() {
            "toggle" => "Toggle",
            "quit" => "Quit",
            "--help" | "-h" | "help" => {
                print_usage();
                return;
            }
            other => {
                eprintln!("Unknown command: {}", other);
                print_usage();
                std::process::exit(1);
            }
        };

        match dbus_service::send_command(method) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Failed (is voice-to-text running?): {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // --- Daemon mode ---

    // If another instance is running, tell it to quit first
    if dbus_service::send_command("Quit").is_ok() {
        eprintln!("Stopped existing instance");
        std::thread::sleep(Duration::from_millis(500));
    }

    let display_server = DisplayServer::detect();
    eprintln!("Voice-to-Text starting");
    eprintln!("Display server: {}", display_server);

    // Check model availability (downloaded via Settings)
    let model_path = config::model_path();
    let vad_model_path = config::vad_model_path();

    if model_path.exists() {
        eprintln!("Whisper model ready");
    } else {
        eprintln!("Whisper model not found — download it in Settings");
    }

    if vad_model_path.exists() {
        eprintln!("VAD model ready");
    } else {
        eprintln!("VAD model not found — download it in Settings");
    }

    // Initialize GTK (required for tray icon on Linux)
    gtk::init().expect("Failed to initialize GTK");

    // Set default window icon for all GTK windows (titlebar/taskbar)
    {
        static APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");
        let cursor = std::io::Cursor::new(APP_ICON_PNG);
        if let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_read(cursor) {
            gtk::Window::set_default_icon(&pixbuf);
        }
    }

    // Create channels
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<AppCommand>();
    let (raw_tx, raw_rx) = crossbeam_channel::bounded::<Vec<f32>>(32);
    let (chunk_tx, chunk_rx) = crossbeam_channel::bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = crossbeam_channel::unbounded::<TranscribeResult>();
    let (tray_tx, tray_rx) = crossbeam_channel::unbounded::<TrayUpdate>();
    let (ctrl_tx, ctrl_rx) = crossbeam_channel::unbounded::<TranscribeCommand>();
    let (vad_cmd_tx, vad_cmd_rx) = crossbeam_channel::unbounded::<VadCommand>();
    let (hotkey_cmd_tx, hotkey_cmd_rx) = crossbeam_channel::unbounded::<HotkeyCommand>();

    // Start D-Bus service (must keep _conn alive)
    // If this fails, another instance is likely already running.
    let _dbus_conn = match dbus_service::start_server(cmd_tx.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: Failed to start D-Bus service: {}", e);
            eprintln!("Another instance of voice-to-text is probably already running.");
            eprintln!("Use 'voice-to-text quit' to stop it first.");
            std::process::exit(1);
        }
    };

    // Create tray icon (must be on main/GTK thread)
    let tray = tray::create_tray(cmd_tx.clone());

    // Spawn VAD thread (between audio capture and transcription)
    std::thread::Builder::new()
        .name("vad".into())
        .spawn(move || {
            vad::vad_thread(raw_rx, chunk_tx, vad_cmd_rx);
        })
        .expect("Failed to spawn VAD thread");

    // Spawn transcription thread
    let model_path_str = model_path.to_string_lossy().to_string();
    std::thread::Builder::new()
        .name("transcription".into())
        .spawn(move || {
            transcribe::transcription_loop(&model_path_str, chunk_rx, text_tx, ctrl_rx);
        })
        .expect("Failed to spawn transcription thread");

    // Shared timestamp: suppresses hotkey release events shortly after xdotool types text
    let typing_ended_at = Arc::new(AtomicU64::new(0));

    // Spawn hotkey thread (push-to-talk via evdev)
    let cmd_tx_hotkey = cmd_tx.clone();
    let typing_ended_hotkey = typing_ended_at.clone();
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            hotkey::hotkey_thread(cmd_tx_hotkey, hotkey_cmd_rx, typing_ended_hotkey);
        })
        .expect("Failed to spawn hotkey thread");

    // Spawn mic mute detection thread
    let cmd_tx_mute = cmd_tx.clone();
    std::thread::Builder::new()
        .name("mic-mute".into())
        .spawn(move || {
            mic_mute::mic_mute_thread(cmd_tx_mute);
        })
        .expect("Failed to spawn mic mute thread");

    // Spawn coordinator thread
    std::thread::Builder::new()
        .name("coordinator".into())
        .spawn(move || {
            coordinator_loop(cmd_rx, text_rx, raw_tx, tray_tx, ctrl_tx, vad_cmd_tx, hotkey_cmd_tx, display_server, typing_ended_at);
        })
        .expect("Failed to spawn coordinator thread");

    // GTK main loop: poll for tray updates (state changes + quit)
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        while let Ok(update) = tray_rx.try_recv() {
            tray::apply_update(&tray, update);
        }
        gtk::glib::ControlFlow::Continue
    });

    eprintln!("Voice-to-Text ready.");
    eprintln!("  Tray menu: right-click icon for Start/Stop/Quit");
    eprintln!("  CLI:       voice-to-text toggle  (bind this to a keyboard shortcut)");

    gtk::main();
}
