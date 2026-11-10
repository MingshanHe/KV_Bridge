//! Mac: announce this Mac on the LAN, accept pairing and sessions, replay input.

use crate::config::{MacCfg, Peer};
use crate::shared::Shared;
use crate::tray;
use kvm_core::layout::WinLayout;
use kvm_net::hello::{self, Hello};
use kvm_net::pair::{self, PairError};
use kvm_net::{discovery, from_hex32, secure, to_hex, PORT};
use kvm_mac::platform::{accessibility_ok, Event, Output};
use kvm_mac::state::{LayoutMode, Settings};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a pairing code stays valid.
pub const PAIRING_TIME: Duration = Duration::from_secs(300);

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

pub fn start(app: AppHandle, sh: Arc<Shared>) {
    // Ask once at start; macOS shows its own dialog when the permission is missing.
    accessibility_ok(true);

    let (id, name) = {
        let c = sh.cfg();
        (c.device_id.clone(), c.device_name.clone())
    };
    match discovery::advertise(&id, &name, PORT) {
        Ok(adv) => std::mem::forget(adv), // keep announcing for the life of the app
        Err(e) => sh.say(format!("局域网广播不可用（{e}），Windows 可以用 IP 地址配对")),
    }

    let listener = match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(l) => l,
        Err(e) => {
            sh.say(format!("端口 {PORT} 无法使用（{e}）。是不是已经开着一个 KV Bridge？"));
            return;
        }
    };
    thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let (app, sh) = (app.clone(), sh.clone());
            thread::spawn(move || handle(app, sh, s));
        }
    });
}

/// The pairing code currently shown, if still valid.
fn pairing_code(sh: &Shared) -> Option<String> {
    let mut l = sh.live();
    match &l.pairing {
        Some((code, until)) if Instant::now() < *until => Some(code.clone()),
        Some(_) => {
            l.pairing = None;
            None
        }
        None => None,
    }
}

fn handle(app: AppHandle, sh: Arc<Shared>, mut s: TcpStream) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let ip = s.peer_addr().map(|a| a.ip().to_string()).ok();
    let Ok(h) = Hello::read_from(&mut s) else { return };
    let (my_id, my_name) = {
        let c = sh.cfg();
        (c.device_id.clone(), c.device_name.clone())
    };
    match h {
        Hello::Pair { id, name } => {
            let Some(code) = pairing_code(&sh) else {
                let _ = hello::refuse(
                    &mut s,
                    "这台 Mac 现在不在配对模式。请先在 Mac 的 KV Bridge 里点「配对新电脑」。",
                );
                sh.say(format!("{name} 想配对。请点「配对新电脑」，再在 {name} 上输入显示的配对码。"));
                return;
            };
            if hello::accept(&mut s, &my_id, &my_name).is_err() {
                return;
            }
            let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
            match pair::as_server(&mut s, &code, &my_id, &id) {
                Ok(p) => {
                    sh.update_cfg(|c| {
                        c.upsert_peer(Peer { id, name: name.clone(), psk: to_hex(&p.psk), last_addr: ip })
                    });
                    sh.live().pairing = None;
                    sh.say(format!("已和 {name} 配对，正在等待它连接…"));
                }
                Err(PairError::WrongCode) => sh.say(format!("{name} 输入的配对码不对")),
                Err(PairError::Io(e)) => sh.say(format!("与 {name} 配对中断：{e}")),
            }
        }
        Hello::Connect { id, name } => {
            let peer = sh.cfg().peer(&id).cloned();
            let Some(peer) = peer else {
                let _ = hello::refuse(&mut s, "这台 Mac 还没有和这台电脑配对，请重新配对。");
                sh.say(format!("{name} 想连接，但还没有配对"));
                return;
            };
            let Some(psk) = from_hex32(&peer.psk) else { return };
            if hello::accept(&mut s, &my_id, &my_name).is_err() {
                return;
            }
            match secure::server(s, &psk) {
                Ok((tx, rx)) => session(&app, &sh, &peer, tx, rx),
                Err(_) => sh.say(format!("与 {} 的加密握手失败，请删除后重新配对", peer.name)),
            }
        }
    }
}

fn session(app: &AppHandle, sh: &Arc<Shared>, peer: &Peer, mut tx: secure::Sender, mut rx: secure::Receiver) {
    let _ = rx.stream().set_read_timeout(Some(PEER_TIMEOUT));
    // A newer connection from Windows replaces any older one.
    let my_sid = {
        let mut l = sh.live();
        if let Some((_, old)) = l.session.take() {
            let _ = old.shutdown(Shutdown::Both);
        }
        l.session_counter += 1;
        let sid = l.session_counter;
        if let Ok(s) = rx.stream().try_clone() {
            l.session = Some((sid, s));
        }
        l.connected = Some(peer.name.clone());
        l.controlling = false;
        l.message = format!("已连接 {}。在 Windows 上按 Lift 的按键即可控制这台 Mac。", peer.name);
        sid
    };
    tray::refresh(app, sh);

    // Heartbeats let Windows notice quickly if this Mac sleeps or drops off Wi-Fi.
    thread::spawn(move || loop {
        thread::sleep(HEARTBEAT);
        if tx.send(&[kvm_core::protocol::Msg::Heartbeat]).is_err() {
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

    let current = {
        let mut l = sh.live();
        let current = matches!(l.session, Some((sid, _)) if sid == my_sid);
        if current {
            l.session = None;
            l.connected = None;
            l.controlling = false;
            l.message = format!("与 {} 的连接断开了，等待它重新连接…", peer.name);
        }
        current
    };
    if current {
        tray::refresh(app, sh);
    }
}

pub fn start_pairing(sh: &Shared) -> String {
    let code = kvm_net::new_pairing_code();
    sh.live().pairing = Some((code.clone(), Instant::now() + PAIRING_TIME));
    code
}
