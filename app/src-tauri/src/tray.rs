//! Tray icon (Windows) / menu-bar icon (Mac).

use crate::shared::Shared;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const LOCAL: &[u8] = include_bytes!("../icons/tray-local.png");
const REMOTE: &[u8] = include_bytes!("../icons/tray-remote.png");

struct TrayState {
    status: MenuItem<Wry>,
}

pub fn setup(app: &tauri::App) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "KV Bridge", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开 KV Bridge", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 KV Bridge", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&status, &sep, &open, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(LOCAL)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("KV Bridge")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    app.manage(TrayState { status });
    Ok(())
}

/// Refresh the icon and text from the current state. Never call while holding
/// a lock on `Shared` (this waits for the main thread).
pub fn refresh(app: &AppHandle, sh: &Shared) {
    let text = sh.summary();
    let controlling = sh.live().controlling;
    if let Some(t) = app.tray_by_id("main") {
        if let Ok(img) = Image::from_bytes(if controlling { REMOTE } else { LOCAL }) {
            let _ = t.set_icon(Some(img));
        }
        let _ = t.set_icon_as_template(!controlling && cfg!(target_os = "macos"));
        let _ = t.set_tooltip(Some(format!("KV Bridge – {text}")));
    }
    if let Some(s) = app.try_state::<TrayState>() {
        let _ = s.status.set_text(text);
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
