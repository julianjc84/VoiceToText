use crossbeam_channel::Sender;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::config::{
    self, ActiveBackend, AppCommand, Config, RecordingMode, AVAILABLE_MODELS, APP_VERSION,
    CHUNK_DURATION_MAX, CHUNK_DURATION_MIN,
};
use crate::output;
use crate::transcript;

/// State for one shortcut recorder. Stored inside a shared `Rc<RefCell<Option<...>>>`
/// so that the single key-press handler knows which recorder (if any) is active.
struct RecorderState {
    button: gtk::Button,
    warning_label: gtk::Label,
    saved_shortcut: Rc<RefCell<String>>,
    on_save: Rc<dyn Fn(&str)>,
}

/// Map GDK key names to our internal format.
/// Returns None for unrecognized keys.
fn gdk_name_to_internal(name: &str) -> Option<String> {
    // Single lowercase letter
    if name.len() == 1 {
        let ch = name.chars().next().unwrap();
        if ch.is_ascii_lowercase() {
            return Some(name.to_string());
        }
        if ch.is_ascii_uppercase() {
            return Some(ch.to_ascii_lowercase().to_string());
        }
        if ch.is_ascii_digit() {
            return Some(name.to_string());
        }
    }

    let result = match name {
        // Navigation / editing
        "Return" => "enter",
        "Escape" => "escape",
        "Tab" => "tab",
        "BackSpace" => "backspace",
        "Delete" => "delete",
        "Insert" => "insert",
        "Home" => "home",
        "End" => "end",
        "Page_Up" => "pageup",
        "Page_Down" => "pagedown",
        // Arrows
        "Up" => "up",
        "Down" => "down",
        "Left" => "left",
        "Right" => "right",
        // Space
        "space" => "space",
        // Misc
        "Caps_Lock" => "capslock",
        "Num_Lock" => "numlock",
        "Scroll_Lock" => "scrolllock",
        "Print" | "Sys_Req" => "printscreen",
        "Pause" => "pause",
        // Punctuation
        "minus" => "minus",
        "equal" => "equal",
        "bracketleft" => "leftbracket",
        "bracketright" => "rightbracket",
        "backslash" => "backslash",
        "semicolon" => "semicolon",
        "apostrophe" => "apostrophe",
        "grave" => "grave",
        "comma" => "comma",
        "period" => "period",
        "slash" => "slash",
        _ => {
            // Function keys: "F1" -> "f1", etc.
            if name.starts_with('F') && name[1..].parse::<u32>().is_ok() {
                return Some(name.to_lowercase());
            }
            return None;
        }
    };
    Some(result.to_string())
}

fn is_modifier_keyval(keyval: gtk::gdk::keys::Key) -> bool {
    use gtk::gdk::keys::constants;
    matches!(
        keyval,
        constants::Control_L
            | constants::Control_R
            | constants::Alt_L
            | constants::Alt_R
            | constants::Shift_L
            | constants::Shift_R
            | constants::Super_L
            | constants::Super_R
            | constants::Meta_L
            | constants::Meta_R
            | constants::ISO_Level3_Shift
    )
}

/// Create a clickable path row: dim path text + "Open" button that opens the folder.
fn create_path_row(path: &std::path::Path) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);

    let path_str = path.to_string_lossy().to_string();
    let label = gtk::Label::new(Some(&path_str));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_hexpand(true);
    label.set_selectable(true);
    label.style_context().add_class("dim-label");

    let open_btn = gtk::Button::with_label("Open");
    open_btn.set_tooltip_text(Some("Open folder in file manager"));
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .unwrap_or(path)
            .to_path_buf()
    };
    open_btn.connect_clicked(move |_| {
        let _ = std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn();
    });

    row.pack_start(&label, true, true, 0);
    row.pack_start(&open_btn, false, false, 0);
    row
}

/// Create a Frame + inner Box with standard margins, returning both.
fn create_section(title: &str) -> (gtk::Frame, gtk::Box) {
    let frame = gtk::Frame::new(Some(title));
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
    inner.set_margin_start(8);
    inner.set_margin_end(8);
    inner.set_margin_top(8);
    inner.set_margin_bottom(8);
    frame.add(&inner);
    (frame, inner)
}

/// Build a shortcut recorder widget: a row with a label and button, plus warning
/// and note labels underneath. Only one recorder can be active at a time — controlled
/// via the shared `active_recorder`.
fn build_shortcut_recorder(
    label_text: &str,
    current_shortcut: &str,
    active_recorder: &Rc<RefCell<Option<RecorderState>>>,
    on_save: Rc<dyn Fn(&str)>,
) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 2);

    // Row: [Label] [Button]
    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(label_text));
    label.set_xalign(0.0);

    let button = gtk::Button::with_label(&config::display_shortcut(current_shortcut));
    button.set_size_request(200, -1);

    let saved_shortcut = Rc::new(RefCell::new(current_shortcut.to_string()));

    // Warning label (hidden by default)
    let warning_label = gtk::Label::new(None);
    warning_label.set_xalign(0.0);
    warning_label.set_line_wrap(true);
    warning_label.set_no_show_all(true);
    warning_label.set_visible(false);
    if config::is_dangerous_shortcut(current_shortcut) {
        warning_label.set_markup(&format!(
            "<span foreground=\"#cc0000\">Warning: {} conflicts with a common shortcut.</span>",
            gtk::glib::markup_escape_text(&config::display_shortcut(current_shortcut))
        ));
        warning_label.set_visible(true);
    }

    // Note label
    let note_label = gtk::Label::new(Some("This shortcut also reaches other applications."));
    note_label.set_xalign(0.0);
    note_label.set_line_wrap(true);
    note_label.style_context().add_class("dim-label");

    // Click handler: enter recording mode for this recorder
    let active_click = active_recorder.clone();
    let button_click = button.clone();
    let warning_click = warning_label.clone();
    let saved_click = saved_shortcut.clone();
    let on_save_click = on_save.clone();
    button.connect_clicked(move |_| {
        // If another recorder is active, ignore this click
        if active_click.borrow().is_some() {
            return;
        }
        button_click.set_label("Press shortcut...");
        button_click.style_context().add_class("dim-label");
        *active_click.borrow_mut() = Some(RecorderState {
            button: button_click.clone(),
            warning_label: warning_click.clone(),
            saved_shortcut: saved_click.clone(),
            on_save: on_save_click.clone(),
        });
    });

    key_row.pack_start(&label, false, false, 0);
    key_row.pack_start(&button, false, false, 0);
    container.pack_start(&key_row, false, false, 0);
    container.pack_start(&warning_label, false, false, 0);
    container.pack_start(&note_label, false, false, 0);

    container
}

enum DownloadMsg {
    Progress(f64),
    Done,
    Failed(String),
}

/// Spawn a background download and poll progress on the GTK thread.
/// `on_done` is called once on success; the caller handles post-download logic.
fn start_download_with_polling(
    url: String,
    target_path: std::path::PathBuf,
    status_btn: &gtk::Button,
    progress_bar: &gtk::ProgressBar,
    on_done: Box<dyn Fn() + 'static>,
) {
    status_btn.set_sensitive(false);
    status_btn.set_label("...");
    progress_bar.set_visible(true);
    progress_bar.set_fraction(0.0);

    let (dl_tx, dl_rx) = crossbeam_channel::unbounded::<DownloadMsg>();

    let progress_poll = progress_bar.clone();
    let status_btn_poll = status_btn.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        while let Ok(msg) = dl_rx.try_recv() {
            match msg {
                DownloadMsg::Progress(frac) => {
                    progress_poll.set_fraction(frac);
                }
                DownloadMsg::Done => {
                    status_btn_poll.set_label("\u{2713} Ready");
                    status_btn_poll.set_sensitive(false);
                    progress_poll.set_visible(false);
                    on_done();
                    return gtk::glib::ControlFlow::Break;
                }
                DownloadMsg::Failed(e) => {
                    eprintln!("Download failed: {}", e);
                    status_btn_poll.set_label("Retry");
                    status_btn_poll.set_sensitive(true);
                    progress_poll.set_visible(false);
                    return gtk::glib::ControlFlow::Break;
                }
            }
        }
        gtk::glib::ControlFlow::Continue
    });

    std::thread::spawn(move || {
        if let Some(parent) = target_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match download_model_with_progress(&url, &target_path, &dl_tx) {
            Ok(()) => {
                let _ = dl_tx.send(DownloadMsg::Done);
            }
            Err(e) => {
                let _ = dl_tx.send(DownloadMsg::Failed(e.to_string()));
            }
        }
    });
}

/// Load config, apply a mutation, save, and notify the coordinator to reload.
fn save_and_notify(cmd_tx: &Sender<AppCommand>, mutate: impl FnOnce(&mut Config)) {
    let mut cfg = Config::load();
    mutate(&mut cfg);
    if let Err(e) = cfg.save() {
        eprintln!("Failed to save config: {}", e);
    }
    let _ = cmd_tx.send(AppCommand::ReloadConfig);
}

/// Build the Settings page content. Returns (scrolled_window, chunk_controls, vad_controls, use_vad)
/// so the caller can fix visibility after show_all.
fn build_settings_page(
    cmd_tx: &Sender<AppCommand>,
    config: &Config,
    active_backend: &Rc<RefCell<Option<ActiveBackend>>>,
    window: &gtk::Window,
) -> (gtk::ScrolledWindow, gtk::Box, gtk::Box, bool) {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);

    // --- Shortcut section ---
    let (shortcut_frame, shortcut_box) = create_section("Shortcut");

    // Shared state: only one recorder can be active at a time
    let active_recorder: Rc<RefCell<Option<RecorderState>>> = Rc::new(RefCell::new(None));

    // Recording shortcut recorder
    let cmd_tx_rec = cmd_tx.clone();
    let recording_recorder = build_shortcut_recorder(
        "Recording:",
        &config.shortcut,
        &active_recorder,
        Rc::new(move |shortcut: &str| {
            let s = shortcut.to_string();
            save_and_notify(&cmd_tx_rec, |cfg| cfg.shortcut = s);
        }),
    );
    shortcut_box.pack_start(&recording_recorder, false, false, 0);

    // Always Listen shortcut recorder
    let cmd_tx_listen = cmd_tx.clone();
    let listen_recorder = build_shortcut_recorder(
        "Always Listen:",
        &config.always_listen_shortcut,
        &active_recorder,
        Rc::new(move |shortcut: &str| {
            let s = shortcut.to_string();
            save_and_notify(&cmd_tx_listen, |cfg| cfg.always_listen_shortcut = s);
        }),
    );
    shortcut_box.pack_start(&listen_recorder, false, false, 0);

    // Shared key-press handler for whichever recorder is active
    let active_kp = active_recorder.clone();
    let window_ref = window.clone();
    window.connect_key_press_event(move |_win, event| {
        use gtk::gdk::keys::constants as keys;
        use gtk::gdk::ModifierType;

        let active = active_kp.borrow();
        if active.is_none() {
            return gtk::glib::Propagation::Proceed;
        }
        drop(active);

        let keyval = event.keyval();

        // Escape cancels recording
        if keyval == keys::Escape {
            let recorder = active_kp.borrow_mut().take().unwrap();
            let prev = recorder.saved_shortcut.borrow().clone();
            recorder.button.set_label(&config::display_shortcut(&prev));
            recorder.button.style_context().remove_class("dim-label");
            return gtk::glib::Propagation::Stop;
        }

        // If it's just a modifier, show partial state
        if is_modifier_keyval(keyval) {
            let state = event.state();
            let mut parts = Vec::new();
            if state.contains(ModifierType::CONTROL_MASK) { parts.push("Ctrl"); }
            if state.contains(ModifierType::MOD1_MASK) { parts.push("Alt"); }
            if state.contains(ModifierType::SHIFT_MASK) { parts.push("Shift"); }
            if state.contains(ModifierType::SUPER_MASK) || state.contains(ModifierType::MOD4_MASK) {
                parts.push("Super");
            }
            if let Some(name_str) = keyval.name() {
                match name_str.as_str() {
                    "Control_L" | "Control_R" if !parts.contains(&"Ctrl") => parts.push("Ctrl"),
                    "Alt_L" | "Alt_R" | "ISO_Level3_Shift" if !parts.contains(&"Alt") => parts.push("Alt"),
                    "Shift_L" | "Shift_R" if !parts.contains(&"Shift") => parts.push("Shift"),
                    "Super_L" | "Super_R" | "Meta_L" | "Meta_R" if !parts.contains(&"Super") => parts.push("Super"),
                    _ => {}
                }
            }
            if !parts.is_empty() {
                let active = active_kp.borrow();
                if let Some(ref rec) = *active {
                    rec.button.set_label(&format!("{} + ...", parts.join(" + ")));
                }
            }
            return gtk::glib::Propagation::Stop;
        }

        // Non-modifier key pressed — resolve the physical key
        let display = window_ref.display();
        let keymap = match gtk::gdk::Keymap::for_display(&display) {
            Some(km) => km,
            None => return gtk::glib::Propagation::Stop,
        };

        let hw_keycode = event.hardware_keycode();
        let entries = keymap.entries_for_keycode(hw_keycode as u32);
        let base_name = if let Some((_keymap_key, first_kv)) = entries.first() {
            let k = gtk::gdk::keys::Key::from(*first_kv);
            k.name().map(|n| n.to_string())
        } else {
            keyval.name().map(|n| n.to_string())
        };

        let base_name = match base_name {
            Some(n) => n,
            None => return gtk::glib::Propagation::Stop,
        };

        let internal_key = match gdk_name_to_internal(&base_name) {
            Some(k) => k,
            None => {
                let active = active_kp.borrow();
                if let Some(ref rec) = *active {
                    rec.warning_label.set_markup(
                        "<span foreground=\"#cc0000\">Unrecognized key. Try another.</span>",
                    );
                    rec.warning_label.set_visible(true);
                }
                return gtk::glib::Propagation::Stop;
            }
        };

        // Build modifier list in fixed order
        let state = event.state();
        let mut mods = Vec::new();
        if state.contains(ModifierType::CONTROL_MASK) { mods.push("ctrl"); }
        if state.contains(ModifierType::MOD1_MASK) { mods.push("alt"); }
        if state.contains(ModifierType::SHIFT_MASK) { mods.push("shift"); }
        if state.contains(ModifierType::SUPER_MASK) || state.contains(ModifierType::MOD4_MASK) {
            mods.push("super");
        }

        mods.push(&internal_key);
        let shortcut = mods.join("+");

        // Validate
        match config::validate_shortcut(&shortcut) {
            Err(msg) => {
                let active = active_kp.borrow();
                if let Some(ref rec) = *active {
                    rec.warning_label.set_markup(&format!(
                        "<span foreground=\"#cc0000\">{}</span>",
                        gtk::glib::markup_escape_text(msg)
                    ));
                    rec.warning_label.set_visible(true);
                }
                return gtk::glib::Propagation::Stop;
            }
            Ok(()) => {}
        }

        // Valid — finalize: take the active recorder, update UI, save
        let recorder = active_kp.borrow_mut().take().unwrap();
        recorder.button.set_label(&config::display_shortcut(&shortcut));
        recorder.button.style_context().remove_class("dim-label");
        *recorder.saved_shortcut.borrow_mut() = shortcut.clone();

        if config::is_dangerous_shortcut(&shortcut) {
            recorder.warning_label.set_markup(&format!(
                "<span foreground=\"#cc0000\">Warning: {} conflicts with a common shortcut.</span>",
                gtk::glib::markup_escape_text(&config::display_shortcut(&shortcut))
            ));
            recorder.warning_label.set_visible(true);
        } else {
            recorder.warning_label.set_visible(false);
        }

        (recorder.on_save)(&shortcut);

        gtk::glib::Propagation::Stop
    });

    // Recording mode: Push to Talk / Toggle
    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let mode_label = gtk::Label::new(Some("Mode:"));
    mode_label.set_xalign(0.0);

    let radio_ptt = gtk::RadioButton::with_label("Push to talk");
    let radio_toggle = gtk::RadioButton::with_label_from_widget(&radio_ptt, "Toggle");

    match config.recording_mode {
        RecordingMode::PushToTalk => radio_ptt.set_active(true),
        RecordingMode::Toggle => radio_toggle.set_active(true),
    }

    mode_row.pack_start(&mode_label, false, false, 0);
    mode_row.pack_start(&radio_ptt, false, false, 0);
    mode_row.pack_start(&radio_toggle, false, false, 0);
    shortcut_box.pack_start(&mode_row, false, false, 0);

    let mode_hint = gtk::Label::new(None);
    mode_hint.set_xalign(0.0);
    mode_hint.set_line_wrap(true);
    mode_hint.style_context().add_class("dim-label");
    shortcut_box.pack_start(&mode_hint, false, false, 0);

    match config.recording_mode {
        RecordingMode::PushToTalk => {
            mode_hint.set_text("Hold to record, release to stop.");
        }
        RecordingMode::Toggle => {
            mode_hint.set_text("Press to start, press again to stop.");
        }
    }

    let cmd_tx_mode = cmd_tx.clone();
    let mode_hint_ref = mode_hint.clone();
    radio_ptt.connect_toggled(move |btn| {
        if btn.is_active() {
            mode_hint_ref.set_text("Hold to record, release to stop.");
            save_and_notify(&cmd_tx_mode, |cfg| {
                cfg.recording_mode = RecordingMode::PushToTalk;
            });
        }
    });

    let cmd_tx_mode2 = cmd_tx.clone();
    let mode_hint_ref2 = mode_hint;
    radio_toggle.connect_toggled(move |btn| {
        if btn.is_active() {
            mode_hint_ref2.set_text("Press to start, press again to stop.");
            save_and_notify(&cmd_tx_mode2, |cfg| {
                cfg.recording_mode = RecordingMode::Toggle;
            });
        }
    });

    // Backend status row
    let backend_row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    backend_row.set_margin_top(4);

    let backend_status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let backend_label = gtk::Label::new(None);
    backend_label.set_xalign(0.0);
    backend_label.set_hexpand(true);

    let backend_hint = gtk::Label::new(None);
    backend_hint.set_xalign(0.0);
    backend_hint.set_line_wrap(true);
    backend_hint.style_context().add_class("dim-label");

    let setup_btn = gtk::Button::with_label("Setup evdev");
    setup_btn.set_tooltip_text(Some("Add user to 'input' group for kernel-level key detection"));
    setup_btn.set_no_show_all(true);

    let setup_result_label = gtk::Label::new(None);
    setup_result_label.set_xalign(0.0);
    setup_result_label.set_line_wrap(true);
    setup_result_label.set_no_show_all(true);

    match *active_backend.borrow() {
        Some(ActiveBackend::Evdev) => {
            backend_label.set_text("\u{2713} Input: evdev (kernel-level)");
            backend_hint.set_text("Live text output during push-to-talk.");
            setup_btn.set_visible(false);
        }
        Some(ActiveBackend::GlobalHotkey) => {
            backend_label.set_text("Input: global_hotkey (X11-level)");
            backend_hint.set_text("Text output after key release in push-to-talk (fallback).");
            setup_btn.set_visible(true);
        }
        None => {
            backend_label.set_text("Input: detecting...");
            backend_hint.set_text("");
            setup_btn.set_visible(true);
        }
    }

    let setup_result_ref = setup_result_label.clone();
    setup_btn.connect_clicked(move |btn| {
        let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        match std::process::Command::new("pkexec")
            .args(["usermod", "-aG", "input", &user])
            .status()
        {
            Ok(status) if status.success() => {
                setup_result_ref.set_text("Added to input group. Log out and back in to activate.");
                setup_result_ref.set_visible(true);
                btn.set_sensitive(false);
                btn.set_label("\u{2713} Done");
            }
            Ok(_) => {
                setup_result_ref.set_text("Failed (authentication cancelled or denied).");
                setup_result_ref.set_visible(true);
            }
            Err(e) => {
                setup_result_ref.set_text(&format!("Error: {}", e));
                setup_result_ref.set_visible(true);
            }
        }
    });

    backend_status.pack_start(&backend_label, true, true, 0);
    backend_status.pack_start(&setup_btn, false, false, 0);
    backend_row.pack_start(&backend_status, false, false, 0);
    backend_row.pack_start(&backend_hint, false, false, 0);
    backend_row.pack_start(&setup_result_label, false, false, 0);
    shortcut_box.pack_start(&backend_row, false, false, 0);

    vbox.pack_start(&shortcut_frame, false, false, 0);

    // --- Clipboard section ---
    let (clip_frame, clip_box) = create_section("Clipboard");

    let clip_check = gtk::CheckButton::with_label("Auto-copy to clipboard on stop");
    clip_check.set_active(config.clipboard_auto_copy);

    let cmd_tx_clip = cmd_tx.clone();
    clip_check.connect_toggled(move |btn| {
        let active = btn.is_active();
        save_and_notify(&cmd_tx_clip, |cfg| cfg.clipboard_auto_copy = active);
    });

    clip_box.pack_start(&clip_check, false, false, 0);
    vbox.pack_start(&clip_frame, false, false, 0);

    // --- Segmentation section ---
    let (seg_frame, seg_box) = create_section("Segmentation");

    let radio_chunk = gtk::RadioButton::with_label("Fixed chunks");
    let radio_vad =
        gtk::RadioButton::with_label_from_widget(&radio_chunk, "VAD (speech detection)");

    if config.use_vad {
        radio_vad.set_active(true);
    } else {
        radio_chunk.set_active(true);
    }

    seg_box.pack_start(&radio_chunk, false, false, 0);

    // Chunk mode controls
    let chunk_controls = gtk::Box::new(gtk::Orientation::Vertical, 4);
    chunk_controls.set_margin_start(24);

    let duration_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let duration_label = gtk::Label::new(Some("Chunk duration:"));
    duration_label.set_xalign(0.0);

    let duration_spin = gtk::SpinButton::with_range(
        CHUNK_DURATION_MIN as f64,
        CHUNK_DURATION_MAX as f64,
        0.5,
    );
    duration_spin.set_value(config.chunk_duration_secs as f64);
    duration_spin.set_digits(1);

    let seconds_label = gtk::Label::new(Some("seconds"));

    duration_row.pack_start(&duration_label, false, false, 0);
    duration_row.pack_start(&duration_spin, false, false, 0);
    duration_row.pack_start(&seconds_label, false, false, 0);
    chunk_controls.pack_start(&duration_row, false, false, 0);

    let chunk_hint = gtk::Label::new(Some("Below 2s causes poor transcription"));
    chunk_hint.set_xalign(0.0);
    chunk_hint.style_context().add_class("dim-label");
    chunk_controls.pack_start(&chunk_hint, false, false, 0);

    seg_box.pack_start(&chunk_controls, false, false, 0);

    seg_box.pack_start(&radio_vad, false, false, 0);

    // VAD mode controls
    let vad_controls = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vad_controls.set_margin_start(24);

    let vad_hint = gtk::Label::new(Some("Automatically splits on speech pauses"));
    vad_hint.set_xalign(0.0);
    vad_hint.style_context().add_class("dim-label");
    vad_controls.pack_start(&vad_hint, false, false, 0);

    // VAD model download row
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let vad_path = config::vad_model_path();
        let vad_downloaded = vad_path.exists();

        let label = gtk::Label::new(Some("TEN VAD (315 KB)"));
        label.set_xalign(0.0);
        label.set_hexpand(true);

        let status_btn = gtk::Button::new();
        status_btn.set_size_request(80, -1);

        let progress = gtk::ProgressBar::new();
        progress.set_no_show_all(true);
        progress.set_visible(false);
        progress.set_hexpand(true);

        if vad_downloaded {
            status_btn.set_label("\u{2713} Ready");
            status_btn.set_sensitive(false);
        } else {
            status_btn.set_label("Download");

            let cmd_tx_vad = cmd_tx.clone();
            let status_btn_clone = status_btn.clone();
            let progress_clone = progress.clone();

            status_btn.connect_clicked(move |_btn| {
                let cmd_tx = cmd_tx_vad.clone();
                start_download_with_polling(
                    config::VAD_MODEL_URL.to_string(),
                    config::vad_model_path(),
                    &status_btn_clone,
                    &progress_clone,
                    Box::new(move || {
                        let _ = cmd_tx.send(AppCommand::ReloadConfig);
                    }),
                );
            });
        }

        row.pack_start(&label, true, true, 0);
        row.pack_start(&progress, true, true, 0);
        row.pack_start(&status_btn, false, false, 0);
        vad_controls.pack_start(&row, false, false, 0);

        let vad_path_row = create_path_row(&vad_path);
        vad_controls.pack_start(&vad_path_row, false, false, 0);
    }

    seg_box.pack_start(&vad_controls, false, false, 0);

    // Initial visibility
    chunk_controls.set_visible(!config.use_vad);
    chunk_controls.set_no_show_all(config.use_vad);
    vad_controls.set_visible(config.use_vad);
    vad_controls.set_no_show_all(!config.use_vad);

    let cmd_tx_duration = cmd_tx.clone();
    duration_spin.connect_value_changed(move |spin| {
        let val = spin.value() as f32;
        save_and_notify(&cmd_tx_duration, |cfg| cfg.chunk_duration_secs = val);
    });

    let cmd_tx_seg = cmd_tx.clone();
    let chunk_controls_ref = chunk_controls.clone();
    let vad_controls_ref = vad_controls.clone();
    radio_vad.connect_toggled(move |btn| {
        let vad_active = btn.is_active();
        chunk_controls_ref.set_no_show_all(vad_active);
        chunk_controls_ref.set_visible(!vad_active);
        vad_controls_ref.set_no_show_all(!vad_active);
        vad_controls_ref.set_visible(vad_active);

        if vad_active {
            vad_controls_ref.show_all();
        } else {
            chunk_controls_ref.show_all();
        }

        save_and_notify(&cmd_tx_seg, |cfg| cfg.use_vad = vad_active);
    });

    vbox.pack_start(&seg_frame, false, false, 0);

    // --- Whisper Model section ---
    let (model_frame, model_box) = create_section("Whisper Model");

    let mut first_radio: Option<gtk::RadioButton> = None;

    for model_info in AVAILABLE_MODELS {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let label_text = format!(
            "{} ({}) — {}",
            model_info.label, model_info.size, model_info.description
        );

        let radio = if let Some(ref first) = first_radio {
            gtk::RadioButton::with_label_from_widget(first, &label_text)
        } else {
            gtk::RadioButton::with_label(&label_text)
        };

        if first_radio.is_none() {
            first_radio = Some(radio.clone());
        }

        let is_selected = config.model == model_info.filename;
        radio.set_active(is_selected);

        let model_path = config::model_path_for(model_info.filename);
        let is_downloaded = model_path.exists();

        let status_btn = gtk::Button::new();
        status_btn.set_size_request(80, -1);

        if is_downloaded {
            status_btn.set_label("\u{2713} Ready");
            status_btn.set_sensitive(false);
        } else {
            status_btn.set_label("Download");
            radio.set_sensitive(false);
        }

        let progress = gtk::ProgressBar::new();
        progress.set_no_show_all(true);
        progress.set_visible(false);
        progress.set_hexpand(true);

        let filename = model_info.filename.to_string();
        let url = model_info.url.to_string();

        if !is_downloaded {
            let cmd_tx_model = cmd_tx.clone();
            let radio_clone = radio.clone();
            let status_btn_clone = status_btn.clone();
            let progress_clone = progress.clone();
            let filename_clone = filename.clone();

            status_btn.connect_clicked(move |_btn| {
                let cmd_tx = cmd_tx_model.clone();
                let radio = radio_clone.clone();
                let filename = filename_clone.clone();
                start_download_with_polling(
                    url.clone(),
                    config::model_path_for(&filename),
                    &status_btn_clone,
                    &progress_clone,
                    Box::new(move || {
                        radio.set_sensitive(true);
                        radio.set_active(true);
                        save_and_notify(&cmd_tx, |cfg| cfg.model = filename.clone());
                    }),
                );
            });
        }

        if is_downloaded {
            let filename = model_info.filename.to_string();
            let cmd_tx_radio = cmd_tx.clone();
            radio.connect_toggled(move |btn| {
                if btn.is_active() && Config::load().model != filename {
                    save_and_notify(&cmd_tx_radio, |cfg| cfg.model = filename.clone());
                }
            });
        }

        row.pack_start(&radio, true, true, 0);
        row.pack_start(&progress, true, true, 0);
        row.pack_start(&status_btn, false, false, 0);
        model_box.pack_start(&row, false, false, 0);
    }

    let models_path_row = create_path_row(&config::models_dir());
    model_box.pack_start(&models_path_row, false, false, 4);

    vbox.pack_start(&model_frame, false, false, 0);

    // --- Transcripts section ---
    let (trans_frame, trans_box) = create_section("Transcripts");

    let max_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let keep_all_check = gtk::CheckButton::with_label("Keep all");
    keep_all_check.set_active(config.max_transcripts == 0);

    let max_spin = gtk::SpinButton::with_range(1.0, 99999.0, 10.0);
    max_spin.set_digits(0);
    max_spin.set_value(if config.max_transcripts > 0 { config.max_transcripts as f64 } else { 100.0 });
    max_spin.set_sensitive(config.max_transcripts > 0);

    let cmd_tx_keep_all = cmd_tx.clone();
    let max_spin_ref = max_spin.clone();
    keep_all_check.connect_toggled(move |btn| {
        let all = btn.is_active();
        max_spin_ref.set_sensitive(!all);
        if all {
            save_and_notify(&cmd_tx_keep_all, |cfg| cfg.max_transcripts = 0);
        } else {
            let val = max_spin_ref.value() as u32;
            save_and_notify(&cmd_tx_keep_all, |cfg| cfg.max_transcripts = val);
            transcript::enforce_max(val);
        }
    });

    let cmd_tx_spin = cmd_tx.clone();
    let keep_all_ref = keep_all_check.clone();
    max_spin.connect_value_changed(move |spin| {
        if !keep_all_ref.is_active() {
            let val = spin.value() as u32;
            save_and_notify(&cmd_tx_spin, |cfg| cfg.max_transcripts = val);
            transcript::enforce_max(val);
        }
    });

    max_row.pack_start(&keep_all_check, false, false, 0);
    max_row.pack_start(&max_spin, false, false, 0);
    trans_box.pack_start(&max_row, false, false, 0);

    // Transcript shortcut recorder
    let cmd_tx_trans = cmd_tx.clone();
    let transcript_recorder = build_shortcut_recorder(
        "Shortcut:",
        &config.transcript_shortcut,
        &active_recorder,
        Rc::new(move |shortcut: &str| {
            let s = shortcut.to_string();
            save_and_notify(&cmd_tx_trans, |cfg| cfg.transcript_shortcut = s);
        }),
    );
    trans_box.pack_start(&transcript_recorder, false, false, 0);

    vbox.pack_start(&trans_frame, false, false, 0);

    // --- About section ---
    let (about_frame, about_box) = create_section("About");

    let version_label = gtk::Label::new(Some(&format!("voice-to-text v{}", APP_VERSION)));
    version_label.set_xalign(0.0);
    about_box.pack_start(&version_label, false, false, 0);

    vbox.pack_start(&about_frame, false, false, 0);

    // Wrap in scrolled window
    let scrolled = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.add(&vbox);

    (scrolled, chunk_controls, vad_controls, config.use_vad)
}

/// Populate the transcript list, showing empty state or entries newest-first.
fn populate_transcript_list(
    list_box: &gtk::ListBox,
    empty_label: &gtk::Label,
    clear_btn: &gtk::Button,
) {
    // Remove all existing rows
    for child in list_box.children() {
        list_box.remove(&child);
    }

    let entries = transcript::load_all();

    if entries.is_empty() {
        empty_label.set_visible(true);
        clear_btn.set_sensitive(false);
    } else {
        empty_label.set_visible(false);
        clear_btn.set_sensitive(true);

        // Show newest first
        for entry in entries.iter().rev() {
            let row = build_transcript_row(entry, list_box, empty_label, clear_btn);
            list_box.add(&row);
        }
    }

    list_box.show_all();
}

/// Build a single transcript row widget.
fn build_transcript_row(
    entry: &transcript::Transcript,
    list_box: &gtk::ListBox,
    empty_label: &gtk::Label,
    clear_btn: &gtk::Button,
) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    outer.set_margin_start(4);
    outer.set_margin_end(4);
    outer.set_margin_top(6);
    outer.set_margin_bottom(2);

    // Top line: datetime + buttons
    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let header_text = if entry.process_time_ms > 0 {
        let secs = entry.process_time_ms as f64 / 1000.0;
        format!("{}  ({:.1}s)", entry.datetime, secs)
    } else {
        entry.datetime.clone()
    };
    let datetime_label = gtk::Label::new(Some(&header_text));
    datetime_label.set_xalign(0.0);
    datetime_label.set_hexpand(true);
    datetime_label.style_context().add_class("dim-label");

    let copy_btn = gtk::Button::with_label("Copy");
    copy_btn.set_tooltip_text(Some("Copy to clipboard"));
    let text_for_copy = entry.text.clone();
    copy_btn.connect_clicked(move |_| {
        output::copy_to_clipboard(&text_for_copy);
    });

    let delete_btn = gtk::Button::with_label("Delete");
    delete_btn.set_tooltip_text(Some("Delete this transcript"));
    let timestamp = entry.timestamp;
    let list_box_ref = list_box.clone();
    let empty_label_ref = empty_label.clone();
    let clear_btn_ref = clear_btn.clone();
    let outer_ref = outer.clone();
    delete_btn.connect_clicked(move |_| {
        transcript::delete_transcript(timestamp);
        // Find the ListBoxRow parent and remove it
        if let Some(row) = outer_ref.parent() {
            list_box_ref.remove(&row);
        }
        // Check if list is now empty
        if list_box_ref.children().is_empty() {
            empty_label_ref.set_visible(true);
            clear_btn_ref.set_sensitive(false);
        }
    });

    top_row.pack_start(&datetime_label, true, true, 0);
    top_row.pack_start(&copy_btn, false, false, 0);
    top_row.pack_start(&delete_btn, false, false, 0);

    // Transcript text
    let text_label = gtk::Label::new(Some(&entry.text));
    text_label.set_xalign(0.0);
    text_label.set_line_wrap(true);
    text_label.set_selectable(true);
    text_label.set_max_width_chars(60);

    // Separator
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);

    outer.pack_start(&top_row, false, false, 0);
    outer.pack_start(&text_label, false, false, 0);
    outer.pack_start(&sep, false, false, 4);

    outer
}

/// Build the Transcripts page. Returns (page_widget, refresh_closure).
fn build_transcripts_page() -> (gtk::Box, Box<dyn Fn()>) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // Header row
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.set_margin_top(12);
    header.set_margin_bottom(8);

    let title = gtk::Label::new(None);
    title.set_markup("<b>Transcription History</b>");
    title.set_xalign(0.0);
    title.set_hexpand(true);

    let clear_btn = gtk::Button::with_label("Clear All");

    header.pack_start(&title, true, true, 0);
    header.pack_start(&clear_btn, false, false, 0);
    page.pack_start(&header, false, false, 0);

    // Empty state label
    let empty_label = gtk::Label::new(Some("No transcripts yet."));
    empty_label.style_context().add_class("dim-label");
    empty_label.set_vexpand(true);
    empty_label.set_valign(gtk::Align::Center);
    empty_label.set_no_show_all(true);

    // Scrolled list
    let scrolled = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_vexpand(true);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    scrolled.add(&list_box);

    page.pack_start(&empty_label, true, true, 0);
    page.pack_start(&scrolled, true, true, 0);

    // Path row at bottom
    let path_row = create_path_row(&config::transcripts_path());
    path_row.set_margin_start(12);
    path_row.set_margin_end(12);
    path_row.set_margin_bottom(8);
    path_row.set_margin_top(4);
    page.pack_end(&path_row, false, false, 0);

    // Initial populate
    populate_transcript_list(&list_box, &empty_label, &clear_btn);

    // Clear All handler
    let list_box_clear = list_box.clone();
    let empty_label_clear = empty_label.clone();
    let clear_btn_ref = clear_btn.clone();
    clear_btn.connect_clicked(move |_| {
        transcript::clear_all();
        populate_transcript_list(&list_box_clear, &empty_label_clear, &clear_btn_ref);
    });

    // Refresh closure (used by live updates and connect_map)
    let list_box_refresh = list_box.clone();
    let empty_label_refresh = empty_label.clone();
    let clear_btn_refresh = clear_btn.clone();
    let refresh: Box<dyn Fn()> = Box::new(move || {
        populate_transcript_list(&list_box_refresh, &empty_label_refresh, &clear_btn_refresh);
    });

    // Refresh list when page becomes visible (user navigates to Transcripts tab)
    let list_box_map = list_box.clone();
    let empty_label_map = empty_label.clone();
    let clear_btn_map = clear_btn.clone();
    page.connect_map(move |_| {
        populate_transcript_list(&list_box_map, &empty_label_map, &clear_btn_map);
    });

    (page, refresh)
}

pub fn open_settings_window(
    cmd_tx: Sender<AppCommand>,
    existing: &Rc<RefCell<Option<gtk::Window>>>,
    stack_ref: &Rc<RefCell<Option<gtk::Stack>>>,
    refresher_ref: &Rc<RefCell<Option<Box<dyn Fn()>>>>,
    active_backend: &Rc<RefCell<Option<ActiveBackend>>>,
) {
    // If window already open, just present it
    if let Some(ref win) = *existing.borrow() {
        win.present();
        return;
    }

    let config = Config::load();

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Voice to Text");
    window.set_default_size(700, 500);
    window.set_resizable(true);
    window.set_position(gtk::WindowPosition::Center);

    // Horizontal layout: sidebar | separator | stack
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(160, -1);

    // Build pages
    let (settings_page, chunk_controls, vad_controls, use_vad) =
        build_settings_page(&cmd_tx, &config, active_backend, &window);
    let (transcripts_page, refresh) = build_transcripts_page();

    stack.add_titled(&settings_page, "settings", "Settings");
    stack.add_titled(&transcripts_page, "transcripts", "Transcripts");

    let separator = gtk::Separator::new(gtk::Orientation::Vertical);

    hbox.pack_start(&sidebar, false, false, 0);
    hbox.pack_start(&separator, false, false, 0);
    hbox.pack_start(&stack, true, true, 0);

    window.add(&hbox);

    // Track window lifecycle
    let existing_ref = existing.clone();
    let stack_ref_del = stack_ref.clone();
    let refresher_ref_del = refresher_ref.clone();
    window.connect_delete_event(move |win, _| {
        win.hide();
        *existing_ref.borrow_mut() = None;
        *stack_ref_del.borrow_mut() = None;
        *refresher_ref_del.borrow_mut() = None;
        gtk::glib::Propagation::Stop
    });

    window.show_all();

    // Apply initial visibility after show_all (so no_show_all takes effect)
    chunk_controls.set_visible(!use_vad);
    vad_controls.set_visible(use_vad);

    *stack_ref.borrow_mut() = Some(stack.clone());
    *refresher_ref.borrow_mut() = Some(refresh);
    *existing.borrow_mut() = Some(window);
}

fn download_model_with_progress(
    url: &str,
    target_path: &std::path::Path,
    progress_tx: &crossbeam_channel::Sender<DownloadMsg>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::io::{Read, Write};

    let resp = ureq::get(url).call().map_err(|e| format!("{}", e))?;

    let total_size: Option<u64> = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok());

    let mut reader = resp.into_reader();

    let tmp_path = target_path.with_extension("bin.part");
    let mut file = std::fs::File::create(&tmp_path)?;

    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536];
    let mut last_pct = 0u8;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        if let Some(total) = total_size {
            let pct = (downloaded * 100 / total) as u8;
            if pct != last_pct {
                last_pct = pct;
                let _ = progress_tx.send(DownloadMsg::Progress(downloaded as f64 / total as f64));
            }
        }
    }

    file.flush()?;
    drop(file);

    std::fs::rename(&tmp_path, target_path)?;
    Ok(())
}
