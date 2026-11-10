//! Commands called from the settings window.

use crate::config::{MacCfg, WinCfg};
use crate::shared::Shared;
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
    version: &'static str,
    device_name: String,
    summary: String,
    connected: Option<String>,
    controlling: bool,
    message: String,
    peers: Vec<PeerView>,
    found: Vec<FoundView>,
    pairing_code: Option<String>,
    pairing_seconds: u64,
    accessibility: Option<bool>,
    local_ip: Option<String>,
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

#[tauri::command]
pub fn get_status(state: St<'_>) -> StatusView {
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
    #[cfg(target_os = "macos")]
    let accessibility = Some(kvm_mac::platform::accessibility_ok(false));
    #[cfg(not(target_os = "macos"))]
    let accessibility = None;
    StatusView {
        platform: platform(),
        version: env!("CARGO_PKG_VERSION"),
        device_name,
        summary,
        connected: l.connected.clone(),
        controlling: l.controlling,
        message: l.message.clone(),
        peers,
        found,
        pairing_code,
        pairing_seconds,
        accessibility,
        local_ip: local_ip(),
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
    state.inner().update_cfg(|c| {
        let n = device_name.trim();
        if !n.is_empty() {
            c.device_name = n.chars().take(60).collect();
        }
        c.win = win;
        c.mac = mac;
    });
    let al = app.autolaunch();
    let now = al.is_enabled().unwrap_or(false);
    if autostart != now {
        let r = if autostart { al.enable() } else { al.disable() };
        r.map_err(|e| format!("开机启动设置失败：{e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn start_pairing(state: St<'_>) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::mac_side::start_pairing(state.inner()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        Err("配对码在 Mac 上生成".into())
    }
}

#[tauri::command]
pub fn stop_pairing(state: St<'_>) {
    state.inner().live().pairing = None;
}

#[tauri::command]
pub async fn pair_with(state: St<'_>, target: String, code: String) -> Result<String, String> {
    let sh = state.inner().clone();
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || crate::win_side::pair(&sh, &target, &code))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (sh, target, code);
        Err("请在 Windows 上输入配对码".into())
    }
}

#[tauri::command]
pub fn forget_peer(state: St<'_>, id: String) {
    let sh = state.inner();
    sh.update_cfg(|c| c.peers.retain(|p| p.id != id));
    // Close the current session if it was with that computer.
    let l = sh.live();
    if let Some((_, s)) = &l.session {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
}

#[tauri::command]
pub fn open_accessibility() {
    #[cfg(target_os = "macos")]
    kvm_mac::platform::open_accessibility_settings();
}
