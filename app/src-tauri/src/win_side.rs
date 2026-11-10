//! Windows: capture the keyboard and mouse, find the paired Mac, keep a session open.

use crate::config::Peer;
use crate::shared::Shared;
use crate::tray;
use kvm_core::protocol::Msg;
use kvm_core::router::Notice;
use kvm_net::hello::{self, Hello};
use kvm_net::pair::{self, PairError};
use kvm_net::{discovery, from_hex32, secure, to_hex, PORT};
use kvm_win::capture::{self, Event};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn start(app: AppHandle, sh: Arc<Shared>) {
    let rc = sh.cfg().win.router_config();
    let Some(cap) = capture::start(rc) else {
        sh.say("键盘鼠标捕获启动失败");
        return;
    };

    // Switches and log lines from the hooks.
    {
        let (app, sh) = (app.clone(), sh.clone());
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
                    Event::Log(l) => sh.say(l),
                }
            }
        });
    }

    // Find Macs on the network.
    match discovery::browse() {
        Ok((browser, rx)) => {
            std::mem::forget(browser); // keep browsing for the life of the app
            let sh = sh.clone();
            thread::spawn(move || {
                for f in rx {
                    sh.live().found.insert(f.id.clone(), f);
                }
            });
        }
        Err(e) => sh.say(format!("局域网自动发现不可用（{e}），可以用 IP 地址配对")),
    }

    let outgoing = cap.outgoing;
    thread::spawn(move || connector(app, sh, outgoing));
}

/// Addresses to try for a paired Mac: where it was just seen, then where it was last.
fn candidates(sh: &Shared, peer: &Peer) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = Vec::new();
    if let Some(f) = sh.live().found.get(&peer.id) {
        v.extend(f.addrs.iter().map(|ip| SocketAddr::new(*ip, f.port)));
    }
    if let Some(ip) = peer.last_addr.as_deref().and_then(|a| a.parse::<IpAddr>().ok()) {
        v.push(SocketAddr::new(ip, PORT));
    }
    v.dedup();
    v
}

fn connector(app: AppHandle, sh: Arc<Shared>, outgoing: Receiver<Msg>) {
    let mut gen = sh.generation();
    loop {
        apply_settings(&sh, &mut gen);
        let peer = sh.cfg().peers.first().cloned();
        let Some(peer) = peer else {
            thread::sleep(Duration::from_secs(1));
            continue;
        };
        let Some(psk) = from_hex32(&peer.psk) else {
            sh.say("配对信息损坏，请删除后重新配对");
            thread::sleep(Duration::from_secs(3));
            continue;
        };
        for addr in candidates(&sh, &peer) {
            match open(addr, &sh, &peer, &psk) {
                Ok((tx, rx)) => {
                    let ip = addr.ip().to_string();
                    if peer.last_addr.as_deref() != Some(ip.as_str()) {
                        sh.update_cfg(|c| {
                            if let Some(p) = c.peers.iter_mut().find(|p| p.id == peer.id) {
                                p.last_addr = Some(ip);
                            }
                        });
                    }
                    session(&app, &sh, &peer, tx, rx, &outgoing, &mut gen);
                    break;
                }
                Err(e) => sh.say(format!("连接 {} 失败：{e}", peer.name)),
            }
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn apply_settings(sh: &Shared, gen: &mut u64) {
    let g = sh.generation();
    if g != *gen {
        *gen = g;
        capture::set_config(sh.cfg().win.router_config());
    }
}

fn my_identity(sh: &Shared) -> (String, String) {
    let c = sh.cfg();
    (c.device_id.clone(), c.device_name.clone())
}

fn open(
    addr: SocketAddr,
    sh: &Shared,
    peer: &Peer,
    psk: &[u8; 32],
) -> Result<(secure::Sender, secure::Receiver), String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (id, name) = my_identity(sh);
    Hello::Connect { id, name }.write_to(&mut s).map_err(|e| e.to_string())?;
    match hello::read_answer(&mut s).map_err(|e| e.to_string())? {
        Err(reason) => return Err(reason),
        Ok((mid, _)) if mid != peer.id => return Err("这个地址上是另一台 Mac".into()),
        Ok(_) => {}
    }
    secure::client(s, psk).map_err(|_| "加密握手失败，配对可能已失效，请重新配对".to_string())
}

fn session(
    app: &AppHandle,
    sh: &Arc<Shared>,
    peer: &Peer,
    mut tx: secure::Sender,
    mut rx: secure::Receiver,
    outgoing: &Receiver<Msg>,
    gen: &mut u64,
) {
    let _ = rx.stream().set_read_timeout(Some(PEER_TIMEOUT));
    // Drop anything queued while no Mac was attached.
    while outgoing.try_recv().is_ok() {}
    {
        let mut l = sh.live();
        l.connected = Some(peer.name.clone());
        l.message = format!("已连接到 {}。按 Lift 的按键切换控制。", peer.name);
        // Lets "forget" close this session.
        l.session = tx.stream().try_clone().ok().map(|s| (0, s));
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
            Err(RecvTimeoutError::Timeout) => {
                apply_settings(sh, gen);
                batch.push(Msg::Heartbeat);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if tx.send(&batch).is_err() {
            break;
        }
    }
    tx.shutdown();
    capture::set_connected(false);
    {
        let mut l = sh.live();
        l.connected = None;
        l.controlling = false;
        l.session = None;
        l.message = if sh.cfg().peer(&peer.id).is_some() {
            format!("与 {} 的连接断开了，正在重新连接…", peer.name)
        } else {
            format!("已删除 {}", peer.name)
        };
    }
    tray::refresh(app, sh);
}

/// Pair with a Mac by its id (seen on the network) or an IP address.
pub fn pair(sh: &Shared, target: &str, code: &str) -> Result<String, String> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return Err("请输入 Mac 上显示的 6 位数字".into());
    }
    let addrs: Vec<SocketAddr> = match sh.live().found.get(target) {
        Some(f) => f.addrs.iter().map(|ip| SocketAddr::new(*ip, f.port)).collect(),
        None => {
            let t = target.trim();
            if let Ok(a) = t.parse::<SocketAddr>() {
                vec![a]
            } else if let Ok(ip) = t.parse::<IpAddr>() {
                vec![SocketAddr::new(ip, PORT)]
            } else {
                return Err("找不到这台 Mac。请确认两台电脑在同一个 Wi-Fi，或者输入 Mac 的 IP 地址".into());
            }
        }
    };
    let (my_id, my_name) = my_identity(sh);
    let mut last_err = String::from("连不上这台 Mac");
    for addr in addrs {
        let mut s = match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("连不上 {addr}：{e}。Mac 上的 KV Bridge 打开了吗？");
                continue;
            }
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
        Hello::Pair { id: my_id.clone(), name: my_name.clone() }
            .write_to(&mut s)
            .map_err(|e| e.to_string())?;
        let (mac_id, mac_name) = match hello::read_answer(&mut s).map_err(|e| e.to_string())? {
            Ok(x) => x,
            Err(reason) => return Err(reason),
        };
        return match pair::as_client(&mut s, &code, &my_id, &mac_id) {
            Ok(p) => {
                sh.update_cfg(|c| {
                    c.upsert_peer(Peer {
                        id: mac_id,
                        name: mac_name.clone(),
                        psk: to_hex(&p.psk),
                        last_addr: Some(addr.ip().to_string()),
                    })
                });
                sh.say(format!("已和 {mac_name} 配对，正在连接…"));
                Ok(mac_name)
            }
            Err(PairError::WrongCode) => Err("配对码不对，请核对 Mac 上显示的 6 位数字".into()),
            Err(PairError::Io(e)) => Err(format!("配对中断：{e}")),
        };
    }
    Err(last_err)
}
