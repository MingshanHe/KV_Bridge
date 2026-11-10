//! State shared between the background threads and the settings window.

use crate::config::Config;
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub struct Shared {
    path: PathBuf,
    cfg: Mutex<Config>,
    /// Bumped on every settings change, so running sessions can pick them up.
    pub gen: AtomicU64,
    live: Mutex<Live>,
}

/// Things that change while running (not saved).
#[derive(Default)]
pub struct Live {
    /// Name of the computer we're connected to.
    pub connected: Option<String>,
    /// Windows: the Mac has control. Mac: Windows is controlling this Mac.
    pub controlling: bool,
    /// Last thing worth telling the user.
    pub message: String,
    /// Windows: Macs seen on the network, by id.
    pub found: HashMap<String, kvm_net::discovery::Found>,
    /// Mac: the pairing code being shown, and until when it is valid.
    pub pairing: Option<(String, Instant)>,
    /// Mac: the current session's connection, so a newer one can replace it.
    pub session: Option<(u64, TcpStream)>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub session_counter: u64,
}

impl Shared {
    pub fn new(path: PathBuf, cfg: Config) -> Shared {
        Shared { path, cfg: Mutex::new(cfg), gen: AtomicU64::new(0), live: Mutex::new(Live::default()) }
    }

    pub fn cfg(&self) -> MutexGuard<'_, Config> {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Change and save the settings.
    pub fn update_cfg(&self, f: impl FnOnce(&mut Config)) {
        {
            let mut c = self.cfg();
            f(&mut c);
            let _ = c.save(&self.path);
        }
        self.gen.fetch_add(1, Ordering::SeqCst);
    }

    pub fn generation(&self) -> u64 {
        self.gen.load(Ordering::SeqCst)
    }

    pub fn say(&self, msg: impl Into<String>) {
        self.live().message = msg.into();
    }

    /// One line describing the current state (tray tooltip and menu).
    pub fn summary(&self) -> String {
        let l = self.live();
        let on_mac = cfg!(target_os = "macos");
        match (&l.connected, l.controlling, on_mac) {
            (Some(n), true, false) => format!("正在控制 {n}"),
            (Some(n), true, true) => format!("{n} 正在控制这台 Mac"),
            (Some(n), false, _) => format!("已连接 {n}"),
            (None, _, false) => "未连接 Mac".into(),
            (None, _, true) => "等待 Windows 连接".into(),
        }
    }
}
