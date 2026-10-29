//! Hooks, network and status output for kvm-win.
//!
//! Threads:
//! * main  – installs the low-level hooks and pumps messages. Hook callbacks only lock
//!   the router briefly and push to channels; they never print or touch the
//!   network, because Windows silently removes hooks that respond too slowly.
//! * net   – accepts the Mac, writes forwarded input, sends heartbeats.
//! * ui    – prints status, beeps, sets the console title.

use crate::ffi::*;
use crate::Options;
use kvm_core::keymap::win_to_hid;
use kvm_core::protocol::{button, Msg};
use kvm_core::router::{Config, Input, Notice, Router};
use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

static ROUTER: OnceLock<Mutex<Router>> = OnceLock::new();
static NET_TX: OnceLock<Sender<Msg>> = OnceLock::new();
static UI_TX: OnceLock<Sender<Ui>> = OnceLock::new();
static LOG_KEYS: AtomicBool = AtomicBool::new(false);

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

enum Ui {
    Notice(Notice),
    Line(String),
}

fn ui(line: String) {
    if let Some(tx) = UI_TX.get() {
        let _ = tx.send(Ui::Line(line));
    }
}

fn router() -> std::sync::MutexGuard<'static, Router> {
    // A panic while holding the lock must not kill input handling for good.
    ROUTER.get().expect("router initialised").lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run(opts: Options) {
    // Without this, a laptop running at 125%/150% display scaling gives us scaled
    // coordinates from GetCursorPos/SetCursorPos but real pixels in the mouse hook,
    // and the mismatch looks like a huge movement on every event (the Mac cursor
    // flies into a corner). Being DPI-aware makes every API use real pixels.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    LOG_KEYS.store(opts.log_keys, Ordering::Relaxed);
    let _ = ROUTER.set(Mutex::new(Router::new(Config {
        toggle_button: opts.toggle_button,
        hotkey: opts.hotkey,
    })));

    let (net_tx, net_rx) = channel::<Msg>();
    let (ui_tx, ui_rx) = channel::<Ui>();
    let _ = NET_TX.set(net_tx);
    let _ = UI_TX.set(ui_tx);

    let quiet = opts.quiet;
    thread::spawn(move || ui_thread(ui_rx, quiet));

    let listener = match TcpListener::bind(("0.0.0.0", opts.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot listen on port {}: {e}", opts.port);
            std::process::exit(1);
        }
    };
    print_banner(&opts);
    let allow = opts.allow;
    thread::spawn(move || net_thread(listener, allow, net_rx));

    unsafe {
        let hmod = GetModuleHandleW(std::ptr::null());
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_proc, hmod, 0);
        let ms = SetWindowsHookExW(WH_MOUSE_LL, mouse_proc, hmod, 0);
        if kb.is_null() || ms.is_null() {
            eprintln!("error: could not install input hooks (code {})", GetLastError());
            std::process::exit(1);
        }

        // Low-level hooks are delivered through this thread's message loop.
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(kb);
        UnhookWindowsHookEx(ms);
    }
}

fn print_banner(opts: &Options) {
    let ip = local_ip().unwrap_or_else(|| "<this PC's IP>".into());
    let switch = match (opts.toggle_button, opts.hotkey) {
        (Some(b), true) => format!("{} thumb button or Ctrl+Alt+M", button_name(b)),
        (Some(b), false) => format!("{} thumb button", button_name(b)),
        (None, _) => "Ctrl+Alt+M".into(),
    };
    println!("kvm-win {}", env!("CARGO_PKG_VERSION"));
    println!("Listening on port {}.", opts.port);
    println!("On the Mac, run:   ./kvm-mac {ip}");
    println!("Switch control:    {switch}");
    println!("Stop:              close this window or press Ctrl+C here (control returns to Windows)");
    println!();
    set_title("kvm-win — waiting for Mac");
}

fn button_name(b: u8) -> &'static str {
    match b {
        button::BACK => "Back",
        button::FORWARD => "Forward",
        _ => "?",
    }
}

/// The address the Mac should connect to. Connecting a UDP socket sends no packets;
/// it just asks the OS which local interface would be used.
fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

fn set_title(t: &str) {
    let w = wide(t);
    unsafe {
        SetConsoleTitleW(w.as_ptr());
    }
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// Where the Windows cursor was before switching to the Mac, restored on the way back.
static PARKED_FROM: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// Park the Windows cursor mid-screen while the Mac has control, so mouse movement
/// is never clipped by a screen edge, and use that spot as the delta anchor.
fn park_cursor(r: &mut Router) {
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p) != 0 {
            *PARKED_FROM.lock().unwrap_or_else(|e| e.into_inner()) = Some((p.x, p.y));
        }
        let (cx, cy) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
        SetCursorPos(cx, cy);
        // Anchor where the cursor really ended up, in the same coordinates the hook uses.
        let mut a = POINT { x: cx, y: cy };
        GetCursorPos(&mut a);
        r.set_anchor(a.x, a.y);
        if LOG_KEYS.load(Ordering::Relaxed) {
            ui(format!("cursor parked at ({}, {}); mouse deltas are measured from here", a.x, a.y));
        }
    }
}

fn unpark_cursor() {
    if let Some((x, y)) = PARKED_FROM.lock().unwrap_or_else(|e| e.into_inner()).take() {
        unsafe {
            SetCursorPos(x, y);
        }
    }
}

/// Run one input event through the router. Returns true to block it from Windows.
fn route(input: Input) -> bool {
    let decision = {
        let mut r = router();
        if let Input::MouseMove { .. } = input {
            if r.mode() == kvm_core::router::Mode::Remote {
                // Measure each movement from where the cursor really is right now
                // (inside the hook, before this move is applied). Robust against
                // display scaling and anything else nudging the cursor.
                let mut c = POINT::default();
                if unsafe { GetCursorPos(&mut c) } != 0 {
                    r.set_anchor(c.x, c.y);
                }
            }
        }
        let d = r.handle(input, now_ms());
        match d.notice {
            Some(Notice::EnteredRemote) => park_cursor(&mut r),
            Some(Notice::LeftRemote) => unpark_cursor(),
            _ => {}
        }
        d
    };
    if LOG_KEYS.load(Ordering::Relaxed) {
        if let Input::MouseMove { x, y } = input {
            log_move(x, y, &decision.send);
        }
    }
    if let Some(tx) = NET_TX.get() {
        for m in decision.send {
            let _ = tx.send(m);
        }
    }
    if let (Some(n), Some(tx)) = (decision.notice, UI_TX.get()) {
        let _ = tx.send(Ui::Notice(n));
    }
    decision.block
}

/// With --log: print roughly every 20th mouse event while on the Mac, enough to see
/// the deltas without flooding the console.
fn log_move(x: i32, y: i32, sent: &[Msg]) {
    use std::sync::atomic::AtomicU32;
    static N: AtomicU32 = AtomicU32::new(0);
    if let Some(Msg::MouseMove { dx, dy }) = sent.first() {
        if N.fetch_add(1, Ordering::Relaxed).is_multiple_of(20) {
            ui(format!("mouse at ({x}, {y}) -> sent dx={dx} dy={dy}"));
        }
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        let msg = wparam as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if down || up {
            let extended = kb.flags & LLKHF_EXTENDED != 0;
            let hid = win_to_hid(kb.scanCode, extended, kb.vkCode);
            // Pair down/up by physical key. Keys without a scan code (some media keys)
            // fall back to their virtual-key code.
            let id = if kb.scanCode == 0 {
                0x1_0000 | kb.vkCode
            } else {
                kb.scanCode | ((extended as u32) << 8)
            };
            if LOG_KEYS.load(Ordering::Relaxed) {
                ui(format!(
                    "key {:<4} scan=0x{:02x}{} vk=0x{:02x} -> {}{}",
                    if down { "down" } else { "up" },
                    kb.scanCode,
                    if extended { " ext" } else { "" },
                    kb.vkCode,
                    hid.map(|h| format!("HID 0x{h:02x}")).unwrap_or_else(|| "unmapped".into()),
                    if kb.flags & LLKHF_INJECTED != 0 { " (injected)" } else { "" },
                ));
            }
            if route(Input::Key { id, hid, down }) {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        let ms = &*(lparam as *const MSLLHOOKSTRUCT);
        let hi = (ms.mouseData >> 16) as u16;
        let input = match wparam as u32 {
            WM_MOUSEMOVE => Some(Input::MouseMove { x: ms.pt.x, y: ms.pt.y }),
            WM_LBUTTONDOWN => Some(Input::Button { button: button::LEFT, down: true }),
            WM_LBUTTONUP => Some(Input::Button { button: button::LEFT, down: false }),
            WM_RBUTTONDOWN => Some(Input::Button { button: button::RIGHT, down: true }),
            WM_RBUTTONUP => Some(Input::Button { button: button::RIGHT, down: false }),
            WM_MBUTTONDOWN => Some(Input::Button { button: button::MIDDLE, down: true }),
            WM_MBUTTONUP => Some(Input::Button { button: button::MIDDLE, down: false }),
            m @ (WM_XBUTTONDOWN | WM_XBUTTONUP) => {
                let b = match hi {
                    XBUTTON1 => Some(button::BACK),
                    XBUTTON2 => Some(button::FORWARD),
                    _ => None,
                };
                b.map(|b| Input::Button { button: b, down: m == WM_XBUTTONDOWN })
            }
            WM_MOUSEWHEEL => Some(Input::Wheel { dx: 0, dy: hi as i16 }),
            WM_MOUSEHWHEEL => Some(Input::Wheel { dx: hi as i16, dy: 0 }),
            _ => None,
        };
        if let Some(input) = input {
            if LOG_KEYS.load(Ordering::Relaxed) {
                if let Input::Button { button: b, down } = input {
                    ui(format!(
                        "mouse button {b} {} ({})",
                        if down { "down" } else { "up" },
                        match b {
                            button::LEFT => "left",
                            button::RIGHT => "right",
                            button::MIDDLE => "middle",
                            button::BACK => "Back thumb button",
                            button::FORWARD => "Forward thumb button",
                            _ => "?",
                        }
                    ));
                }
            }
            if route(input) {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

fn net_thread(listener: TcpListener, allow: Option<std::net::IpAddr>, rx: Receiver<Msg>) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                ui(format!("accept failed: {e}"));
                continue;
            }
        };
        let peer = match stream.peer_addr() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Some(ip) = allow {
            if peer.ip() != ip {
                ui(format!("rejected connection from {} (not --allow {ip})", peer.ip()));
                continue;
            }
        }
        ui(format!("Mac connected from {}", peer.ip()));
        set_title("kvm-win — Mac connected (controlling Windows)");
        serve(stream, &rx);
        if let Some(n) = router().set_connected(false) {
            unpark_cursor();
            if let Some(tx) = UI_TX.get() {
                let _ = tx.send(Ui::Notice(n));
            }
        }
        ui("Mac disconnected; waiting for it to reconnect".into());
        set_title("kvm-win — waiting for Mac");
    }
}

fn serve(mut stream: TcpStream, rx: &Receiver<Msg>) {
    let _ = stream.set_nodelay(true);
    // Drop anything queued while no Mac was attached.
    while rx.try_recv().is_ok() {}

    let alive = Arc::new(AtomicBool::new(true));
    // Reader: the Mac sends heartbeats; silence means it's gone (sleep, Wi-Fi drop),
    // which a writer alone could take minutes to notice.
    if let Ok(mut rs) = stream.try_clone() {
        let alive = alive.clone();
        let _ = rs.set_read_timeout(Some(PEER_TIMEOUT));
        thread::spawn(move || {
            loop {
                match Msg::read_from(&mut rs) {
                    Ok(Msg::Hello { version }) if version != kvm_core::protocol::VERSION => {
                        ui(format!(
                            "Mac app is protocol v{version}, this is v{}; update both",
                            kvm_core::protocol::VERSION
                        ));
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            alive.store(false, Ordering::Relaxed);
            let _ = rs.shutdown(Shutdown::Both);
        });
    }

    if Msg::hello().write_to(&mut stream).is_err() {
        return;
    }
    router().set_connected(true);

    let mut buf = Vec::with_capacity(256);
    while alive.load(Ordering::Relaxed) {
        buf.clear();
        match rx.recv_timeout(HEARTBEAT) {
            Ok(m) => {
                m.encode(&mut buf);
                // Batch whatever else is already queued into one write.
                while let Ok(m) = rx.try_recv() {
                    m.encode(&mut buf);
                }
            }
            Err(RecvTimeoutError::Timeout) => Msg::Heartbeat.encode(&mut buf),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if stream.write_all(&buf).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn ui_thread(rx: Receiver<Ui>, quiet: bool) {
    for ev in rx {
        match ev {
            Ui::Line(s) => println!("{s}"),
            Ui::Notice(n) => match n {
                Notice::EnteredRemote => {
                    println!(">>> Controlling the MAC");
                    set_title("kvm-win — controlling MAC");
                    if !quiet {
                        unsafe { MessageBeep(MB_ICONASTERISK) };
                    }
                }
                Notice::LeftRemote => {
                    println!("<<< Controlling WINDOWS");
                    set_title("kvm-win — Mac connected (controlling Windows)");
                    if !quiet {
                        unsafe { MessageBeep(MB_OK) };
                    }
                }
                Notice::NotConnected => {
                    println!("No Mac connected yet; staying on Windows");
                    if !quiet {
                        unsafe { MessageBeep(MB_ICONHAND) };
                    }
                }
                Notice::Unmapped(id) => println!("(key 0x{id:x} has no Mac equivalent; ignored)"),
            },
        }
    }
}
