//! Mac: replay the Windows keyboard and mouse here during a session.

use crate::config::{MacCfg, Peer};
use crate::shared::Shared;
use crate::tray;
use kvm_core::layout::WinLayout;
use kvm_core::protocol::Msg;
use kvm_mac::platform::{accessibility_ok, Event, Output};
use kvm_mac::state::{LayoutMode, Settings};
use kvm_net::secure;
use std::net::Shutdown;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn settings(m: &MacCfg) -> Settings {
    Settings {
        swap_ctrl_cmd: m.windows_shortcuts,
        invert_scroll: m.invert_scroll,
        scroll_px_per_notch: m.scroll_speed.clamp(1.0, 1000.0),
        jis: m.jis,
        layout: match m.layout.as_str() {
            "uk" => LayoutMode::Fixed(WinLayout::Uk),
            "us" => LayoutMode::Fixed(WinLayout::Us),
            "off" => LayoutMode::Off,
            _ => LayoutMode::Auto,
        },
    }
}

pub fn start(_app: AppHandle, _sh: Arc<Shared>) {
    // Ask once at start; macOS shows its own dialog when the permission is missing.
    accessibility_ok(true);
}

/// Replay input from Windows until the connection ends.
pub fn run_session(app: &AppHandle, sh: &Arc<Shared>, _peer: &Peer, mut tx: secure::Sender, mut rx: secure::Receiver) {
    let _ = rx.stream().set_read_timeout(Some(PEER_TIMEOUT));

    // Heartbeats let Windows notice quickly if this Mac sleeps or drops off Wi-Fi.
    thread::spawn(move || loop {
        thread::sleep(HEARTBEAT);
        if tx.send(&[Msg::Heartbeat]).is_err() {
            break;
        }
    });

    let mut gen = sh.generation();
    let mut out = Output::new(settings(&sh.cfg().mac), false);
    while let Ok(msgs) = rx.recv() {
        let g = sh.generation();
        if g != gen {
            gen = g;
            out.set_settings(settings(&sh.cfg().mac));
        }
        for m in msgs {
            let changed = match out.handle(m) {
                Some(Event::Entered { .. }) => Some(true),
                Some(Event::Left) => Some(false),
                None => None,
            };
            if let Some(c) = changed {
                sh.live().controlling = c;
                tray::refresh(app, sh);
            }
        }
    }
    out.reset();
    drop(out);
    let _ = rx.stream().shutdown(Shutdown::Both);
}
