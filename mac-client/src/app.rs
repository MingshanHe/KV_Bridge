//! Network loop and CoreGraphics event posting for kvm-mac.

use crate::ffi::*;
use crate::state::{Injector, MouseKind, Out, Rect};
use crate::Options;
use kvm_core::protocol::{Msg, VERSION};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::thread;
use std::time::{Duration, Instant};

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_secs(2);

pub fn run(opts: Options) {
    if !accessibility_ok() {
        eprintln!(
            "kvm-mac needs the Accessibility permission to type and click.\n\
             System Settings has been asked to show the prompt. Then:\n  \
             System Settings > Privacy & Security > Accessibility\n  \
             turn on the app you ran this from (e.g. Terminal), then run kvm-mac again."
        );
        std::process::exit(1);
    }

    let poster = Poster::new();
    let start = Instant::now();
    let now_ms = move || start.elapsed().as_millis() as u64;

    if opts.test_type {
        test_type(&poster);
        return;
    }

    let host = opts.host.clone().expect("host checked in parse_args");
    let addr = match resolve(&host, opts.port) {
        Some(a) => a,
        None => {
            eprintln!("error: cannot resolve '{host}'");
            std::process::exit(2);
        }
    };

    println!("kvm-mac {}", env!("CARGO_PKG_VERSION"));
    let mut inj = Injector::new(opts.settings.clone());
    loop {
        println!("Connecting to {addr} ...");
        match TcpStream::connect_timeout(&addr, PEER_TIMEOUT) {
            Ok(stream) => {
                println!("Connected. Press the Lift button on Windows to control this Mac.");
                let reason = session(stream, &mut inj, &poster, opts.verbose, &now_ms);
                for o in inj.release_all() {
                    poster.post(&o);
                }
                println!("Disconnected: {reason}");
            }
            Err(e) => println!("Not reachable ({e}). Is kvm-win running? Retrying..."),
        }
        thread::sleep(RETRY);
    }
}

fn resolve(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port).to_socket_addrs().ok()?.find(|a| a.is_ipv4()).or_else(|| {
        (host, port).to_socket_addrs().ok()?.next()
    })
}

fn session(
    mut stream: TcpStream,
    inj: &mut Injector,
    poster: &Poster,
    verbose: bool,
    now_ms: &dyn Fn() -> u64,
) -> String {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(PEER_TIMEOUT));
    if let Err(e) = Msg::hello().write_to(&mut stream) {
        return e.to_string();
    }

    // Heartbeats let Windows notice quickly if this Mac sleeps or drops off Wi-Fi.
    if let Ok(mut ws) = stream.try_clone() {
        thread::spawn(move || loop {
            thread::sleep(HEARTBEAT);
            if Msg::Heartbeat.write_to(&mut ws).is_err() {
                break;
            }
        });
    }

    match Msg::read_from(&mut stream) {
        Ok(Msg::Hello { version }) if version == VERSION => {}
        Ok(Msg::Hello { version }) => {
            let _ = stream.shutdown(Shutdown::Both);
            return format!("kvm-win speaks protocol v{version}, this is v{VERSION}; update both");
        }
        Ok(other) => return format!("unexpected first message {other:?}"),
        Err(e) => return e.to_string(),
    }

    let mut awake: Option<Child> = None;
    let reason = loop {
        let msg = match Msg::read_from(&mut stream) {
            Ok(m) => m,
            Err(e) if is_timeout(&e) => break "no heartbeat from Windows".to_string(),
            Err(e) => break e.to_string(),
        };
        if verbose && msg != Msg::Heartbeat {
            println!("  {msg:?}");
        }
        match msg {
            Msg::Enter => {
                let bounds = screen_bounds();
                // Start every session with the cursor visible in the middle of the
                // main screen, so it's always easy to find.
                let (x, y) = main_screen_center();
                if verbose {
                    let (cx, cy) = cursor_position();
                    println!(
                        "  cursor was at ({cx:.0}, {cy:.0}); moving it to ({x:.0}, {y:.0}). \
                         Screen area {:.0}x{:.0} at ({:.0}, {:.0})",
                        bounds.w, bounds.h, bounds.x, bounds.y
                    );
                }
                inj.sync_pointer(x, y, bounds);
                poster.show_cursor_at(x, y);
                // Keep the display on while it's being driven remotely.
                awake = keep_awake();
                println!(">>> Windows keyboard and mouse are controlling this Mac");
            }
            Msg::Leave => {
                for o in inj.handle(msg, now_ms()) {
                    poster.post(&o);
                }
                stop(&mut awake);
                println!("<<< Control returned to Windows");
            }
            other => {
                for o in inj.handle(other, now_ms()) {
                    poster.post(&o);
                }
            }
        }
    };
    stop(&mut awake);
    let _ = stream.shutdown(Shutdown::Both);
    reason
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// `caffeinate` exits on its own if this process dies (-w).
fn keep_awake() -> Option<Child> {
    Command::new("caffeinate")
        .args(["-di", "-w", &std::process::id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn stop(c: &mut Option<Child>) {
    if let Some(mut child) = c.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Checks the Accessibility permission; when missing, macOS shows its own prompt.
fn accessibility_ok() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let opts = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as *const _,
            &kCFTypeDictionaryValueCallBacks as *const u8 as *const _,
        );
        let trusted = AXIsProcessTrustedWithOptions(opts) != 0;
        if !opts.is_null() {
            CFRelease(opts);
        }
        trusted
    }
}

fn cursor_position() -> (f64, f64) {
    unsafe {
        let e = CGEventCreate(null_mut());
        if e.is_null() {
            return (0.0, 0.0);
        }
        let p = CGEventGetLocation(e);
        CFRelease(e as CFTypeRef);
        (p.x, p.y)
    }
}

/// The area covered by all active displays (global coordinates, top-left origin).
fn screen_bounds() -> Rect {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    let ok = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } == 0;
    if !ok || n == 0 {
        return Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 };
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &id in &ids[..n as usize] {
        let r = unsafe { CGDisplayBounds(id) };
        x0 = x0.min(r.origin.x);
        y0 = y0.min(r.origin.y);
        x1 = x1.max(r.origin.x + r.size.width);
        y1 = y1.max(r.origin.y + r.size.height);
    }
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

fn main_screen_center() -> (f64, f64) {
    let r = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    if r.size.width < 1.0 || r.size.height < 1.0 {
        return cursor_position();
    }
    (r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0)
}

struct Poster {
    src: CGEventSourceRef,
}

impl Poster {
    fn new() -> Self {
        let src = unsafe { CGEventSourceCreate(kCGEventSourceStateHIDSystemState) };
        unsafe {
            if !src.is_null() {
                // Don't freeze the Mac's own trackpad after each warp.
                CGEventSourceSetLocalEventsSuppressionInterval(src, 0.0);
            }
            CGAssociateMouseAndMouseCursorPosition(1);
        }
        Poster { src }
    }

    /// Put the cursor at (x, y), make sure it isn't hidden, and tell apps it moved.
    fn show_cursor_at(&self, x: f64, y: f64) {
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(1);
            CGWarpMouseCursorPosition(CGPoint { x, y });
            CGDisplayShowCursor(CGMainDisplayID());
        }
        self.post(&Out::Mouse {
            kind: MouseKind::Moved,
            button: 0,
            x,
            y,
            dx: 0,
            dy: 0,
            clicks: 0,
            flags: crate::state::flag::NON_COALESCED,
        });
    }

    fn post(&self, out: &Out) {
        unsafe {
            let e = match *out {
                Out::Key { code, down, flags, modifier, autorepeat } => {
                    let e = CGEventCreateKeyboardEvent(self.src, code, down);
                    if e.is_null() {
                        return;
                    }
                    if modifier {
                        CGEventSetType(e, kCGEventFlagsChanged);
                    }
                    CGEventSetFlags(e, flags);
                    if autorepeat {
                        CGEventSetIntegerValueField(e, kCGKeyboardEventAutorepeat, 1);
                    }
                    e
                }
                Out::Mouse { kind, button, x, y, dx, dy, clicks, flags } => {
                    let t = mouse_type(kind, button);
                    let cg_button = button.min(2); // 0 left, 1 right, 2 = "other"
                    let e = CGEventCreateMouseEvent(self.src, t, CGPoint { x, y }, cg_button);
                    if e.is_null() {
                        return;
                    }
                    if button >= 2 {
                        CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, button as i64);
                    }
                    if clicks > 0 {
                        CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks);
                    }
                    if matches!(kind, MouseKind::Moved | MouseKind::Dragged) {
                        CGEventSetIntegerValueField(e, kCGMouseEventDeltaX, dx);
                        CGEventSetIntegerValueField(e, kCGMouseEventDeltaY, dy);
                        // Move the real cursor directly too; a posted event alone
                        // doesn't always move it.
                        CGWarpMouseCursorPosition(CGPoint { x, y });
                    }
                    CGEventSetFlags(e, flags);
                    e
                }
                Out::Scroll { dy, dx } => {
                    let e = CGEventCreateScrollWheelEvent2(self.src, kCGScrollEventUnitPixel, 2, dy, dx, 0);
                    if e.is_null() {
                        return;
                    }
                    e
                }
            };
            CGEventPost(kCGHIDEventTap, e);
            CFRelease(e as CFTypeRef);
        }
    }
}

fn mouse_type(kind: MouseKind, button: u32) -> u32 {
    match (kind, button) {
        (MouseKind::Moved, _) => kCGEventMouseMoved,
        (MouseKind::Down, 0) => kCGEventLeftMouseDown,
        (MouseKind::Up, 0) => kCGEventLeftMouseUp,
        (MouseKind::Dragged, 0) => kCGEventLeftMouseDragged,
        (MouseKind::Down, 1) => kCGEventRightMouseDown,
        (MouseKind::Up, 1) => kCGEventRightMouseUp,
        (MouseKind::Dragged, 1) => kCGEventRightMouseDragged,
        (MouseKind::Down, _) => kCGEventOtherMouseDown,
        (MouseKind::Up, _) => kCGEventOtherMouseUp,
        (MouseKind::Dragged, _) => kCGEventOtherMouseDragged,
    }
}

/// Milestone-1 check: types a sentence into whatever app is focused.
fn test_type(poster: &Poster) {
    use crate::state::Settings;
    println!("Click into a text field (e.g. TextEdit). Typing in 3 seconds...");
    thread::sleep(Duration::from_secs(3));
    let mut inj = Injector::new(Settings::default());
    for c in "hello from kvm bridge".chars() {
        let hid = match c {
            'a'..='z' => 0x04 + (c as u16 - 'a' as u16),
            ' ' => 0x2C,
            _ => continue,
        };
        for msg in [Msg::KeyDown(hid), Msg::KeyUp(hid)] {
            for o in inj.handle(msg, 0) {
                poster.post(&o);
            }
            thread::sleep(Duration::from_millis(15));
        }
    }
    println!("Done. If nothing was typed, check the Accessibility permission.");
}
