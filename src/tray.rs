use crossbeam_channel::Sender;
use muda::{Menu, MenuId, MenuItem, MenuEvent, PredefinedMenuItem, Submenu};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use gtk::prelude::StackExt;

use crate::config::{ActiveBackend, AppCommand, RecordingState};
use crate::output;
use crate::settings;
use crate::transcript;

/// Messages sent from coordinator to the GTK/tray thread.
#[derive(Debug, Clone)]
pub enum TrayUpdate {
    State(RecordingState),
    MicMuted(bool),
    BackendInfo(ActiveBackend),
    CopyToClipboard(String),
    OpenSettings,
    OpenTranscripts,
    RefreshTranscripts,
    Quit,
}

const VIEW_ALL_TRANSCRIPTS_ID: &str = "view_all_transcripts";
const MAX_SUBMENU_ITEMS: usize = 20;

pub struct Tray {
    pub icon: TrayIcon,
    pub toggle_item: MenuItem,
    pub cmd_tx: Sender<AppCommand>,
    pub settings_window: Rc<RefCell<Option<gtk::Window>>>,
    pub settings_stack: Rc<RefCell<Option<gtk::Stack>>>,
    pub transcript_refresher: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    pub active_backend: Rc<RefCell<Option<ActiveBackend>>>,
    pub transcripts_submenu: Submenu,
    pub transcript_texts: Arc<Mutex<HashMap<MenuId, String>>>,
    pub is_mic_muted: Rc<RefCell<bool>>,
    pub current_state: Rc<RefCell<RecordingState>>,
}

static ICON_BASE_PNG: &[u8] = include_bytes!("../assets/mic-idle.png");

fn load_icon(png_data: &[u8]) -> Icon {
    let decoder = png::Decoder::new(png_data);
    let mut reader = decoder.read_info().expect("Failed to read PNG header");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("Failed to decode PNG");
    let rgba = &buf[..info.buffer_size()];
    Icon::from_rgba(rgba.to_vec(), info.width, info.height).expect("Failed to create icon")
}

/// Decode the base icon PNG and recolor all visible pixels to (r, g, b).
fn colorized_icon(r: u8, g: u8, b: u8) -> Icon {
    let decoder = png::Decoder::new(ICON_BASE_PNG);
    let mut reader = decoder.read_info().expect("Failed to read PNG header");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("Failed to decode PNG");
    let rgba = &mut buf[..info.buffer_size()];

    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[3] > 0 {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    }

    Icon::from_rgba(rgba.to_vec(), info.width, info.height).expect("Failed to create icon")
}

/// Grey mic — idle/off
pub fn idle_icon() -> Icon {
    load_icon(ICON_BASE_PNG)
}

/// Blue mic — push-to-talk active
pub fn ptt_icon() -> Icon {
    colorized_icon(0x32, 0x78, 0xDC)
}

/// Red mic — always listen active
pub fn listen_icon() -> Icon {
    colorized_icon(0xDC, 0x32, 0x32)
}

/// Grey mic with red strike-through overlay — mic muted
pub fn muted_icon() -> Icon {
    let decoder = png::Decoder::new(ICON_BASE_PNG);
    let mut reader = decoder.read_info().expect("Failed to read PNG header");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("Failed to decode PNG");
    let rgba = &mut buf[..info.buffer_size()];
    let w = info.width as i32;

    // Draw a red circle with white diagonal slash in the lower-right quadrant
    let cx = 24i32;
    let cy = 24i32;
    let r_outer = 7i32;
    let r_inner = 5i32;

    for y in 0..info.height as i32 {
        for x in 0..w {
            let dx = x - cx;
            let dy = y - cy;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq <= r_outer * r_outer {
                let idx = ((y * w + x) * 4) as usize;
                if dist_sq > r_inner * r_inner {
                    // Red circle ring
                    rgba[idx] = 0xDC;
                    rgba[idx + 1] = 0x32;
                    rgba[idx + 2] = 0x32;
                    rgba[idx + 3] = 255;
                } else {
                    // Inside circle: draw white diagonal slash (2px thick)
                    let on_slash = (dx - dy).abs() <= 1;
                    if on_slash {
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 255;
                    } else {
                        // Fill inside with semi-transparent red
                        rgba[idx] = 0xDC;
                        rgba[idx + 1] = 0x32;
                        rgba[idx + 2] = 0x32;
                        rgba[idx + 3] = 160;
                    }
                }
            }
        }
    }

    Icon::from_rgba(rgba.to_vec(), info.width, info.height).expect("Failed to create icon")
}

fn truncate_for_menu(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim().replace('\n', " ");
    if trimmed.chars().count() <= max_chars {
        trimmed
    } else {
        let end = trimmed
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(trimmed.len());
        format!("{}...", &trimmed[..end])
    }
}

fn rebuild_transcript_submenu(
    submenu: &Submenu,
    transcript_texts: &Arc<Mutex<HashMap<MenuId, String>>>,
) {
    // Clear existing items
    while !submenu.items().is_empty() {
        let _ = submenu.remove_at(0);
    }

    let entries = transcript::load_all();
    let mut map = transcript_texts.lock().unwrap();
    map.clear();

    if entries.is_empty() {
        let _ = submenu.append(&MenuItem::new("No transcripts yet", false, None));
    } else {
        for entry in entries.iter().rev().take(MAX_SUBMENU_ITEMS) {
            let label = truncate_for_menu(&entry.text, 50);
            let item = MenuItem::new(&label, true, None);
            map.insert(item.id().clone(), entry.text.clone());
            let _ = submenu.append(&item);
        }
    }

    let _ = submenu.append(&PredefinedMenuItem::separator());
    let view_all = MenuItem::with_id(
        MenuId::new(VIEW_ALL_TRANSCRIPTS_ID),
        "View All...",
        true,
        None,
    );
    let _ = submenu.append(&view_all);
}

pub fn create_tray(cmd_tx: Sender<AppCommand>) -> Tray {
    let menu = Menu::new();
    let toggle_item = MenuItem::new("Always Listen", true, None);
    let settings_item = MenuItem::new("Settings...", true, None);
    let transcripts_submenu = Submenu::new("Transcripts", true);
    let quit_item = MenuItem::new("Quit", true, None);

    let transcript_texts: Arc<Mutex<HashMap<MenuId, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    rebuild_transcript_submenu(&transcripts_submenu, &transcript_texts);

    menu.append(&toggle_item).expect("Failed to add menu item");
    menu.append(&PredefinedMenuItem::separator()).expect("Failed to add separator");
    menu.append(&settings_item).expect("Failed to add menu item");
    menu.append(&transcripts_submenu).expect("Failed to add submenu");
    menu.append(&PredefinedMenuItem::separator()).expect("Failed to add separator");
    menu.append(&quit_item).expect("Failed to add menu item");

    let tray_icon = TrayIconBuilder::new()
        .with_icon(idle_icon())
        .with_tooltip("Voice to Text — Idle")
        .with_menu(Box::new(menu))
        .build()
        .expect("Failed to create tray icon");

    let toggle_id = toggle_item.id().clone();
    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();
    let view_all_id = MenuId::new(VIEW_ALL_TRANSCRIPTS_ID);

    let cmd_tx_menu = cmd_tx.clone();
    let transcript_texts_menu = transcript_texts.clone();
    std::thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        loop {
            if let Ok(event) = receiver.recv() {
                if event.id == toggle_id {
                    let _ = cmd_tx_menu.send(AppCommand::ToggleAlwaysListen);
                } else if event.id == settings_id {
                    let _ = cmd_tx_menu.send(AppCommand::OpenSettings);
                } else if event.id == view_all_id {
                    let _ = cmd_tx_menu.send(AppCommand::OpenTranscripts);
                } else if event.id == quit_id {
                    let _ = cmd_tx_menu.send(AppCommand::Quit);
                } else if let Some(text) = transcript_texts_menu.lock().unwrap().get(&event.id).cloned() {
                    let _ = cmd_tx_menu.send(AppCommand::CopyTranscript(text));
                }
            }
        }
    });

    // Left-click on tray icon opens settings
    let cmd_tx_click = cmd_tx.clone();
    std::thread::spawn(move || {
        let receiver = TrayIconEvent::receiver();
        loop {
            if let Ok(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) = receiver.recv()
            {
                let _ = cmd_tx_click.send(AppCommand::OpenSettings);
            }
        }
    });

    Tray {
        icon: tray_icon,
        toggle_item,
        cmd_tx,
        settings_window: Rc::new(RefCell::new(None)),
        settings_stack: Rc::new(RefCell::new(None)),
        transcript_refresher: Rc::new(RefCell::new(None)),
        active_backend: Rc::new(RefCell::new(None)),
        transcripts_submenu,
        transcript_texts,
        is_mic_muted: Rc::new(RefCell::new(false)),
        current_state: Rc::new(RefCell::new(RecordingState::Idle)),
    }
}

/// Called from the GTK main thread to apply tray updates.
pub fn apply_update(tray: &Tray, update: TrayUpdate) {
    match update {
        TrayUpdate::State(RecordingState::Idle) => {
            *tray.current_state.borrow_mut() = RecordingState::Idle;
            if *tray.is_mic_muted.borrow() {
                let _ = tray.icon.set_icon(Some(muted_icon()));
                let _ = tray.icon.set_tooltip(Some("Voice to Text — Mic Muted"));
            } else {
                let _ = tray.icon.set_icon(Some(idle_icon()));
                let _ = tray.icon.set_tooltip(Some("Voice to Text — Idle"));
            }
            tray.toggle_item.set_text("Always Listen");
        }
        TrayUpdate::State(RecordingState::PushToTalk) => {
            *tray.current_state.borrow_mut() = RecordingState::PushToTalk;
            let _ = tray.icon.set_icon(Some(ptt_icon()));
            let _ = tray.icon.set_tooltip(Some("Voice to Text — Push to Talk"));
            tray.toggle_item.set_text("Stop Listening");
        }
        TrayUpdate::State(RecordingState::AlwaysListen) => {
            *tray.current_state.borrow_mut() = RecordingState::AlwaysListen;
            let _ = tray.icon.set_icon(Some(listen_icon()));
            let _ = tray.icon.set_tooltip(Some("Voice to Text — Always Listen"));
            tray.toggle_item.set_text("Stop Listening");
        }
        TrayUpdate::MicMuted(muted) => {
            *tray.is_mic_muted.borrow_mut() = muted;
            if *tray.current_state.borrow() == RecordingState::Idle {
                if muted {
                    let _ = tray.icon.set_icon(Some(muted_icon()));
                    let _ = tray.icon.set_tooltip(Some("Voice to Text — Mic Muted"));
                } else {
                    let _ = tray.icon.set_icon(Some(idle_icon()));
                    let _ = tray.icon.set_tooltip(Some("Voice to Text — Idle"));
                }
            }
        }
        TrayUpdate::BackendInfo(backend) => {
            *tray.active_backend.borrow_mut() = Some(backend);
        }
        TrayUpdate::CopyToClipboard(text) => {
            output::copy_to_clipboard(&text);
            let preview: String = text.chars().take(50).collect();
            let body = if text.chars().count() > 50 {
                format!("{}...", preview)
            } else {
                preview
            };
            output::send_notification("Copied to clipboard", &body, "edit-copy");
        }
        TrayUpdate::OpenSettings => {
            settings::open_settings_window(
                tray.cmd_tx.clone(),
                &tray.settings_window,
                &tray.settings_stack,
                &tray.transcript_refresher,
                &tray.active_backend,
            );
        }
        TrayUpdate::OpenTranscripts => {
            settings::open_settings_window(
                tray.cmd_tx.clone(),
                &tray.settings_window,
                &tray.settings_stack,
                &tray.transcript_refresher,
                &tray.active_backend,
            );
            if let Some(ref stack) = *tray.settings_stack.borrow() {
                stack.set_visible_child_name("transcripts");
            }
        }
        TrayUpdate::RefreshTranscripts => {
            rebuild_transcript_submenu(&tray.transcripts_submenu, &tray.transcript_texts);
            if let Some(ref refresher) = *tray.transcript_refresher.borrow() {
                refresher();
            }
        }
        TrayUpdate::Quit => {
            gtk::main_quit();
        }
    }
}
