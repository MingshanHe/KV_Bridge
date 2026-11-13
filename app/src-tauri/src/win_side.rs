//! Windows: capture the keyboard and mouse and stream them to the Mac during a session.

use crate::config::Peer;
use crate::shared::Shared;
use crate::tray;
use kvm_core::protocol::{action, Msg};
use kvm_core::router::Notice;
use kvm_net::secure;
use kvm_win::capture::{self, Event};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

/// Input to send to the Mac. Only one session can hold it.
static OUTGOING: OnceLock<Mutex<Receiver<Msg>>> = OnceLock::new();

pub fn start(app: AppHandle, sh: Arc<Shared>) {
    let rc = sh.cfg().win.router_config();
    let Some(cap) = capture::start(rc) else {
        sh.say("键盘鼠标捕获启动失败");
        return;
    };
    let _ = OUTGOING.set(Mutex::new(cap.outgoing));

    // Switches, recorded shortcuts and log lines from the hooks.
    let events = cap.events;
    thread::spawn(move || {
        for ev in events {
            match ev {
                Event::Notice(n) => {
                    match n {
                        Notice::EnteredRemote => sh.live().controlling = true,
                        Notice::LeftRemote => sh.live().controlling = false,
                        Notice::NotConnected => sh.say("还没有连接到 Mac，暂时不能切换"),
                        Notice::Unmapped(_) => continue,
                    }
                    if sh.cfg().win.sound {
                        capture::beep(&n);
                    }
                    tray::refresh(&app, &sh);
                }
                Event::Recorded { mods, key } => sh.live().recorded = Some((mods, key)),
                Event::Log(l) => sh.say(l),
            }
        }
    });
}

/// The custom shortcuts, as messages for the Mac.
fn rule_msgs(sh: &Shared) -> Vec<Msg> {
    let mut v = vec![Msg::RulesClear];
    for r in &sh.cfg().win.rules {
        let act = if r.action == "input" { action::SWITCH_INPUT } else { action::KEYS };
        v.push(Msg::Rule {
            from_mods: r.from.mods,
            from_key: r.from.key,
            action: act,
            to_mods: r.to_mods,
            to_key: r.to_key,
        });
    }
    v
}

/// Stream input to the Mac until the connection ends.
pub fn run_session(app: &AppHandle, sh: &Arc<Shared>, peer: &Peer, mut tx: secure::Sender, mut rx: secure::Receiver) {
    let Some(lock) = OUTGOING.get() else { return };
    let Ok(outgoing) = lock.try_lock() else {
        tx.shutdown();
        return;
    };
    let _ = rx.stream().set_read_timeout(Some(PEER_TIMEOUT));
    // Drop anything queued while no Mac was attached.
    while outgoing.try_recv().is_ok() {}
    if tx.send(&rule_msgs(sh)).is_err() {
        return;
    }
    capture::set_connected(true);
    tray::refresh(app, sh);

    // The Mac sends heartbeats; silence means it slept or left the network.
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        thread::spawn(move || {
            while rx.recv().is_ok() {}
            alive.store(false, Ordering::SeqCst);
        });
    }

    let mut gen = sh.generation();
    let mut batch = Vec::with_capacity(32);
    while alive.load(Ordering::SeqCst) {
        batch.clear();
        match outgoing.recv_timeout(HEARTBEAT) {
            Ok(m) => {
                batch.push(m);
                while let Ok(m) = outgoing.try_recv() {
                    batch.push(m);
                }
            }
            Err(RecvTimeoutError::Timeout) => batch.push(Msg::Heartbeat),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Settings changed: new switch keys here, new custom shortcuts on the Mac.
        let g = sh.generation();
        if g != gen {
            gen = g;
            capture::set_config(sh.cfg().win.router_config());
            batch.extend(rule_msgs(sh));
        }
        if tx.send(&batch).is_err() {
            break;
        }
    }
    tx.shutdown();
    capture::set_connected(false);
    let _ = peer;
}

/// Settings changed while no Mac is connected.
pub fn apply_settings(sh: &Shared) {
    capture::set_config(sh.cfg().win.router_config());
}
