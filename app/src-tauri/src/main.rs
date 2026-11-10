//! KV Bridge: use the Windows laptop's keyboard and mouse on the Mac.
//! The same app runs on both; the platform decides its role.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
#[cfg(target_os = "macos")]
mod mac_side;
mod shared;
mod tray;
#[cfg(windows)]
mod win_side;

use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("config.json");
            let cfg = config::Config::load(&path);
            let sh = Arc::new(shared::Shared::new(path, cfg));
            app.manage(sh.clone());
            tray::setup(app)?;

            // Start at login by default (the user can turn it off in settings).
            let first_run = !sh.cfg().autostart_initialised;
            if first_run {
                let _ = app.autolaunch().enable();
                sh.update_cfg(|c| c.autostart_initialised = true);
            }

            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                mac_side::start(app.handle().clone(), sh.clone());
            }
            #[cfg(windows)]
            win_side::start(app.handle().clone(), sh.clone());

            tray::refresh(app.handle(), &sh);
            if !std::env::args().any(|a| a == "--autostart") {
                tray::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps KV Bridge running in the tray / menu bar.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_settings,
            commands::save_settings,
            commands::start_pairing,
            commands::stop_pairing,
            commands::pair_with,
            commands::forget_peer,
            commands::open_accessibility,
        ])
        .run(tauri::generate_context!())
        .expect("error while running KV Bridge");
}
