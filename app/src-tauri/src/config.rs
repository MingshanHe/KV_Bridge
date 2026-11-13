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

/// A key combination on the Windows keyboard.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Combo {
    /// Modifier bits: 1 Ctrl, 2 Shift, 4 Alt, 8 Win.
    pub mods: u8,
    /// HID usage of the key.
    pub key: u16,
}

/// A custom shortcut: pressing `from` on Windows does something on the Mac.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleCfg {
    pub from: Combo,
    /// "keys" (press `to_mods` + `to_key` on the Mac) or "input" (switch input source).
    pub action: String,
    /// Mac modifier bits: 1 Ctrl, 2 Shift, 4 Option, 8 Cmd.
    #[serde(default)]
    pub to_mods: u8,
    #[serde(default)]
    pub to_key: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WinCfg {
    /// "forward", "back" or "none".
    pub toggle_button: String,
    /// Keyboard shortcut that switches control (None = mouse button only).
    pub switch_hotkey: Option<Combo>,
    /// Beep when switching.
    pub sound: bool,
    /// Custom shortcuts, applied on the Mac.
    pub rules: Vec<RuleCfg>,
}

impl Default for WinCfg {
    fn default() -> Self {
        let d = kvm_core::router::Hotkey::DEFAULT;
        WinCfg {
            toggle_button: "forward".into(),
            switch_hotkey: Some(Combo { mods: d.mods, key: d.key }),
            sound: true,
            rules: Vec::new(),
        }
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
        let mut hotkey = self.switch_hotkey.map(|c| kvm_core::router::Hotkey { mods: c.mods, key: c.key });
        // Never leave the user without a way to switch back.
        if toggle_button.is_none() && hotkey.is_none() {
            hotkey = Some(kvm_core::router::Hotkey::DEFAULT);
        }
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
