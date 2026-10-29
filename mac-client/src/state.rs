//! Turns protocol messages into the exact events to post on macOS.
//!
//! Pure logic (no macOS calls) so it can be unit-tested anywhere. It owns everything
//! the Mac needs to remember between events: which keys and buttons are held, the
//! modifier flags, where the pointer is, and double-click counting.

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

const DOUBLE_CLICK_MS: u64 = 500;
const DOUBLE_CLICK_SLOP: f64 = 4.0;

#[derive(Debug, Clone)]
pub struct Settings {
    /// Ctrl -> Cmd, Win -> Ctrl (on by default).
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
    keys_down: Vec<u16>,
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
            keys_down: Vec::new(),
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
        !self.keys_down.is_empty() || self.buttons_down.iter().any(|&b| b)
    }

    fn clamp(&self, x: f64, y: f64) -> (f64, f64) {
        let b = self.bounds;
        (x.clamp(b.x, b.x + b.w - 1.0), y.clamp(b.y, b.y + b.h - 1.0))
    }

    pub fn handle(&mut self, msg: Msg, now_ms: u64) -> Vec<Out> {
        match msg {
            Msg::KeyDown(h) => self.key(h, true),
            Msg::KeyUp(h) => self.key(h, false),
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
        let mut keys: Vec<u16> = self.keys_down.clone();
        keys.sort_by_key(|&k| is_modifier(k));
        for k in keys {
            self.keys_down.retain(|&d| d != k);
            out.push(Out::Key {
                code: k,
                down: false,
                flags: self.flags_for(k),
                modifier: is_modifier(k),
                autorepeat: false,
            });
        }
        self.scroll_rem = (0.0, 0.0);
        out
    }

    fn key(&mut self, h: u16, down: bool) -> Vec<Out> {
        let h = if self.s.swap_ctrl_cmd { keymap::swap_ctrl_cmd(h) } else { h };

        if self.s.jis {
            if h == hid::GRAVE {
                // 半角/全角: one key toggles the Mac between Eisu and Kana.
                if !down {
                    return Vec::new();
                }
                self.kana_on = !self.kana_on;
                let code = if self.kana_on { mac::JIS_KANA } else { mac::JIS_EISU };
                return self.tap(code);
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
        self.plain_key(code, down)
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
        vec![Out::Key {
            code,
            down,
            flags: self.flags_for(code),
            modifier: is_modifier(code),
            autorepeat: down && held,
        }]
    }

    fn tap(&mut self, code: u16) -> Vec<Out> {
        let mut v = self.plain_key(code, true);
        v.extend(self.plain_key(code, false));
        v
    }

    fn flags(&self) -> u64 {
        let mut f = flag::NON_COALESCED;
        for &k in &self.keys_down {
            f |= match k {
                mac::COMMAND => flag::COMMAND | flag::DEV_LCMD,
                mac::RIGHT_COMMAND => flag::COMMAND | flag::DEV_RCMD,
                mac::SHIFT => flag::SHIFT | flag::DEV_LSHIFT,
                mac::RIGHT_SHIFT => flag::SHIFT | flag::DEV_RSHIFT,
                mac::OPTION => flag::ALTERNATE | flag::DEV_LALT,
                mac::RIGHT_OPTION => flag::ALTERNATE | flag::DEV_RALT,
                mac::CONTROL => flag::CONTROL | flag::DEV_LCTRL,
                mac::RIGHT_CONTROL => flag::CONTROL | flag::DEV_RCTRL,
                _ => 0,
            };
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

pub fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        mac::COMMAND
            | mac::RIGHT_COMMAND
            | mac::SHIFT
            | mac::RIGHT_SHIFT
            | mac::OPTION
            | mac::RIGHT_OPTION
            | mac::CONTROL
            | mac::RIGHT_CONTROL
            | mac::FUNCTION
            | mac::CAPS_LOCK
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn ctrl_c_becomes_cmd_c() {
        let mut i = inj();
        let o = i.handle(Msg::KeyDown(hid::LEFT_CTRL), 0);
        let (code, down, flags) = key_out(&o[0]);
        assert_eq!((code, down), (mac::COMMAND, true));
        assert!(flags & flag::COMMAND != 0);
        let o = i.handle(Msg::KeyDown(0x06), 0); // C
        let (code, _, flags) = key_out(&o[0]);
        assert_eq!(code, 0x08);
        assert!(flags & flag::COMMAND != 0 && flags & flag::CONTROL == 0);
        i.handle(Msg::KeyUp(0x06), 0);
        let o = i.handle(Msg::KeyUp(hid::LEFT_CTRL), 0);
        let (_, down, flags) = key_out(&o[0]);
        assert!(!down && flags & flag::COMMAND == 0);
    }

    #[test]
    fn no_swap_keeps_ctrl() {
        let mut i = Injector::new(Settings { swap_ctrl_cmd: false, ..Default::default() });
        let o = i.handle(Msg::KeyDown(hid::LEFT_CTRL), 0);
        assert_eq!(key_out(&o[0]).0, mac::CONTROL);
    }

    #[test]
    fn held_key_repeats_as_autorepeat() {
        let mut i = inj();
        i.handle(Msg::KeyDown(0x04), 0);
        let o = i.handle(Msg::KeyDown(0x04), 30);
        assert!(matches!(o[0], Out::Key { autorepeat: true, .. }));
    }

    #[test]
    fn leave_releases_everything() {
        let mut i = inj();
        i.handle(Msg::KeyDown(hid::LEFT_SHIFT), 0);
        i.handle(Msg::KeyDown(0x04), 0);
        i.handle(Msg::Button { button: button::LEFT, down: true }, 0);
        assert!(i.is_holding_anything());
        let o = i.handle(Msg::Leave, 0);
        assert_eq!(o.len(), 3);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Up, button: 0, .. }));
        assert_eq!(key_out(&o[1]).0, 0x00); // A before Shift
        let (code, _, flags) = key_out(&o[2]);
        assert_eq!(code, mac::SHIFT);
        assert_eq!(flags & flag::SHIFT, 0);
        assert!(!i.is_holding_anything());
        // Ups for keys we no longer hold are dropped.
        assert!(i.handle(Msg::KeyUp(0x04), 0).is_empty());
    }

    #[test]
    fn pointer_moves_and_clamps() {
        let mut i = inj();
        let o = i.handle(Msg::MouseMove { dx: 10, dy: -5 }, 0);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Moved, x, y, .. } if x == 110.0 && y == 95.0));
        let o = i.handle(Msg::MouseMove { dx: -5000, dy: 5000 }, 0);
        assert!(matches!(o[0], Out::Mouse { x, y, .. } if x == 0.0 && y == 899.0));
    }

    #[test]
    fn drag_while_button_held() {
        let mut i = inj();
        i.handle(Msg::Button { button: button::RIGHT, down: true }, 0);
        let o = i.handle(Msg::MouseMove { dx: 3, dy: 3 }, 0);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Dragged, button: 1, .. }));
    }

    #[test]
    fn double_click_counting() {
        let mut i = inj();
        let down = |i: &mut Injector, t| i.handle(Msg::Button { button: button::LEFT, down: true }, t);
        let up = |i: &mut Injector, t| i.handle(Msg::Button { button: button::LEFT, down: false }, t);
        assert!(matches!(down(&mut i, 0)[0], Out::Mouse { clicks: 1, .. }));
        up(&mut i, 50);
        assert!(matches!(down(&mut i, 200)[0], Out::Mouse { clicks: 2, .. }));
        assert!(matches!(up(&mut i, 250)[0], Out::Mouse { clicks: 2, .. }));
        // Too late: a fresh single click.
        assert!(matches!(down(&mut i, 2000)[0], Out::Mouse { clicks: 1, .. }));
        up(&mut i, 2050);
        // Moved away: also a fresh click.
        i.handle(Msg::MouseMove { dx: 50, dy: 0 }, 2100);
        assert!(matches!(down(&mut i, 2150)[0], Out::Mouse { clicks: 1, .. }));
    }

    #[test]
    fn thumb_button_maps_to_cg_button_3() {
        let mut i = inj();
        let o = i.handle(Msg::Button { button: button::BACK, down: true }, 0);
        assert!(matches!(o[0], Out::Mouse { kind: MouseKind::Down, button: 3, .. }));
    }

    #[test]
    fn scroll_directions_and_smooth_remainders() {
        let mut i = inj();
        // One notch up -> 40 px up.
        assert_eq!(i.handle(Msg::Scroll { dx: 0, dy: 120 }, 0), vec![Out::Scroll { dy: 40, dx: 0 }]);
        // Tilt right -> negative wheel2.
        assert_eq!(i.handle(Msg::Scroll { dx: 120, dy: 0 }, 0), vec![Out::Scroll { dy: 0, dx: -40 }]);
        // Tiny smooth-scroll steps accumulate instead of being lost.
        assert!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0).is_empty());
        assert!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0).is_empty());
        assert_eq!(i.handle(Msg::Scroll { dx: 0, dy: 1 }, 0), vec![Out::Scroll { dy: 1, dx: 0 }]);
        let mut inv = Injector::new(Settings { invert_scroll: true, ..Default::default() });
        assert_eq!(inv.handle(Msg::Scroll { dx: 0, dy: 120 }, 0), vec![Out::Scroll { dy: -40, dx: 0 }]);
    }

    #[test]
    fn jis_hankaku_toggles_ime() {
        let mut i = Injector::new(Settings { jis: true, ..Default::default() });
        let o = i.handle(Msg::KeyDown(hid::GRAVE), 0);
        assert_eq!(o.len(), 2);
        assert_eq!(key_out(&o[0]).0, mac::JIS_KANA);
        assert!(i.handle(Msg::KeyUp(hid::GRAVE), 0).is_empty());
        let o = i.handle(Msg::KeyDown(hid::GRAVE), 0);
        assert_eq!(key_out(&o[0]).0, mac::JIS_EISU);
        // 変換 -> Kana, 無変換 -> Eisu, without --jis too.
        let mut plain = inj();
        assert_eq!(key_out(&plain.handle(Msg::KeyDown(hid::INTL_HENKAN), 0)[0]).0, mac::JIS_KANA);
        assert_eq!(key_out(&plain.handle(Msg::KeyDown(hid::INTL_MUHENKAN), 0)[0]).0, mac::JIS_EISU);
    }

    #[test]
    fn caps_lock_toggles_alpha_shift() {
        let mut i = inj();
        let o = i.handle(Msg::KeyDown(hid::CAPS_LOCK), 0);
        assert!(key_out(&o[0]).2 & flag::ALPHA_SHIFT != 0);
        i.handle(Msg::KeyUp(hid::CAPS_LOCK), 0);
        let o = i.handle(Msg::KeyDown(0x04), 0);
        assert!(key_out(&o[0]).2 & flag::ALPHA_SHIFT != 0);
        i.handle(Msg::KeyUp(0x04), 0);
        let o = i.handle(Msg::KeyDown(hid::CAPS_LOCK), 0);
        assert_eq!(key_out(&o[0]).2 & flag::ALPHA_SHIFT, 0);
    }
}
