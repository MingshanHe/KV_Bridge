//! Turns protocol messages into the exact events to post on macOS.
//!
//! Pure logic (no macOS calls) so it can be unit-tested anywhere. It owns everything
//! the Mac needs to remember between events: which keys and buttons are held, the
//! modifier flags, where the pointer is, and double-click counting.
//!
//! ## Windows-style shortcuts
//!
//! Modifiers are tracked per physical key. Each held modifier has an *output* (the Mac
//! modifier it currently acts as). By default Ctrl -> Cmd, Win -> Ctrl, Alt -> Option.
//! When a normal key is pressed, [`Injector::plan`] may change those outputs for that
//! chord, so Windows shortcuts do what a Windows user expects:
//!
//! * Alt+Tab            -> Cmd+Tab (app switcher; stays open while Alt is held)
//! * Ctrl+Tab           -> Ctrl+Tab (next browser/editor tab)
//! * Ctrl+Left/Right    -> Option+Left/Right (word jump), Ctrl+Backspace/Delete -> delete word
//! * Home / End         -> line start / end;  Ctrl+Home / Ctrl+End -> document start / end
//! * In terminal apps   -> Ctrl stays Ctrl (Ctrl+C interrupts, Ctrl+D, Ctrl+R ...), except:
//!   Ctrl+C with text selected -> copy, Ctrl+V -> paste,
//!   Ctrl+Shift+C / V / T / W / N -> copy / paste / new tab / close tab / new window.

use kvm_core::keymap::{self, hid, mac};
use kvm_core::protocol::{button, Msg, WHEEL_DELTA};

/// CGEventFlags masks.
pub mod flag {
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
    pub const SHIFT: u64 = 0x0002_0000;
    pub const CONTROL: u64 = 0x0004_0000;
    pub const ALTERNATE: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    pub const NUMERIC_PAD: u64 = 0x0020_0000;
    pub const SECONDARY_FN: u64 = 0x0080_0000;
    pub const NON_COALESCED: u64 = 0x0000_0100;
    // Device-dependent left/right bits (NX_DEVICE*KEYMASK), which some apps check.
    pub const DEV_LCTRL: u64 = 0x0000_0001;
    pub const DEV_LSHIFT: u64 = 0x0000_0002;
    pub const DEV_RSHIFT: u64 = 0x0000_0004;
    pub const DEV_LCMD: u64 = 0x0000_0008;
    pub const DEV_RCMD: u64 = 0x0000_0010;
    pub const DEV_LALT: u64 = 0x0000_0020;
    pub const DEV_RALT: u64 = 0x0000_0040;
    pub const DEV_RCTRL: u64 = 0x0000_2000;
}

/// HID usages the shortcut rules refer to.
mod key {
    pub const C: u16 = 0x06;
    pub const N: u16 = 0x11;
    pub const T: u16 = 0x17;
    pub const V: u16 = 0x19;
    pub const W: u16 = 0x1A;
    pub const BACKSPACE: u16 = 0x2A;
    pub const TAB: u16 = 0x2B;
    pub const HOME: u16 = 0x4A;
    pub const DELETE: u16 = 0x4C;
    pub const END: u16 = 0x4D;
    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
}

/// macOS arrow keycodes.
mod mkey {
    pub const LEFT: u16 = 0x7B;
    pub const RIGHT: u16 = 0x7C;
    pub const DOWN: u16 = 0x7D;
    pub const UP: u16 = 0x7E;
}

const DOUBLE_CLICK_MS: u64 = 500;
const DOUBLE_CLICK_SLOP: f64 = 4.0;

#[derive(Debug, Clone)]
pub struct Settings {
    /// Ctrl -> Cmd, Win -> Ctrl, plus the Windows-style shortcut rules (on by default).
    pub swap_ctrl_cmd: bool,
    pub invert_scroll: bool,
    /// Pixels scrolled per wheel notch.
    pub scroll_px_per_notch: f64,
    /// Japanese keyboard: Caps (英数) -> Eisu, 半角/全角 toggles Eisu/Kana.
    pub jis: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { swap_ctrl_cmd: true, invert_scroll: false, scroll_px_per_notch: 40.0, jis: false }
    }
}

/// What the shortcut rules need to know about the Mac right now.
/// Only asked when a rule actually depends on it.
pub trait Ctx {
    /// Is the frontmost app a terminal (Terminal, iTerm2, ...)?
    fn front_is_terminal(&mut self) -> bool;
    /// Does the focused element have selected text?
    fn has_selection(&mut self) -> bool;
}

/// A context that knows nothing: not a terminal, no selection.
pub struct NoCtx;

impl Ctx for NoCtx {
    fn front_is_terminal(&mut self) -> bool {
        false
    }
    fn has_selection(&mut self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Moved,
    Down,
    Up,
    Dragged,
}

/// One event to post with CoreGraphics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Out {
    Key { code: u16, down: bool, flags: u64, modifier: bool, autorepeat: bool },
    /// `button` is the CoreGraphics button number: 0 left, 1 right, 2 middle, 3/4 thumb.
    Mouse { kind: MouseKind, button: u32, x: f64, y: f64, dx: i64, dy: i64, clicks: i64, flags: u64 },
    /// Pixels, CoreGraphics sign convention (positive y = up, positive x = left).
    Scroll { dy: i32, dx: i32 },
}

/// Kind of physical (Windows-keyboard) modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phys {
    Ctrl,
    Shift,
    Alt,
    Win,
}

/// A Mac modifier, without side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacMod {
    Cmd,
    Ctrl,
    Option,
    Shift,
}

fn phys_of(h: u16) -> Option<(Phys, bool)> {
    Some(match h {
        hid::LEFT_CTRL => (Phys::Ctrl, false),
        hid::RIGHT_CTRL => (Phys::Ctrl, true),
        hid::LEFT_SHIFT => (Phys::Shift, false),
        hid::RIGHT_SHIFT => (Phys::Shift, true),
        hid::LEFT_ALT => (Phys::Alt, false),
        hid::RIGHT_ALT => (Phys::Alt, true),
        hid::LEFT_GUI => (Phys::Win, false),
        hid::RIGHT_GUI => (Phys::Win, true),
        _ => return None,
    })
}

fn mac_code(m: MacMod, right: bool) -> u16 {
    match (m, right) {
        (MacMod::Cmd, false) => mac::COMMAND,
        (MacMod::Cmd, true) => mac::RIGHT_COMMAND,
        (MacMod::Ctrl, false) => mac::CONTROL,
        (MacMod::Ctrl, true) => mac::RIGHT_CONTROL,
        (MacMod::Option, false) => mac::OPTION,
        (MacMod::Option, true) => mac::RIGHT_OPTION,
        (MacMod::Shift, false) => mac::SHIFT,
        (MacMod::Shift, true) => mac::RIGHT_SHIFT,
    }
}

fn mod_flags(code: u16) -> u64 {
    match code {
        mac::COMMAND => flag::COMMAND | flag::DEV_LCMD,
        mac::RIGHT_COMMAND => flag::COMMAND | flag::DEV_RCMD,
        mac::SHIFT => flag::SHIFT | flag::DEV_LSHIFT,
        mac::RIGHT_SHIFT => flag::SHIFT | flag::DEV_RSHIFT,
        mac::OPTION => flag::ALTERNATE | flag::DEV_LALT,
        mac::RIGHT_OPTION => flag::ALTERNATE | flag::DEV_RALT,
        mac::CONTROL => flag::CONTROL | flag::DEV_LCTRL,
        mac::RIGHT_CONTROL => flag::CONTROL | flag::DEV_RCTRL,
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy)]
struct PhysMod {
    hid: u16,
    kind: Phys,
    right: bool,
    /// The Mac modifier this key currently acts as (None = nothing).
    out: Option<MacMod>,
}

/// How a normal key-press comes out on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyOut {
    /// The key itself, held until released.
    Normal,
    /// A complete press+release of another key, optionally with an extra modifier;
    /// the physical key's release is then ignored.
    Tap { code: u16, extra: Option<MacMod> },
}

/// Target outputs for each kind of held physical modifier, for one chord.
#[derive(Debug, Clone, Copy)]
struct Plan {
    ctrl: Option<MacMod>,
    shift: Option<MacMod>,
    alt: Option<MacMod>,
    key: KeyOut,
}

#[derive(Debug, Clone, Copy)]
struct Click {
    button: u32,
    at_ms: u64,
    x: f64,
    y: f64,
    count: i64,
}

pub struct Injector {
    s: Settings,
    /// Held physical modifiers and what each currently acts as.
    phys: Vec<PhysMod>,
    /// Mac modifier keys currently down (as sent).
    sent_mods: Vec<u16>,
    /// Non-modifier Mac keys currently down.
    keys_down: Vec<u16>,
    /// Physical keys whose release must be ignored (sent as a tap on press).
    tapped: Vec<u16>,
    buttons_down: [bool; 5],
    caps_on: bool,
    kana_on: bool,
    pos: (f64, f64),
    bounds: Rect,
    last_click: Option<Click>,
    scroll_rem: (f64, f64),
}

impl Injector {
    pub fn new(s: Settings) -> Self {
        Injector {
            s,
            phys: Vec::new(),
            sent_mods: Vec::new(),
            keys_down: Vec::new(),
            tapped: Vec::new(),
            buttons_down: [false; 5],
            caps_on: false,
            kana_on: false,
            pos: (0.0, 0.0),
            bounds: Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
            last_click: None,
            scroll_rem: (0.0, 0.0),
        }
    }

    /// Call when control arrives, with the real pointer position and the screen area.
    pub fn sync_pointer(&mut self, x: f64, y: f64, bounds: Rect) {
        self.bounds = bounds;
        self.pos = self.clamp(x, y);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_holding_anything(&self) -> bool {
        !self.keys_down.is_empty() || !self.sent_mods.is_empty() || self.buttons_down.iter().any(|&b| b)
    }

    fn clamp(&self, x: f64, y: f64) -> (f64, f64) {
        let b = self.bounds;
        (x.clamp(b.x, b.x + b.w - 1.0), y.clamp(b.y, b.y + b.h - 1.0))
    }

    pub fn handle(&mut self, msg: Msg, now_ms: u64, ctx: &mut dyn Ctx) -> Vec<Out> {
        match msg {
            Msg::KeyDown(h) => self.key(h, true, ctx),
            Msg::KeyUp(h) => self.key(h, false, ctx),
            Msg::MouseMove { dx, dy } => self.mouse_move(dx, dy),
            Msg::Button { button: b, down } => self.button(b, down, now_ms),
            Msg::Scroll { dx, dy } => self.scroll(dx, dy),
            Msg::Leave => self.release_all(),
            Msg::Enter | Msg::Hello { .. } | Msg::Heartbeat => Vec::new(),
        }
    }

    /// Key-ups and button-ups for everything still held. Used on Leave and when the
    /// connection drops, so nothing stays stuck down on the Mac.
    pub fn release_all(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for i in 0..self.buttons_down.len() {
            if self.buttons_down[i] {
                self.buttons_down[i] = false;
                out.push(self.mouse_event(MouseKind::Up, i as u32, 0, 0, self.clicks_for(i as u32)));
            }
        }
        // Ordinary keys first, modifiers last, so the final flags are clean.
        for k in std::mem::take(&mut self.keys_down) {
            out.push(Out::Key { code: k, down: false, flags: self.flags_for(k), modifier: false, autorepeat: false });
        }
        self.tapped.clear();
        self.phys.clear();
        out.extend(self.sync_mods());
        self.scroll_rem = (0.0, 0.0);
        out
    }

    // ----- keyboard -------------------------------------------------------------

    fn default_out(&self, kind: Phys) -> Option<MacMod> {
        let swap = self.s.swap_ctrl_cmd;
        Some(match kind {
            Phys::Ctrl if swap => MacMod::Cmd,
            Phys::Ctrl => MacMod::Ctrl,
            Phys::Win if swap => MacMod::Ctrl,
            Phys::Win => MacMod::Cmd,
            Phys::Alt => MacMod::Option,
            Phys::Shift => MacMod::Shift,
        })
    }

    fn holding(&self, kind: Phys) -> bool {
        self.phys.iter().any(|p| p.kind == kind)
    }

    /// Bring the Mac modifier keys in line with what the held physical modifiers
    /// currently act as. Releases first, then presses.
    fn sync_mods(&mut self) -> Vec<Out> {
        let mut want: Vec<u16> = Vec::new();
        for p in &self.phys {
            if let Some(m) = p.out {
                let c = mac_code(m, p.right);
                if !want.contains(&c) {
                    want.push(c);
                }
            }
        }
        let mut out = Vec::new();
        for c in self.sent_mods.clone() {
            if !want.contains(&c) {
                self.sent_mods.retain(|&s| s != c);
                out.push(Out::Key { code: c, down: false, flags: self.flags(), modifier: true, autorepeat: false });
            }
        }
        for c in want {
            if !self.sent_mods.contains(&c) {
                self.sent_mods.push(c);
                out.push(Out::Key { code: c, down: true, flags: self.flags(), modifier: true, autorepeat: false });
            }
        }
        out
    }

    fn key(&mut self, h: u16, down: bool, ctx: &mut dyn Ctx) -> Vec<Out> {
        // Modifiers: track the physical key, send its default meaning right away
        // (so e.g. Ctrl+click is Cmd+click). Windows auto-repeats held modifiers;
        // those repeats change nothing.
        if let Some((kind, right)) = phys_of(h) {
            if down {
                if !self.phys.iter().any(|p| p.hid == h) {
                    let out = self.default_out(kind);
                    self.phys.push(PhysMod { hid: h, kind, right, out });
                }
            } else {
                self.phys.retain(|p| p.hid != h);
            }
            return self.sync_mods();
        }

        // Release of a key that was sent as a tap.
        if !down {
            if let Some(i) = self.tapped.iter().position(|&t| t == h) {
                self.tapped.remove(i);
                return Vec::new();
            }
        }

        if self.s.jis {
            if h == hid::GRAVE {
                // 半角/全角: one key toggles the Mac between Eisu and Kana.
                if !down {
                    return Vec::new();
                }
                self.kana_on = !self.kana_on;
                let code = if self.kana_on { mac::JIS_KANA } else { mac::JIS_EISU };
                return self.tap(code, None);
            }
            if h == hid::CAPS_LOCK {
                if down {
                    self.kana_on = false;
                }
                return self.plain_key(mac::JIS_EISU, down);
            }
            if matches!(h, hid::INTL_HENKAN | hid::INTL_KANA | hid::LANG1) && down {
                self.kana_on = true;
            } else if matches!(h, hid::INTL_MUHENKAN | hid::LANG2) && down {
                self.kana_on = false;
            }
        }

        let Some(code) = keymap::hid_to_mac(h) else { return Vec::new() };

        if code == mac::CAPS_LOCK {
            // Caps Lock is a toggle on the Mac: one flags change per press, none on release.
            if !down {
                return Vec::new();
            }
            self.caps_on = !self.caps_on;
            return vec![Out::Key { code, down, flags: self.flags(), modifier: true, autorepeat: false }];
        }

        if !down {
            return self.plain_key(code, false);
        }

        // Key press: decide what the held modifiers mean for this chord.
        let mut out = Vec::new();
        let mut keyout = KeyOut::Normal;
        if (self.s.swap_ctrl_cmd && !self.phys.is_empty()) || matches!(h, key::HOME | key::END) {
            let plan = self.plan(h, ctx);
            for p in self.phys.iter_mut() {
                match p.kind {
                    Phys::Ctrl => p.out = plan.ctrl,
                    Phys::Shift => p.out = plan.shift,
                    Phys::Alt => p.out = plan.alt,
                    Phys::Win => {}
                }
            }
            out.extend(self.sync_mods());
            keyout = plan.key;
        }
        match keyout {
            KeyOut::Normal => out.extend(self.plain_key(code, true)),
            KeyOut::Tap { code: c, extra } => {
                if !self.tapped.contains(&h) {
                    self.tapped.push(h);
                }
                out.extend(self.tap(c, extra));
            }
        }
        out
    }

    /// The Windows-style shortcut rules (see the module docs).
    fn plan(&self, h: u16, ctx: &mut dyn Ctx) -> Plan {
        let ctrl = self.holding(Phys::Ctrl);
        let shift = self.holding(Phys::Shift);
        let alt = self.holding(Phys::Alt);
        let win = self.holding(Phys::Win);
        // Alt keeps whatever it currently means (Cmd stays Cmd for the whole Alt+Tab).
        let alt_now = self
            .phys
            .iter()
            .find(|p| p.kind == Phys::Alt)
            .map(|p| p.out)
            .unwrap_or(Some(MacMod::Option));
        let mut p = Plan {
            ctrl: self.default_out(Phys::Ctrl),
            shift: Some(MacMod::Shift),
            alt: alt_now,
            key: KeyOut::Normal,
        };
        if !self.s.swap_ctrl_cmd {
            return p;
        }

        if h == key::TAB {
            if alt && !ctrl && !win {
                p.alt = Some(MacMod::Cmd); // app switcher
            } else if ctrl {
                p.ctrl = Some(MacMod::Ctrl); // next tab
            }
            return p;
        }

        let home_end = matches!(h, key::HOME | key::END);
        if (ctrl || home_end) && ctx.front_is_terminal() {
            if ctrl {
                let to_cmd = |p: &mut Plan| {
                    p.ctrl = Some(MacMod::Cmd);
                    p.shift = None;
                };
                match h {
                    key::C if shift => to_cmd(&mut p),
                    key::C if ctx.has_selection() => to_cmd(&mut p),
                    key::V => to_cmd(&mut p),
                    key::T | key::W | key::N if shift => to_cmd(&mut p),
                    _ => p.ctrl = Some(MacMod::Ctrl),
                }
            }
            return p; // terminals handle Home/End themselves
        }

        if ctrl && matches!(h, key::LEFT | key::RIGHT | key::BACKSPACE | key::DELETE) {
            p.ctrl = Some(MacMod::Option); // by word
        }
        if home_end {
            p.key = if ctrl {
                // Ctrl acts as Cmd: Cmd+Up / Cmd+Down = start / end of document.
                KeyOut::Tap { code: if h == key::HOME { mkey::UP } else { mkey::DOWN }, extra: None }
            } else {
                KeyOut::Tap {
                    code: if h == key::HOME { mkey::LEFT } else { mkey::RIGHT },
                    extra: Some(MacMod::Cmd),
                }
            };
        }
        p
    }

    fn plain_key(&mut self, code: u16, down: bool) -> Vec<Out> {
        let held = self.keys_down.contains(&code);
        if down {
            if !held {
                self.keys_down.push(code);
            }
        } else {
            if !held {
                return Vec::new(); // never pressed here (e.g. pressed before Enter)
            }
            self.keys_down.retain(|&k| k != code);
        }
        vec![Out::Key { code, down, flags: self.flags_for(code), modifier: false, autorepeat: down && held }]
    }

    /// Press and release `code`, with `extra` held around it if it isn't already.
    fn tap(&mut self, code: u16, extra: Option<MacMod>) -> Vec<Out> {
        let mut v = Vec::new();
        let extra = extra.map(|m| mac_code(m, false)).filter(|c| !self.sent_mods.contains(c));
        if let Some(c) = extra {
            self.sent_mods.push(c);
            v.push(Out::Key { code: c, down: true, flags: self.flags(), modifier: true, autorepeat: false });
        }
        let flags = self.flags_for(code);
        v.push(Out::Key { code, down: true, flags, modifier: false, autorepeat: false });
        v.push(Out::Key { code, down: false, flags, modifier: false, autorepeat: false });
        if let Some(c) = extra {
            self.sent_mods.retain(|&s| s != c);
            v.push(Out::Key { code: c, down: false, flags: self.flags(), modifier: true, autorepeat: false });
        }
        v
    }

    fn flags(&self) -> u64 {
        let mut f = flag::NON_COALESCED;
        for &k in &self.sent_mods {
            f |= mod_flags(k);
        }
        if self.caps_on {
            f |= flag::ALPHA_SHIFT;
        }
        f
    }

    /// Flags as macOS itself would set them for this key (arrows carry Fn + keypad bits).
    fn flags_for(&self, code: u16) -> u64 {
        let mut f = self.flags();
        match code {
            0x7B..=0x7E => f |= flag::SECONDARY_FN | flag::NUMERIC_PAD, // arrows
            0x72..=0x77 | 0x79 => f |= flag::SECONDARY_FN, // help, home, pgup, fwd-del, end, pgdn
            0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51..=0x5C => f |= flag::NUMERIC_PAD,
            _ => {}
        }
        f
    }

    // ----- mouse ----------------------------------------------------------------

    fn pressed_button(&self) -> Option<u32> {
        self.buttons_down.iter().position(|&b| b).map(|i| i as u32)
    }

    fn mouse_event(&self, kind: MouseKind, button: u32, dx: i64, dy: i64, clicks: i64) -> Out {
        Out::Mouse { kind, button, x: self.pos.0, y: self.pos.1, dx, dy, clicks, flags: self.flags() }
    }

    fn mouse_move(&mut self, dx: i16, dy: i16) -> Vec<Out> {
        self.pos = self.clamp(self.pos.0 + dx as f64, self.pos.1 + dy as f64);
        let out = match self.pressed_button() {
            Some(b) => self.mouse_event(MouseKind::Dragged, b, dx as i64, dy as i64, 0),
            None => self.mouse_event(MouseKind::Moved, 0, dx as i64, dy as i64, 0),
        };
        vec![out]
    }

    fn clicks_for(&self, b: u32) -> i64 {
        match self.last_click {
            Some(c) if c.button == b => c.count,
            _ => 1,
        }
    }

    fn button(&mut self, wire: u8, down: bool, now_ms: u64) -> Vec<Out> {
        let b = match wire {
            button::LEFT => 0,
            button::RIGHT => 1,
            button::MIDDLE => 2,
            button::BACK => 3,
            button::FORWARD => 4,
            _ => return Vec::new(),
        };
        let i = b as usize;
        if down {
            let (x, y) = self.pos;
            let count = match self.last_click {
                Some(c)
                    if c.button == b
                        && now_ms.saturating_sub(c.at_ms) <= DOUBLE_CLICK_MS
                        && (c.x - x).abs() <= DOUBLE_CLICK_SLOP
                        && (c.y - y).abs() <= DOUBLE_CLICK_SLOP =>
                {
                    c.count + 1
                }
                _ => 1,
            };
            self.last_click = Some(Click { button: b, at_ms: now_ms, x, y, count });
            self.buttons_down[i] = true;
            vec![self.mouse_event(MouseKind::Down, b, 0, 0, count)]
        } else {
            if !self.buttons_down[i] {
                return Vec::new();
            }
            self.buttons_down[i] = false;
            vec![self.mouse_event(MouseKind::Up, b, 0, 0, self.clicks_for(b))]
        }
    }

    fn scroll(&mut self, dx: i16, dy: i16) -> Vec<Out> {
        let k = self.s.scroll_px_per_notch / WHEEL_DELTA as f64;
        let sign = if self.s.invert_scroll { -1.0 } else { 1.0 };
        // Windows: +dy = away from you (up), +dx = tilt right.
        // CoreGraphics: +wheel1 = up, +wheel2 = left.
        self.scroll_rem.1 += dy as f64 * k * sign;
        self.scroll_rem.0 += -(dx as f64) * k * sign;
        let py = self.scroll_rem.1.trunc();
        let px = self.scroll_rem.0.trunc();
        self.scroll_rem.1 -= py;
        self.scroll_rem.0 -= px;
        if py == 0.0 && px == 0.0 {
            return Vec::new();
        }
        vec![Out::Scroll { dy: py as i32, dx: px as i32 }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        terminal: bool,
        selection: bool,
        asked_selection: u32,
    }

    impl Ctx for Fake {
        fn front_is_terminal(&mut self) -> bool {
            self.terminal
        }
        fn has_selection(&mut self) -> bool {
            self.asked_selection += 1;
            self.selection
        }
    }

    fn term(selection: bool) -> Fake {
        Fake { terminal: true, selection, asked_selection: 0 }
    }

    fn inj() -> Injector {
        let mut i = Injector::new(Settings::default());
        i.sync_pointer(100.0, 100.0, Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 });
        i
    }

    fn key_out(o: &Out) -> (u16, bool, u64) {
        match *o {
            Out::Key { code, down, flags, .. } => (code, down, flags),
            _ => panic!("not a key: {o:?}"),
        }
    }

    /// (code, down) pairs, for compact assertions.
    fn keys(v: &[Out]) -> Vec<(u16, bool)> {
        v.iter().map(|o| { let (c, d, _) = key_out(o); (c, d) }).collect()
    }

    fn press(i: &mut Injector, h: u16, ctx: &mut dyn Ctx) -> Vec<Out> {
        i.handle(Msg::KeyDown(h), 0, ctx)
    }

    fn release(i: &mut Injector, h: u16, ctx: &mut dyn Ctx) -> Vec<Out> {
        i.handle(Msg::KeyUp(h), 0, ctx)
    }

    const MAC_C: u16 = 0x08;
    const MAC_V: u16 = 0x09;
    const MAC_D: u16 = 0x02;
    const MAC_TAB: u16 = 0x30;

    #[test]
    fn ctrl_c_becomes_cmd_c() {
        let mut i = inj();
        let c = &mut NoCtx;
        let o = press(&mut i, hid::LEFT_CTRL, c);
        assert_eq!(keys(&o), vec![(mac::COMMAND, true)]);
        assert!(key_out(&o[0]).2 & flag::COMMAND != 0);
        let o = press(&mut i, key::C, c);
        assert_eq!(keys(&o), vec![(MAC_C, true)]);
        let f = key_out(&o[0]).2;
        assert!(f & flag::COMMAND != 0 && f & flag::CONTROL == 0);
        release(&mut i, key::C, c);
        let o = release(&mut i, hid::LEFT_CTRL, c);
        assert_eq!(keys(&o), vec![(mac::COMMAND, false)]);
        assert_eq!(key_out(&o[0]).2 & flag::COMMAND, 0);
    }

    #[test]
    fn no_swap_keeps_everything_literal() {
        let mut i = Injector::new(Settings { swap_ctrl_cmd: false, ..Default::default() });
        let t = &mut term(false);
        assert_eq!(keys(&press(&mut i, hid::LEFT_CTRL, t)), vec![(mac::CONTROL, true)]);
        assert_eq!(keys(&press(&mut i, key::C, t)), vec![(MAC_C, true)]);
        let mut i = Injector::new(Settings { swap_ctrl_cmd: false, ..Default::default() });
        press(&mut i, hid::LEFT_ALT, t);
        assert_eq!(keys(&press(&mut i, key::TAB, t)), vec![(MAC_TAB, true)]); // Option+Tab
    }

    #[test]
    fn terminal_ctrl_c_interrupts_without_selection() {
        let mut i = inj();
        let t = &mut term(false);
        press(&mut i, hid::LEFT_CTRL, t);
        let o = press(&mut i, key::C, t);
        assert_eq!(keys(&o), vec![(mac::COMMAND, false), (mac::CONTROL, true), (MAC_C, true)]);
        assert!(key_out(&o[2]).2 & flag::CONTROL != 0);
        assert_eq!(key_out(&o[2]).2 & flag::COMMAND, 0);
        assert_eq!(t.asked_selection, 1);
    }

    #[test]
    fn terminal_ctrl_c_copies_selection() {
        let mut i = inj();
        let t = &mut term(true);
        press(&mut i, hid::LEFT_CTRL, t);
        let o = press(&mut i, key::C, t);
        assert_eq!(keys(&o), vec![(MAC_C, true)]); // Cmd already down
        assert!(key_out(&o[0]).2 & flag::COMMAND != 0);
    }

    #[test]
    fn terminal_paste_and_shift_variants() {
        let mut i = inj();
        let t = &mut term(false);
        press(&mut i, hid::LEFT_CTRL, t);
        assert_eq!(keys(&press(&mut i, key::V, t)), vec![(MAC_V, true)]); // Cmd+V
        release(&mut i, key::V, t);
        // Ctrl+Shift+V -> Cmd+V (Shift dropped)
        press(&mut i, hid::LEFT_SHIFT, t);
        let o = press(&mut i, key::V, t);
        assert_eq!(keys(&o), vec![(mac::SHIFT, false), (MAC_V, true)]);
        assert_eq!(key_out(&o[1]).2 & flag::SHIFT, 0);
        release(&mut i, key::V, t);
        // Ctrl+Shift+C -> Cmd+C without asking about the selection.
        let o = press(&mut i, key::C, t);
        assert_eq!(keys(&o), vec![(MAC_C, true)]);
        assert_eq!(t.asked_selection, 0);
        release(&mut i, key::C, t);
        // Releasing Shift sends nothing (it isn't down on the Mac).
        assert!(release(&mut i, hid::LEFT_SHIFT, t).is_empty());
    }

    #[test]
    fn terminal_other_ctrl_keys_stay_ctrl() {
        let mut i = inj();
        let t = &mut term(false);
        press(&mut i, hid::LEFT_CTRL, t);
        let o = press(&mut i, 0x07, t); // D
        assert_eq!(keys(&o), vec![(mac::COMMAND, false), (mac::CONTROL, true), (MAC_D, true)]);
        release(&mut i, 0x07, t);
        // Still holding Ctrl: Ctrl+V switches it to Cmd for the paste.
        let o = press(&mut i, key::V, t);
        assert_eq!(keys(&o), vec![(mac::CONTROL, false), (mac::COMMAND, true), (MAC_V, true)]);
    }

    #[test]
    fn alt_tab_is_app_switcher_held_until_alt_released() {
        let mut i = inj();
        let c = &mut NoCtx;
        assert_eq!(keys(&press(&mut i, hid::LEFT_ALT, c)), vec![(mac::OPTION, true)]);
        let o = press(&mut i, key::TAB, c);
        assert_eq!(keys(&o), vec![(mac::OPTION, false), (mac::COMMAND, true), (MAC_TAB, true)]);
        assert!(key_out(&o[2]).2 & flag::COMMAND != 0);
        release(&mut i, key::TAB, c);
        // Tab again while Alt held: still Cmd, nothing re-pressed.
        assert_eq!(keys(&press(&mut i, key::TAB, c)), vec![(MAC_TAB, true)]);
        release(&mut i, key::TAB, c);
        // Arrow keys inside the switcher keep Cmd held.
        assert_eq!(keys(&press(&mut i, key::LEFT, c)), vec![(mkey::LEFT, true)]);
        release(&mut i, key::LEFT, c);
        // Releasing Alt releases Cmd -> switcher picks the app.
        assert_eq!(keys(&release(&mut i, hid::LEFT_ALT, c)), vec![(mac::COMMAND, false)]);
        assert!(!i.is_holding_anything());
    }

    #[test]
    fn alt_shift_tab_goes_backwards_and_survives_shift_release() {
        let mut i = inj();
        let c = &mut NoCtx;
        press(&mut i, hid::LEFT_ALT, c);
        press(&mut i, hid::LEFT_SHIFT, c);
        let o = press(&mut i, key::TAB, c);
        assert!(key_out(o.last().unwrap()).2 & (flag::COMMAND | flag::SHIFT) == flag::COMMAND | flag::SHIFT);
        release(&mut i, key::TAB, c);
        assert_eq!(keys(&release(&mut i, hid::LEFT_SHIFT, c)), vec![(mac::SHIFT, false)]);
        assert_eq!(keys(&release(&mut i, hid::LEFT_ALT, c)), vec![(mac::COMMAND, false)]);
    }

    #[test]
    fn ctrl_tab_switches_tabs() {
        let mut i = inj();
        let c = &mut NoCtx;
        press(&mut i, hid::LEFT_CTRL, c);
        let o = press(&mut i, key::TAB, c);
        assert_eq!(keys(&o), vec![(mac::COMMAND, false), (mac::CONTROL, true), (MAC_TAB, true)]);
        release(&mut i, key::TAB, c);
        // Ctrl+C right after, still holding Ctrl: back to Cmd+C.
        let o = press(&mut i, key::C, c);
        assert_eq!(keys(&o), vec![(mac::CONTROL, false), (mac::COMMAND, true), (MAC_C, true)]);
    }

    #[test]
    fn ctrl_arrows_jump_by_word() {
        let mut i = inj();
        let c = &mut NoCtx;
        press(&mut i, hid::LEFT_CTRL, c);
        let o = press(&mut i, key::LEFT, c);
        assert_eq!(keys(&o), vec![(mac::COMMAND, false), (mac::OPTION, true), (mkey::LEFT, true)]);
        assert!(key_out(&o[2]).2 & flag::ALTERNATE != 0);
        release(&mut i, key::LEFT, c);
        assert_eq!(keys(&release(&mut i, hid::LEFT_CTRL, c)), vec![(mac::OPTION, false)]);
    }

    #[test]
    fn home_end_go_to_line_start_end() {
        let mut i = inj();
        let c = &mut NoCtx;
        let o = press(&mut i, key::HOME, c);
        assert_eq!(
            keys(&o),
            vec![(mac::COMMAND, true), (mkey::LEFT, true), (mkey::LEFT, false), (mac::COMMAND, false)]
        );
        assert!(release(&mut i, key::HOME, c).is_empty());
        // Ctrl+End -> Cmd+Down (Ctrl is already Cmd).
        press(&mut i, hid::LEFT_CTRL, c);
        let o = press(&mut i, key::END, c);
        assert_eq!(keys(&o), vec![(mkey::DOWN, true), (mkey::DOWN, false)]);
        // Terminals get the real Home/End.
        let mut i = inj();
        let t = &mut term(false);
        assert_eq!(keys(&press(&mut i, key::HOME, t)), vec![(0x73, true)]);
        assert_eq!(keys(&release(&mut i, key::HOME, t)), vec![(0x73, false)]);
    }

    #[test]
    fn held_key_repeats_as_autorepeat() {
        let mut i = inj();
        i.handle(Msg::KeyDown(0x04), 0, &mut NoCtx);
        let o = i.handle(Msg::KeyDown(0x04), 30, &mut NoCtx);
        assert!(matches!(o[0], Out::Key { autorepeat: true, .. }));
    }

    #[test]
    fn modifier_autorepeat_sends_nothing() {
        let mut i = inj();
        press(&mut i, hid::LEFT_SHIFT, &mut NoCtx);
        assert!(press(&mut i, hid::LEFT_SHIFT, &mut NoCtx).is_empty());
    }

    #[test]
    fn leave_releases_everything() {
        let mut i = inj();
        let c = &mut NoCtx;
        press(&mut i, hid::LEFT_SHIFT, c);
        press(&mut i, 0x04, c);
        i.handle(Msg::Button { button: button::LEFT, down: true }, 0, c);
        assert!(i.is_holding_anything());
        let o = i.handle(Msg::Leave, 0, c);
        assert_eq!(o.len(), 3);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Up, button: 0, .. }));
        assert_eq!(key_out(&o[1]).0, 0x00); // A before Shift
        let (code, _, flags) = key_out(&o[2]);
        assert_eq!(code, mac::SHIFT);
        assert_eq!(flags & flag::SHIFT, 0);
        assert!(!i.is_holding_anything());
        // Ups for keys we no longer hold are dropped.
        assert!(release(&mut i, 0x04, c).is_empty());
        assert!(release(&mut i, hid::LEFT_SHIFT, c).is_empty());
    }

    #[test]
    fn pointer_moves_and_clamps() {
        let mut i = inj();
        let o = i.handle(Msg::MouseMove { dx: 10, dy: -5 }, 0, &mut NoCtx);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Moved, x, y, .. } if x == 110.0 && y == 95.0));
        let o = i.handle(Msg::MouseMove { dx: -5000, dy: 5000 }, 0, &mut NoCtx);
        assert!(matches!(o[0], Out::Mouse { x, y, .. } if x == 0.0 && y == 899.0));
    }

    #[test]
    fn drag_while_button_held() {
        let mut i = inj();
        i.handle(Msg::Button { button: button::RIGHT, down: true }, 0, &mut NoCtx);
        let o = i.handle(Msg::MouseMove { dx: 3, dy: 3 }, 0, &mut NoCtx);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Dragged, button: 1, .. }));
    }

    #[test]
    fn ctrl_click_is_cmd_click() {
        let mut i = inj();
        press(&mut i, hid::LEFT_CTRL, &mut NoCtx);
        let o = i.handle(Msg::Button { button: button::LEFT, down: true }, 0, &mut NoCtx);
        assert!(matches!(o[0], Out::Mouse { flags, .. } if flags & flag::COMMAND != 0));
    }

    #[test]
    fn double_click_counting() {
        let mut i = inj();
        let down = |i: &mut Injector, t| i.handle(Msg::Button { button: button::LEFT, down: true }, t, &mut NoCtx);
        let up = |i: &mut Injector, t| i.handle(Msg::Button { button: button::LEFT, down: false }, t, &mut NoCtx);
        assert!(matches!(down(&mut i, 0)[0], Out::Mouse { clicks: 1, .. }));
        up(&mut i, 50);
        assert!(matches!(down(&mut i, 200)[0], Out::Mouse { clicks: 2, .. }));
        assert!(matches!(up(&mut i, 250)[0], Out::Mouse { clicks: 2, .. }));
        // Too late: a fresh single click.
        assert!(matches!(down(&mut i, 2000)[0], Out::Mouse { clicks: 1, .. }));
        up(&mut i, 2050);
        // Moved away: also a fresh click.
        i.handle(Msg::MouseMove { dx: 50, dy: 0 }, 2100, &mut NoCtx);
        assert!(matches!(down(&mut i, 2150)[0], Out::Mouse { clicks: 1, .. }));
    }

    #[test]
    fn thumb_button_maps_to_cg_button_3() {
        let mut i = inj();
        let o = i.handle(Msg::Button { button: button::BACK, down: true }, 0, &mut NoCtx);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Down, button: 3, .. }));
    }

    #[test]
    fn scroll_directions_and_smooth_remainders() {
        let mut i = inj();
        let c = &mut NoCtx;
        assert_eq!(i.handle(Msg::Scroll { dx: 0, dy: 120 }, 0, c), vec![Out::Scroll { dy: 40, dx: 0 }]);
        assert_eq!(i.handle(Msg::Scroll { dx: 120, dy: 0 }, 0, c), vec![Out::Scroll { dy: 0, dx: -40 }]);
        assert!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0, c).is_empty());
        assert!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0, c).is_empty());
        assert_eq!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0, c), vec![Out::Scroll { dy: 1, dx: 0 }]);
        let mut inv = Injector::new(Settings { invert_scroll: true, ..Default::default() });
        assert_eq!(inv.handle(Msg::Scroll { dx: 0, dy: 120 }, 0, c), vec![Out::Scroll { dy: -40, dx: 0 }]);
    }

    #[test]
    fn jis_hankaku_toggles_ime() {
        let mut i = Injector::new(Settings { jis: true, ..Default::default() });
        let c = &mut NoCtx;
        let o = press(&mut i, hid::GRAVE, c);
        assert_eq!(keys(&o), vec![(mac::JIS_KANA, true), (mac::JIS_KANA, false)]);
        assert!(release(&mut i, hid::GRAVE, c).is_empty());
        let o = press(&mut i, hid::GRAVE, c);
        assert_eq!(key_out(&o[0]).0, mac::JIS_EISU);
        let mut plain = inj();
        assert_eq!(key_out(&press(&mut plain, hid::INTL_HENKAN, c)[0]).0, mac::JIS_KANA);
        assert_eq!(key_out(&press(&mut plain, hid::INTL_MUHENKAN, c)[0]).0, mac::JIS_EISU);
    }

    #[test]
    fn caps_lock_toggles_alpha_shift() {
        let mut i = inj();
        let c = &mut NoCtx;
        let o = press(&mut i, hid::CAPS_LOCK, c);
        assert!(key_out(&o[0]).2 & flag::ALPHA_SHIFT != 0);
        assert!(release(&mut i, hid::CAPS_LOCK, c).is_empty());
        let o = press(&mut i, 0x04, c);
        assert!(key_out(&o[0]).2 & flag::ALPHA_SHIFT != 0);
        release(&mut i, 0x04, c);
        let o = press(&mut i, hid::CAPS_LOCK, c);
        assert_eq!(key_out(&o[0]).2 & flag::ALPHA_SHIFT, 0);
    }
}
