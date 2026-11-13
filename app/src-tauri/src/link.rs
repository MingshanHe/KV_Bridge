//! Connections between the computers, the same on both sides.
//!
//! Each app announces itself on the LAN (mDNS), listens on [`PORT`], and dials the
//! computers it is paired with. Whichever direction gets through first carries the
//! session; a second one is refused while a session is up. This way a Mac whose
//! firewall blocks incoming connections (common on company Macs) can still connect
//! out to Windows, and the other way round.
//!
//! Pairing works from either side too: one computer shows a 6-digit code, the other
//! types it and connects.

use crate::config::Peer;
use crate::shared::Shared;
use crate::tray;
use kvm_net::hello::{self, Hello};
use kvm_net::pair::{self, PairError};
use kvm_net::{discovery, from_hex32, secure, to_hex, PORT};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

#[cfg(target_os = "macos")]
use crate::mac_side as plat;
#[cfg(windows)]
use crate::win_side as plat;

/// This computer's role on the network.
pub const ROLE: &str = if cfg!(windows) { "windows" } else { "mac" };
/// The role of the computers we pair with.
pub const OTHER_ROLE: &str = if cfg!(windows) { "mac" } else { "windows" };
/// How long a pairing code stays valid.
const PAIRING_TIME: Duration = Duration::from_secs(300);

fn identity(sh: &Shared) -> (String, String) {
    let c = sh.cfg();
    (c.device_id.clone(), c.device_name.clone())
}

pub fn start(app: AppHandle, sh: Arc<Shared>) {
    let (id, name) = identity(&sh);
    match discovery::advertise(&id, &name, ROLE, PORT) {
        Ok(adv) => std::mem::forget(adv), // announce for the life of the app
        Err(e) => sh.say(format!("局域网广播不可用（{e}），可以用 IP 地址配对")),
    }
    match discovery::browse() {
        Ok((browser, rx)) => {
            std::mem::forget(browser);
            let sh = sh.clone();
            let my_id = id.clone();
            thread::spawn(move || {
                for f in rx {
                    if f.role == OTHER_ROLE && f.id != my_id {
                        sh.live().found.insert(f.id.clone(), f);
                    }
                }
            });
        }
        Err(e) => sh.say(format!("局域网自动发现不可用（{e}），可以用 IP 地址配对")),
    }

    match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(listener) => {
            let (app, sh) = (app.clone(), sh.clone());
            thread::spawn(move || {
                for s in listener.incoming().flatten() {
                    let (app, sh) = (app.clone(), sh.clone());
                    thread::spawn(move || incoming(app, sh, s));
                }
            });
        }
        Err(e) => {
            sh.live().listen_error =
                Some(format!("端口 {PORT} 无法使用（{e}），只能由这台电脑去连接对方"));
        }
    }

    thread::spawn(move || dialer(app, sh));
}

/// Claim the single session slot. None if a session is already running.
fn claim(sh: &Shared, peer: &Peer, stream: &TcpStream) -> Option<u64> {
    let mut l = sh.live();
    if l.session.is_some() {
        return None;
    }
    l.session_counter += 1;
    let sid = l.session_counter;
    l.session = Some((sid, stream.try_clone().ok()?));
    l.connected = Some(peer.name.clone());
    l.connected_id = Some(peer.id.clone());
    l.controlling = false;
    l.message = if cfg!(windows) {
        format!("已连接到 {}。按 Lift 的按键或切换快捷键即可控制它。", peer.name)
    } else {
        format!("已连接 {}。在 Windows 上按 Lift 的按键或切换快捷键即可控制这台 Mac。", peer.name)
    };
    Some(sid)
}

fn release(app: &AppHandle, sh: &Shared, sid: u64, peer: &Peer) {
    {
        let mut l = sh.live();
        if !matches!(l.session, Some((s, _)) if s == sid) {
            return;
        }
        l.session = None;
        l.connected = None;
        l.connected_id = None;
        l.controlling = false;
        l.message = if sh.cfg().peer(&peer.id).is_some() {
            format!("与 {} 的连接断开了，正在重新连接…", peer.name)
        } else {
            format!("已删除 {}", peer.name)
        };
    }
    tray::refresh(app, sh);
}

fn remember_addr(sh: &Shared, peer: &Peer, ip: IpAddr) {
    let ip = ip.to_string();
    if peer.last_addr.as_deref() != Some(ip.as_str()) {
        sh.update_cfg(|c| {
            if let Some(p) = c.peers.iter_mut().find(|p| p.id == peer.id) {
                p.last_addr = Some(ip);
            }
        });
    }
}

/// Run a session that has been claimed, then free the slot.
fn run(app: &AppHandle, sh: &Arc<Shared>, sid: u64, peer: &Peer, tx: secure::Sender, rx: secure::Receiver) {
    tray::refresh(app, sh);
    plat::run_session(app, sh, peer, tx, rx);
    release(app, sh, sid, peer);
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

pub fn start_pairing(sh: &Shared) -> String {
    let code = kvm_net::new_pairing_code();
    sh.live().pairing = Some((code.clone(), Instant::now() + PAIRING_TIME));
    code
}

fn incoming(app: AppHandle, sh: Arc<Shared>, mut s: TcpStream) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let ip = s.peer_addr().map(|a| a.ip()).ok();
    let Ok(h) = Hello::read_from(&mut s) else { return };
    let (my_id, my_name) = identity(&sh);
    match h {
        Hello::Pair { id, name } => {
            let Some(code) = pairing_code(&sh) else {
                let _ = hello::refuse(
                    &mut s,
                    "对方现在不在配对模式。请先在对方的 KV Bridge 里点「显示配对码」。",
                );
                sh.say(format!("{name} 想配对。请点「显示配对码」，再在 {name} 上输入。"));
                return;
            };
            if hello::accept(&mut s, &my_id, &my_name).is_err() {
                return;
            }
            let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
            match pair::as_server(&mut s, &code, &my_id, &id) {
                Ok(p) => {
                    sh.update_cfg(|c| {
                        c.upsert_peer(Peer {
                            id,
                            name: name.clone(),
                            psk: to_hex(&p.psk),
                            last_addr: ip.map(|i| i.to_string()),
                        })
                    });
                    sh.live().pairing = None;
                    sh.say(format!("已和 {name} 配对，正在连接…"));
                }
                Err(PairError::WrongCode) => sh.say(format!("{name} 输入的配对码不对")),
                Err(PairError::Io(e)) => sh.say(format!("与 {name} 配对中断：{e}")),
            }
        }
        Hello::Connect { id, name } => {
            let peer = sh.cfg().peer(&id).cloned();
            let Some(peer) = peer else {
                let _ = hello::refuse(&mut s, "对方还没有和这台电脑配对，请重新配对。");
                sh.say(format!("{name} 想连接，但还没有配对"));
                return;
            };
            let Some(psk) = from_hex32(&peer.psk) else { return };
            let Some(sid) = claim(&sh, &peer, &s) else {
                let _ = hello::refuse(&mut s, "已经连接着");
                return;
            };
            if hello::accept(&mut s, &my_id, &my_name).is_err() {
                release(&app, &sh, sid, &peer);
                return;
            }
            match secure::server(s, &psk) {
                Ok((tx, rx)) => {
                    if let Some(ip) = ip {
                        remember_addr(&sh, &peer, ip);
                    }
                    run(&app, &sh, sid, &peer, tx, rx);
                }
                Err(_) => {
                    release(&app, &sh, sid, &peer);
                    sh.say(format!("与 {} 的加密握手失败，请删除后重新配对", peer.name));
                }
            }
        }
    }
}

/// Addresses to try for a paired computer: where it was just seen, then where it
/// was last reached.
fn candidates(sh: &Shared, peer: &Peer) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = Vec::new();
    if let Some(f) = sh.live().found.get(&peer.id) {
        v.extend(f.addrs.iter().map(|ip| SocketAddr::new(*ip, f.port)));
    }
    if let Some(ip) = peer.last_addr.as_deref().and_then(|a| a.parse::<IpAddr>().ok()) {
        let a = SocketAddr::new(ip, PORT);
        if !v.contains(&a) {
            v.push(a);
        }
    }
    v
}

fn dial(addr: SocketAddr, sh: &Shared, peer: &Peer, psk: &[u8; 32]) -> Result<(TcpStream, secure::Sender, secure::Receiver), String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (id, name) = identity(sh);
    Hello::Connect { id, name }.write_to(&mut s).map_err(|e| e.to_string())?;
    match hello::read_answer(&mut s).map_err(|e| e.to_string())? {
        Err(reason) => return Err(reason),
        Ok((pid, _)) if pid != peer.id => return Err("这个地址上是另一台电脑".into()),
        Ok(_) => {}
    }
    let keep = s.try_clone().map_err(|e| e.to_string())?;
    let (tx, rx) = secure::client(s, psk).map_err(|_| "加密握手失败，配对可能已失效，请重新配对".to_string())?;
    Ok((keep, tx, rx))
}

fn dialer(app: AppHandle, sh: Arc<Shared>) {
    // The Mac waits a little longer before dialing, so the two sides don't keep
    // connecting to each other at the same moment.
    let pause = Duration::from_millis(if cfg!(windows) { 2000 } else { 3500 });
    loop {
        thread::sleep(pause);
        if sh.live().session.is_some() {
            continue;
        }
        let peers = sh.cfg().peers.clone();
        let mut errors = Vec::new();
        'peers: for peer in &peers {
            let Some(psk) = from_hex32(&peer.psk) else { continue };
            let addrs = candidates(&sh, peer);
            if addrs.is_empty() {
                errors.push(format!("还没在网络上找到 {}", peer.name));
            }
            for addr in addrs {
                match dial(addr, &sh, peer, &psk) {
                    Ok((stream, tx, rx)) => {
                        let Some(sid) = claim(&sh, peer, &stream) else {
                            // The other side connected to us in the meantime.
                            let _ = stream.shutdown(Shutdown::Both);
                            break 'peers;
                        };
                        remember_addr(&sh, peer, addr.ip());
                        run(&app, &sh, sid, peer, tx, rx);
                        break 'peers;
                    }
                    Err(e) if e == "已经连接着" => break 'peers,
                    Err(e) => errors.push(format!("连接 {}（{}）失败：{e}", peer.name, addr.ip())),
                }
            }
        }
        if !errors.is_empty() && sh.live().session.is_none() {
            sh.say(errors.join("；"));
        }
    }
}

/// Pair with a computer by its id (seen on the network) or an IP address, using the
/// code shown on it.
pub fn pair_with(sh: &Shared, target: &str, code: &str) -> Result<String, String> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return Err("请输入对方显示的 6 位数字".into());
    }
    let found = sh.live().found.get(target).cloned();
    let addrs: Vec<SocketAddr> = match found {
        Some(f) => f.addrs.iter().map(|ip| SocketAddr::new(*ip, f.port)).collect(),
        None => {
            let t = target.trim();
            if let Ok(a) = t.parse::<SocketAddr>() {
                vec![a]
            } else if let Ok(ip) = t.parse::<IpAddr>() {
                vec![SocketAddr::new(ip, PORT)]
            } else {
                return Err("找不到这台电脑。请确认两台电脑在同一个网络，或者输入对方的 IP 地址".into());
            }
        }
    };
    let (my_id, my_name) = identity(sh);
    let mut last_err = String::from("连不上对方");
    for addr in addrs {
        let mut s = match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(s) => s,
            Err(e) => {
                last_err = format!(
                    "连不上 {addr}：{e}。对方的 KV Bridge 打开了吗？如果对方的防火墙拦截了连接，\
                     可以反过来：在这台电脑上点「显示配对码」，到对方那边输入。"
                );
                continue;
            }
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
        Hello::Pair { id: my_id.clone(), name: my_name.clone() }
            .write_to(&mut s)
            .map_err(|e| e.to_string())?;
        let (peer_id, peer_name) = match hello::read_answer(&mut s).map_err(|e| e.to_string())? {
            Ok(x) => x,
            Err(reason) => return Err(reason),
        };
        return match pair::as_client(&mut s, &code, &my_id, &peer_id) {
            Ok(p) => {
                sh.update_cfg(|c| {
                    c.upsert_peer(Peer {
                        id: peer_id,
                        name: peer_name.clone(),
                        psk: to_hex(&p.psk),
                        last_addr: Some(addr.ip().to_string()),
                    })
                });
                sh.say(format!("已和 {peer_name} 配对，正在连接…"));
                Ok(peer_name)
            }
            Err(PairError::WrongCode) => Err("配对码不对，请核对对方显示的 6 位数字".into()),
            Err(PairError::Io(e)) => Err(format!("配对中断：{e}")),
        };
    }
    Err(last_err)
}
