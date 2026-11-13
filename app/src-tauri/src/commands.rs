//! Commands called from the settings window.

use crate::config::{MacCfg, WinCfg};
use crate::shared::Shared;
use crate::{link, updater};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

type St<'a> = State<'a, Arc<Shared>>;

#[derive(Serialize)]
pub struct PeerView {
    id: String,
    name: String,
    last_addr: Option<String>,
}

#[derive(Serialize)]
pub struct FoundView {
    id: String,
    name: String,
    addr: String,
    paired: bool,
}

#[derive(Serialize)]
pub struct StatusView {
    platform: &'static str,
    version: String,
    device_name: String,
    summary: String,
    connected: Option<String>,
    controlling: bool,
    message: String,
    listen_error: Option<String>,
    peers: Vec<PeerView>,
    found: Vec<FoundView>,
    pairing_code: Option<String>,
    pairing_seconds: u64,
    accessibility: Option<bool>,
    local_ip: Option<String>,
    update_available: Option<String>,
    update_status: String,
}

fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

fn local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

pub fn version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub fn get_status(app: AppHandle, state: St<'_>) -> StatusView {
    let sh = state.inner();
    let summary = sh.summary();
    let (device_name, peers) = {
        let c = sh.cfg();
        (
            c.device_name.clone(),
            c.peers
                .iter()
                .map(|p| PeerView { id: p.id.clone(), name: p.name.clone(), last_addr: p.last_addr.clone() })
                .collect::<Vec<_>>(),
        )
    };
    #[cfg(target_os = "macos")]
    let accessibility = Some(kvm_mac::platform::accessibility_ok(false));
    #[cfg(not(target_os = "macos"))]
    let accessibility = None;
    let l = sh.live();
    let mut found: Vec<FoundView> = l
        .found
        .values()
        .map(|f| FoundView {
            id: f.id.clone(),
            name: f.name.clone(),
            addr: f.addrs.first().map(|a| a.to_string()).unwrap_or_default(),
            paired: peers.iter().any(|p| p.id == f.id),
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    let (pairing_code, pairing_seconds) = match &l.pairing {
        Some((c, until)) if Instant::now() < *until => {
            (Some(c.clone()), until.saturating_duration_since(Instant::now()).as_secs())
        }
        _ => (None, 0),
    };
    StatusView {
        platform: platform(),
        version: version(&app),
        device_name,
        summary,
        connected: l.connected.clone(),
        controlling: l.controlling,
        message: l.message.clone(),
        listen_error: l.listen_error.clone(),
        peers,
        found,
        pairing_code,
        pairing_seconds,
        accessibility,
        local_ip: local_ip(),
        update_available: l.update_available.clone(),
        update_status: l.update_status.clone(),
    }
}

#[derive(Serialize)]
pub struct SettingsView {
    device_name: String,
    win: WinCfg,
    mac: MacCfg,
    autostart: bool,
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: St<'_>) -> SettingsView {
    let c = state.inner().cfg();
    SettingsView {
        device_name: c.device_name.clone(),
        win: c.win.clone(),
        mac: c.mac.clone(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
    }
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: St<'_>,
    device_name: String,
    win: WinCfg,
    mac: MacCfg,
    autostart: bool,
) -> Result<(), String> {
    let sh = state.inner();
    sh.update_cfg(|c| {
        let n = device_name.trim();
        if !n.is_empty() {
            c.device_name = n.chars().take(60).collect();
        }
        c.win = win;
        c.mac = mac;
    });
    #[cfg(windows)]
    crate::win_side::apply_settings(sh);
    let al = app.autolaunch();
    let now = al.is_enabled().unwrap_or(false);
    if autostart != now {
        let r = if autostart { al.enable() } else { al.disable() };
        r.map_err(|e| format!("开机启动设置失败：{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn start_pairing(state: St<'_>) -> String {
    link::start_pairing(state.inner())
}

#[tauri::command]
pub fn stop_pairing(state: St<'_>) {
    state.inner().live().pairing = None;
}

#[tauri::command]
pub async fn pair_with(state: St<'_>, target: String, code: String) -> Result<String, String> {
    let sh = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || link::pair_with(&sh, &target, &code))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn forget_peer(state: St<'_>, id: String) {
    let sh = state.inner();
    sh.update_cfg(|c| c.peers.retain(|p| p.id != id));
    // Close the current session if it was with that computer.
    let l = sh.live();
    if l.connected_id.as_deref() == Some(id.as_str()) {
        if let Some((_, s)) = &l.session {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[tauri::command]
pub fn open_accessibility() {
    #[cfg(target_os = "macos")]
    kvm_mac::platform::open_accessibility_settings();
}

/// Start capturing the next key combination pressed on this keyboard (Windows).
#[tauri::command]
pub fn start_recording(state: St<'_>) -> Result<(), String> {
    state.inner().live().recorded = None;
    #[cfg(windows)]
    {
        kvm_win::capture::start_recording();
        Ok(())
    }
    #[cfg(not(windows))]
    Err("快捷键在 Windows 上录制".into())
}

#[tauri::command]
pub fn cancel_recording() {
    #[cfg(windows)]
    kvm_win::capture::cancel_recording();
}

/// The recorded combination, once there is one: [mods, key].
#[tauri::command]
pub fn take_recorded(state: St<'_>) -> Option<(u8, u16)> {
    state.inner().live().recorded.take()
}

#[tauri::command]
pub async fn check_update(app: AppHandle, state: St<'_>) -> Result<Option<String>, String> {
    let sh = state.inner().clone();
    let current = version(&app);
    let found = tauri::async_runtime::spawn_blocking(move || updater::check(&current))
        .await
        .map_err(|e| e.to_string())??;
    let v = found.map(|l| l.version);
    sh.live().update_available = v.clone();
    Ok(v)
}

#[tauri::command]
pub async fn install_update(app: AppHandle, state: St<'_>) -> Result<(), String> {
    let sh = state.inner().clone();
    let current = version(&app);
    sh.live().update_status = "正在下载新版本…".into();
    let r = tauri::async_runtime::spawn_blocking(move || {
        let latest = updater::check(&current)?.ok_or("已经是最新版本")?;
        updater::install(&app, &latest)
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Err(e) = &r {
        sh.live().update_status = e.clone();
    }
    r
}
