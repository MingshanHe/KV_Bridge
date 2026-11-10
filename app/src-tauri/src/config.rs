//! Settings saved on disk (config.json in the app's config folder).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub device_id: String,
    pub device_name: String,
    pub peers: Vec<Peer>,
    pub win: WinCfg,
    pub mac: MacCfg,
    /// Start-at-login has been turned on once (done automatically on first run).
    pub autostart_initialised: bool,
}

/// A paired computer.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// Shared key from pairing (hex). Never shown in the UI.
    pub psk: String,
    /// Last address it was reached at.
    pub last_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WinCfg {
    /// "forward", "back" or "none".
    pub toggle_button: String,
    /// Ctrl+Alt+M also switches.
    pub hotkey: bool,
    /// Beep when switching.
    pub sound: bool,
}

impl Default for WinCfg {
    fn default() -> Self {
        WinCfg { toggle_button: "forward".into(), hotkey: true, sound: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MacCfg {
    /// Windows-style shortcuts (Ctrl -> Cmd, Alt+Tab, terminal Ctrl+C ...).
    pub windows_shortcuts: bool,
    pub invert_scroll: bool,
    /// Pixels per wheel notch.
    pub scroll_speed: f64,
    /// "auto", "uk", "us" or "off".
    pub layout: String,
    /// Japanese Windows keyboard (英数 / 半角全角 keys).
    pub jis: bool,
}

impl Default for MacCfg {
    fn default() -> Self {
        MacCfg {
            windows_shortcuts: true,
            invert_scroll: false,
            scroll_speed: 40.0,
            layout: "auto".into(),
            jis: false,
        }
    }
}

impl WinCfg {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn router_config(&self) -> kvm_core::router::Config {
        use kvm_core::protocol::button;
        let toggle_button = match self.toggle_button.as_str() {
            "back" => Some(button::BACK),
            "none" => None,
            _ => Some(button::FORWARD),
        };
        // Never leave the user without a way to switch back.
        let hotkey = self.hotkey || toggle_button.is_none();
        kvm_core::router::Config { toggle_button, hotkey }
    }
}

impl Config {
    /// Load, filling in a device id and name the first time.
    pub fn load(path: &Path) -> Config {
        let mut c: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut changed = false;
        if c.device_id.is_empty() {
            c.device_id = kvm_net::new_device_id();
            changed = true;
        }
        if c.device_name.trim().is_empty() {
            c.device_name = computer_name();
            changed = true;
        }
        if changed {
            let _ = c.save(path);
        }
        c
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    pub fn peer(&self, id: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.id == id)
    }

    /// Add or replace a paired computer.
    pub fn upsert_peer(&mut self, p: Peer) {
        self.peers.retain(|x| x.id != p.id);
        self.peers.insert(0, p);
    }
}

/// The computer's name as the user knows it.
pub fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Computer".into())
}
