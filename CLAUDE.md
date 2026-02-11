# Voice to Text — Development Guide

## Build & Run

```sh
cargo build --release              # Build optimized binary
cargo run --release                # Build and run
```

No test suite yet. Verify changes by building and running the app manually.

## Architecture

Multi-threaded Rust app using crossbeam channels. Single coordinator thread routes all commands and results. GTK3 runs on the main thread (required by Linux tray icons).

### Source files

| File | Purpose |
|---|---|
| `src/main.rs` | Entry point, coordinator loop, recording state machine |
| `src/config.rs` | Config struct, constants, paths, shared enums (AppCommand, RecordingState) |
| `src/audio.rs` | Mic capture via cpal (16kHz mono) |
| `src/vad.rs` | Voice activity detection using TEN-VAD ONNX model |
| `src/transcribe.rs` | Whisper inference thread (whisper-rs/whisper.cpp) |
| `src/hotkey.rs` | Keyboard shortcuts — evdev (preferred) + global_hotkey (X11 fallback) |
| `src/tray.rs` | System tray icon, menu, GTK event handling |
| `src/settings.rs` | GTK settings window with shortcut recorders |
| `src/output.rs` | Text typing (xdotool/wtype/ydotool) and clipboard |
| `src/transcript.rs` | Transcript JSON storage (load/save/prune) |
| `src/dbus_service.rs` | D-Bus single-instance and remote control |

### Key patterns

- **State machine**: `RecordingState` enum — `Idle`, `PushToTalk`, `AlwaysListen`. Use `.is_recording()` to check if active.
- **Channels**: `AppCommand` enum routes all user actions through a single `cmd_tx`/`cmd_rx` pair.
- **Hotkey reload**: Settings changes send `AppCommand::ReloadConfig`, hotkey thread re-parses and re-registers shortcuts without restart.
- **Shared GTK state**: `Rc<RefCell<Option<T>>>` for GTK widgets shared across callbacks. `Arc<Mutex<T>>` for cross-thread data (tray menu text map).
- **Icon colorization**: Base grey PNG is programmatically recolored to blue/red at runtime — no separate icon assets needed.

### Recording flow

1. Hotkey press → `AppCommand::StartRecording` (PTT) or `AppCommand::ToggleAlwaysListen`
2. Coordinator calls `audio.start()`, sets state
3. Audio samples flow: `cpal` → raw_tx → VAD → chunk_tx → Whisper → text_tx → Coordinator
4. Coordinator types text and optionally saves transcript
5. On stop: flush VAD pipeline, drain remaining transcriptions, reset state

### Config

TOML at `~/.config/voice-to-text/config.toml`. All fields have serde defaults for backward compatibility. Add new fields with `#[serde(default = "default_fn")]`.

## Code style

- No external formatter configured — follow existing style
- `eprintln!` for logging (no logging crate)
- Prefer `crossbeam_channel::select!` for multi-channel waiting
- GTK callbacks use `clone!` macro or manual `Rc` cloning
