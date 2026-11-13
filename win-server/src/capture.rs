//! Keyboard and mouse capture on Windows: low-level hooks, the switching router, and
//! cursor parking. Hook callbacks only lock the router briefly and push to channels;
//! they never block, because Windows silently removes hooks that respond too slowly.

use crate::ffi::*;
use kvm_core::keymap::win_to_hid;
use kvm_core::protocol::{button, Msg};
use kvm_core::router::{Config, Input, Notice, Router};
use std::net::UdpSocket;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

static ROUTER: OnceLock<Mutex<Router>> = OnceLock::new();
static NET_TX: OnceLock<Sender<Msg>> = OnceLock::new();
static UI_TX: OnceLock<Sender<Event>> = OnceLock::new();
static LOG_KEYS: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);

/// What capture reports to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Notice(Notice),
    Log(String),
    /// A key combination recorded after [`start_recording`]: modifier bits
    /// (`protocol::mods`) and the key's HID usage.
    Recorded { mods: u8, key: u16 },
}

/// Channels from a running capture.
pub struct Capture {
    /// Messages to send to the Mac.
    pub outgoing: Receiver<Msg>,
    /// Switches and log lines.
    pub events: Receiver<Event>,
}

fn ui(line: String) {
    if let Some(tx) = UI_TX.get() {
        let _ = tx.send(Event::Log(line));
    }
}

fn notify(n: Notice) {
    if let Some(tx) = UI_TX.get() {
        let _ = tx.send(Event::Notice(n));
    }
}

fn router() -> std::sync::MutexGuard<'static, Router> {
    // A panic while holding the lock must not kill input handling for good.
    ROUTER.get().expect("capture started").lock().unwrap_or_else(|e| e.into_inner())
}

/// Install the hooks (on their own thread) and start routing. Call once per process;
/// later calls return None.
pub fn start(cfg: Config) -> Option<Capture> {
    if ROUTER.set(Mutex::new(Router::new(cfg))).is_err() {
        return None;
    }
    // Without this, a laptop running at 125%/150% display scaling gives us scaled
    // coordinates from GetCursorPos/SetCursorPos but real pixels in the mouse hook.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let (net_tx, outgoing) = channel::<Msg>();
    let (ui_tx, events) = channel::<Event>();
    let _ = NET_TX.set(net_tx);
    let _ = UI_TX.set(ui_tx);

    thread::Builder::new()
        .name("input-hooks".into())
        .spawn(|| unsafe {
            let hmod = GetModuleHandleW(std::ptr::null());
            let kb = SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_proc, hmod, 0);
            let ms = SetWindowsHookExW(WH_MOUSE_LL, mouse_proc, hmod, 0);
            if kb.is_null() || ms.is_null() {
                ui(format!("could not install input hooks (code {})", GetLastError()));
                return;
            }
            // Low-level hooks are delivered through this thread's message loop.
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            UnhookWindowsHookEx(kb);
            UnhookWindowsHookEx(ms);
        })
        .ok()?;
    Some(Capture { outgoing, events })
}

/// Change the toggle button / hotkey while running.
pub fn set_config(cfg: Config) {
    router().set_config(cfg);
}

/// Capture the next key combination instead of passing it on (for choosing a
/// shortcut). The result arrives as [`Event::Recorded`].
pub fn start_recording() {
    RECORDING.store(true, Ordering::SeqCst);
}

pub fn cancel_recording() {
    RECORDING.store(false, Ordering::SeqCst);
}

/// Log every key and button event (for testing).
pub fn set_logging(on: bool) {
    LOG_KEYS.store(on, Ordering::Relaxed);
}

/// Tell capture whether a Mac is connected. Losing it while the Mac has control hands
/// control back to Windows.
pub fn set_connected(connected: bool) {
    let n = router().set_connected(connected);
    if let Some(n) = n {
        unpark_cursor();
        notify(n);
    }
}

/// Whether the Mac currently has control.
pub fn is_remote() -> bool {
    router().mode() == kvm_core::router::Mode::Remote
}

/// This PC's LAN address. Connecting a UDP socket sends no packets; it just asks the
/// OS which local interface would be used.
pub fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// The standard sound for a switch.
pub fn beep(n: &Notice) {
    let kind = match n {
        Notice::EnteredRemote => MB_ICONASTERISK,
        Notice::LeftRemote => MB_OK,
        Notice::NotConnected => MB_ICONHAND,
        Notice::Unmapped(_) => return,
    };
    unsafe {
        MessageBeep(kind);
    }
}

pub fn button_name(b: u8) -> &'static str {
    match b {
        button::BACK => "Back",
        button::FORWARD => "Forward",
        _ => "?",
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

/// The keyboard layout of the app in front (each Windows app can have its own).
fn keyboard_layout() -> u32 {
    unsafe {
        let thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        GetKeyboardLayout(thread) as usize as u32
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
        if decision.notice == Some(Notice::EnteredRemote) {
            // Tell the Mac which layout this keyboard is, so it can translate symbols.
            let hkl = keyboard_layout();
            if LOG_KEYS.load(Ordering::Relaxed) {
                ui(format!("keyboard layout 0x{hkl:08x}"));
            }
            let _ = tx.send(Msg::Layout(hkl));
        }
        for m in decision.send {
            let _ = tx.send(m);
        }
    }
    if let Some(n) = decision.notice {
        notify(n);
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
            // Recording a shortcut: the first non-modifier key press, with whatever
            // modifiers are held, is captured and kept from Windows.
            if down && RECORDING.load(Ordering::SeqCst) {
                if let Some(h) = hid.filter(|&h| !(0xE0..=0xE7).contains(&h)) {
                    RECORDING.store(false, Ordering::SeqCst);
                    let mods = {
                        let mut r = router();
                        r.swallow_release(id);
                        r.held_mods()
                    };
                    if let Some(tx) = UI_TX.get() {
                        let _ = tx.send(Event::Recorded { mods, key: h });
                    }
                    return 1;
                }
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

