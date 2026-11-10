//! macOS side: posting CoreGraphics events, the Accessibility-based context for the
//! shortcut rules, and [`Output`], which turns protocol messages into input on this Mac.

use crate::ffi::*;
use crate::state::{flag, Ctx, Injector, MouseKind, Out, Rect, Settings};
use kvm_core::protocol::Msg;
use std::ffi::c_void;
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::time::Instant;

/// What [`Output`] reports while it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Control arrived. `layout` names the Windows layout symbols are translated from.
    Entered { layout: Option<&'static str> },
    /// Control went back to Windows.
    Left,
}

/// Replays protocol messages as input on this Mac. Create it on the thread that
/// will use it (it holds CoreGraphics and Accessibility objects).
pub struct Output {
    inj: Injector,
    ctx: MacCtx,
    poster: Poster,
    awake: Option<Child>,
    start: Instant,
    verbose: bool,
}

impl Output {
    pub fn new(settings: Settings, verbose: bool) -> Self {
        Output {
            inj: Injector::new(settings),
            ctx: MacCtx::new(verbose),
            poster: Poster::new(),
            awake: None,
            start: Instant::now(),
            verbose,
        }
    }

    pub fn set_settings(&mut self, s: Settings) {
        self.inj.set_settings(s);
    }

    /// Handle one message; returns an event when control moved.
    pub fn handle(&mut self, msg: Msg) -> Option<Event> {
        if self.verbose && msg != Msg::Heartbeat {
            println!("  {msg:?}");
        }
        let now = self.start.elapsed().as_millis() as u64;
        match msg {
            Msg::Enter => {
                let bounds = screen_bounds();
                // Start every session with the cursor visible in the middle of the
                // main screen, so it's always easy to find.
                let (x, y) = main_screen_center();
                if self.verbose {
                    let (cx, cy) = cursor_position();
                    println!(
                        "  cursor was at ({cx:.0}, {cy:.0}); moving it to ({x:.0}, {y:.0}). \
                         Screen area {:.0}x{:.0} at ({:.0}, {:.0})",
                        bounds.w, bounds.h, bounds.x, bounds.y
                    );
                }
                self.inj.sync_pointer(x, y, bounds);
                self.poster.show_cursor_at(x, y);
                // Keep the display on while it's being driven remotely.
                stop(&mut self.awake);
                self.awake = keep_awake();
                Some(Event::Entered { layout: self.inj.win_layout().map(|l| l.name()) })
            }
            Msg::Leave => {
                for o in self.inj.handle(msg, now, &mut self.ctx) {
                    self.poster.post(&o);
                }
                stop(&mut self.awake);
                Some(Event::Left)
            }
            other => {
                for o in self.inj.handle(other, now, &mut self.ctx) {
                    self.poster.post(&o);
                }
                None
            }
        }
    }

    /// The connection ended: release everything held and let the display sleep.
    pub fn reset(&mut self) {
        for o in self.inj.release_all() {
            self.poster.post(&o);
        }
        stop(&mut self.awake);
    }

    /// Post raw events (used by the typing self-test).
    pub fn post(&self, outs: &[Out]) {
        for o in outs {
            self.poster.post(o);
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.reset();
    }
}

extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_sync_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn pthread_main_np() -> i32;
}

/// Run `f` on the main thread and wait for its result. Requires the main thread to be
/// running a run loop (true in the app); on the main thread itself it runs directly.
pub fn on_main<R, F: FnOnce() -> R>(f: F) -> R {
    struct Job<F, R> {
        f: Option<F>,
        r: Option<R>,
    }
    extern "C" fn tramp<R, F: FnOnce() -> R>(p: *mut c_void) {
        let job = unsafe { &mut *(p as *mut Job<F, R>) };
        if let Some(f) = job.f.take() {
            job.r = Some(f());
        }
    }
    unsafe {
        if pthread_main_np() != 0 {
            return f();
        }
        let mut job = Job { f: Some(f), r: None };
        dispatch_sync_f(
            &_dispatch_main_q as *const u8 as *const c_void,
            &mut job as *mut Job<F, R> as *mut c_void,
            tramp::<R, F>,
        );
        job.r.expect("main-thread job ran")
    }
}

/// Open System Settings at Privacy & Security > Accessibility.
pub fn open_accessibility_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

pub fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// `caffeinate` exits on its own if this process dies (-w).
pub fn keep_awake() -> Option<Child> {
    Command::new("caffeinate")
        .args(["-di", "-w", &std::process::id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

pub fn stop(c: &mut Option<Child>) {
    if let Some(mut child) = c.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Checks the Accessibility permission. With `prompt`, macOS shows its own dialog
/// when it is missing.
pub fn accessibility_ok(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrustedWithOptions(std::ptr::null()) != 0;
        }
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

pub fn cursor_position() -> (f64, f64) {
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
pub fn screen_bounds() -> Rect {
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

/// Terminal apps, matched against the frontmost app's executable path.
const TERMINALS: &[&str] = &[
    "/terminal.app/",
    "/iterm.app/",
    "/iterm2.app/",
    "/warp.app/",
    "/alacritty.app/",
    "/kitty.app/",
    "/wezterm.app/",
    "/ghostty.app/",
    "/hyper.app/",
    "/tabby.app/",
    "/rio.app/",
];

/// Answers the shortcut rules' questions using the Accessibility API
/// (the same permission kvm-mac already needs to type).
pub struct MacCtx {
    system: AXUIElementRef,
    attr_focused_app: CFStringRef,
    attr_focused_element: CFStringRef,
    attr_selected_text: CFStringRef,
    verbose: bool,
    last_app: String,
    keys: KeyMap,
}

/// Character -> Mac key, for the current keyboard layout. Rebuilt whenever the
/// input source or keyboard type changes.
#[derive(Default)]
struct KeyMap {
    source: usize,
    kbd_type: u32,
    map: std::collections::HashMap<char, (u16, bool, bool)>,
}

/// Keypad and control keys: never used to type a symbol.
fn skip_keycode(k: u16) -> bool {
    matches!(k, 0x24 | 0x30 | 0x31 | 0x33 | 0x35 | 0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E)
        || (0x51..=0x5C).contains(&k)
        || k >= 0x5F // keypad comma, F-keys, arrows, 英数/かな
}

impl KeyMap {
    fn refresh(&mut self) {
        // Text Input Source calls must run on the main thread.
        on_main(|| self.refresh_now())
    }

    fn refresh_now(&mut self) {
        unsafe {
            let mut src = TISCopyCurrentKeyboardLayoutInputSource();
            let mut data = if src.is_null() {
                std::ptr::null()
            } else {
                TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData)
            };
            if data.is_null() {
                // Some input methods have no layout data of their own.
                if !src.is_null() {
                    CFRelease(src);
                }
                src = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
                if src.is_null() {
                    return;
                }
                data = TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData);
            }
            let kbd = LMGetKbdType() as u32;
            if !data.is_null() && (data as usize != self.source || kbd != self.kbd_type) {
                self.source = data as usize;
                self.kbd_type = kbd;
                self.build(CFDataGetBytePtr(data) as *const _, kbd);
            }
            CFRelease(src);
        }
    }

    /// Ask the layout what every key types, preferring no modifier, then Shift,
    /// then Option, then Option+Shift. Dead keys are skipped.
    unsafe fn build(&mut self, layout: *const std::ffi::c_void, kbd: u32) {
        self.map.clear();
        // UCKeyTranslate modifier state: (EventModifiers >> 8) & 0xFF.
        let states = [(0u32, false, false), (0x02, true, false), (0x08, false, true), (0x0A, true, true)];
        for (state, shift, option) in states {
            for code in 0u16..0x80 {
                if skip_keycode(code) {
                    continue;
                }
                let mut dead = 0u32;
                let mut len = 0usize;
                let mut buf = [0u16; 4];
                let err = UCKeyTranslate(
                    layout, code, kUCKeyActionDown, state, kbd, 0,
                    &mut dead, buf.len(), &mut len, buf.as_mut_ptr(),
                );
                if err != 0 || dead != 0 || len != 1 {
                    continue;
                }
                if let Some(ch) = char::from_u32(buf[0] as u32) {
                    if !ch.is_control() && ch != ' ' {
                        self.map.entry(ch).or_insert((code, shift, option));
                    }
                }
            }
        }
    }
}

fn cfstr(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).expect("no NUL");
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8) }
}

impl MacCtx {
    pub fn new(verbose: bool) -> Self {
        let system = unsafe { AXUIElementCreateSystemWide() };
        if !system.is_null() {
            // Never let a hung app stall our input handling for long.
            unsafe { AXUIElementSetMessagingTimeout(system, 0.2) };
        }
        MacCtx {
            system,
            attr_focused_app: cfstr("AXFocusedApplication"),
            attr_focused_element: cfstr("AXFocusedUIElement"),
            attr_selected_text: cfstr("AXSelectedText"),
            verbose,
            last_app: String::new(),
            keys: KeyMap::default(),
        }
    }

    /// Copy an attribute; the caller must CFRelease a non-null result.
    fn copy_attr(&self, el: AXUIElementRef, attr: CFStringRef) -> CFTypeRef {
        if el.is_null() || attr.is_null() {
            return std::ptr::null();
        }
        let mut v: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(el, attr, &mut v) };
        if err == kAXErrorSuccess { v } else { std::ptr::null() }
    }

    fn front_app_path(&mut self) -> Option<String> {
        let app = self.copy_attr(self.system, self.attr_focused_app);
        if app.is_null() {
            return None;
        }
        let mut pid = 0i32;
        let ok = unsafe { AXUIElementGetPid(app, &mut pid) } == kAXErrorSuccess;
        unsafe { CFRelease(app) };
        if !ok {
            return None;
        }
        let mut buf = vec![0u8; 4096];
        let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut _, buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        let path = String::from_utf8_lossy(&buf).into_owned();
        if self.verbose && path != self.last_app {
            println!("  front app: {path}");
            self.last_app = path.clone();
        }
        Some(path)
    }
}

impl Ctx for MacCtx {
    fn keystroke_for(&mut self, ch: char) -> Option<(u16, bool, bool)> {
        self.keys.refresh();
        let k = self.keys.map.get(&ch).copied();
        if self.verbose {
            match k {
                Some((code, sh, opt)) => println!(
                    "  '{ch}' -> Mac key 0x{code:02x}{}{}",
                    if sh { " +Shift" } else { "" },
                    if opt { " +Option" } else { "" }
                ),
                None => println!("  '{ch}' -> typed as text (no Mac key for it)"),
            }
        }
        k
    }

    fn front_is_terminal(&mut self) -> bool {
        let Some(path) = self.front_app_path() else { return false };
        let p = path.to_lowercase();
        TERMINALS.iter().any(|t| p.contains(t))
    }

    fn has_selection(&mut self) -> bool {
        let app = self.copy_attr(self.system, self.attr_focused_app);
        if app.is_null() {
            return false;
        }
        let el = self.copy_attr(app, self.attr_focused_element);
        unsafe { CFRelease(app) };
        if el.is_null() {
            return false;
        }
        let text = self.copy_attr(el, self.attr_selected_text);
        unsafe { CFRelease(el) };
        if text.is_null() {
            return false;
        }
        let len = unsafe {
            if CFGetTypeID(text) == CFStringGetTypeID() { CFStringGetLength(text) } else { 0 }
        };
        unsafe { CFRelease(text) };
        if self.verbose {
            println!("  selected text: {len} characters");
        }
        len > 0
    }
}

pub fn main_screen_center() -> (f64, f64) {
    let r = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    if r.size.width < 1.0 || r.size.height < 1.0 {
        return cursor_position();
    }
    (r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0)
}

impl Default for Poster {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Poster {
    src: CGEventSourceRef,
}

impl Poster {
    pub fn new() -> Self {
        let src = unsafe { CGEventSourceCreate(kCGEventSourceStateHIDSystemState) };
        unsafe {
            if !src.is_null() {
                // Don't freeze the Mac's own trackpad after each warp.
                CGEventSourceSetLocalEventsSuppressionInterval(src, 0.0);
                // Same keyboard type (JIS/ISO/ANSI) as this Mac's own keyboard.
                CGEventSourceSetKeyboardType(src, LMGetKbdType() as u32);
            }
            CGAssociateMouseAndMouseCursorPosition(1);
        }
        Poster { src }
    }

    /// Put the cursor at (x, y), make sure it isn't hidden, and tell apps it moved.
    pub fn show_cursor_at(&self, x: f64, y: f64) {
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
            flags: flag::NON_COALESCED,
        });
    }

    pub fn post(&self, out: &Out) {
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
                Out::Text(ch) => {
                    let mut units = [0u16; 2];
                    let units = ch.encode_utf16(&mut units);
                    for down in [true, false] {
                        let e = CGEventCreateKeyboardEvent(self.src, 0, down);
                        if e.is_null() {
                            return;
                        }
                        CGEventSetFlags(e, flag::NON_COALESCED);
                        CGEventKeyboardSetUnicodeString(e, units.len(), units.as_ptr());
                        CGEventPost(kCGHIDEventTap, e);
                        CFRelease(e as CFTypeRef);
                    }
                    return;
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

