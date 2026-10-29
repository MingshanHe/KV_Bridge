//! Windows-side routing: decides, for every keyboard/mouse event the hooks see,
//! whether Windows keeps it or it is forwarded to the Mac, and handles switching.
//!
//! Kept free of any Windows API so it can be unit-tested anywhere.

use crate::keymap::hid;
use crate::protocol::{button, Msg};
use std::collections::HashSet;

/// Minimum time between two toggles, so one press can never switch twice.
pub const TOGGLE_DEBOUNCE_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Input goes to Windows as normal.
    Local,
    /// Input is swallowed on Windows and forwarded to the Mac.
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// `id` identifies the physical key for pairing down/up (the Windows vk code is fine).
    /// `hid` is `None` for keys with no HID mapping.
    Key { id: u32, hid: Option<u16>, down: bool },
    /// Absolute cursor position reported by the mouse hook.
    MouseMove { x: i32, y: i32 },
    Button { button: u8, down: bool },
    Wheel { dx: i16, dy: i16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Switched to the Mac. The caller should call [`Router::set_anchor`] with the
    /// current cursor position.
    EnteredRemote,
    LeftRemote,
    /// The user asked to switch but no Mac is connected.
    NotConnected,
    /// A key was pressed that has no mapping (swallowed while remote).
    Unmapped(u32),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// true = stop Windows from seeing this event.
    pub block: bool,
    pub send: Vec<Msg>,
    pub notice: Option<Notice>,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Wire button number that toggles control (button::BACK / button::FORWARD),
    /// or None to disable the mouse-button toggle.
    pub toggle_button: Option<u8>,
    /// Ctrl+Alt+M toggles control as a keyboard fallback.
    pub hotkey: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { toggle_button: Some(button::FORWARD), hotkey: true }
    }
}

pub struct Router {
    cfg: Config,
    mode: Mode,
    connected: bool,
    /// Keys Windows has seen go down and not yet up. Their key-ups must reach
    /// Windows even while remote, or Windows would think they are stuck down.
    windows_down: HashSet<u32>,
    /// Physical modifier state, tracked ourselves because swallowed keys never
    /// reach GetAsyncKeyState.
    ctrl_down: HashSet<u16>,
    alt_down: HashSet<u16>,
    /// Key-up of the hotkey's letter, swallowed whichever mode we're in by then.
    swallow_key_up: Option<u32>,
    swallow_button_up: Option<u8>,
    last_toggle_ms: Option<u64>,
    anchor: Option<(i32, i32)>,
}

impl Router {
    pub fn new(cfg: Config) -> Self {
        Router {
            cfg,
            mode: Mode::Local,
            connected: false,
            windows_down: HashSet::new(),
            ctrl_down: HashSet::new(),
            alt_down: HashSet::new(),
            swallow_key_up: None,
            swallow_button_up: None,
            last_toggle_ms: None,
            anchor: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// The cursor position Windows' pointer is parked at while remote. Mouse deltas
    /// are measured from here (Windows never moves the cursor, because we block moves).
    pub fn set_anchor(&mut self, x: i32, y: i32) {
        self.anchor = Some((x, y));
    }

    /// Called by the network side. Losing the connection while remote hands control
    /// back to Windows so the keyboard is never left dead.
    pub fn set_connected(&mut self, connected: bool) -> Option<Notice> {
        self.connected = connected;
        if !connected && self.mode == Mode::Remote {
            self.mode = Mode::Local;
            self.anchor = None;
            return Some(Notice::LeftRemote);
        }
        None
    }

    pub fn handle(&mut self, input: Input, now_ms: u64) -> Decision {
        match input {
            Input::Key { id, hid, down } => self.on_key(id, hid, down, now_ms),
            Input::Button { button, down } => self.on_button(button, down, now_ms),
            Input::MouseMove { x, y } => self.on_move(x, y),
            Input::Wheel { dx, dy } => {
                if self.mode == Mode::Remote {
                    Decision { block: true, send: vec![Msg::Scroll { dx, dy }], notice: None }
                } else {
                    Decision::default()
                }
            }
        }
    }

    fn toggle(&mut self, now_ms: u64) -> Decision {
        if let Some(t) = self.last_toggle_ms {
            if now_ms.saturating_sub(t) < TOGGLE_DEBOUNCE_MS {
                return Decision { block: true, ..Default::default() };
            }
        }
        self.last_toggle_ms = Some(now_ms);
        match self.mode {
            Mode::Local if !self.connected => Decision {
                block: true,
                send: vec![],
                notice: Some(Notice::NotConnected),
            },
            Mode::Local => {
                self.mode = Mode::Remote;
                self.anchor = None;
                Decision {
                    block: true,
                    send: vec![Msg::Enter],
                    notice: Some(Notice::EnteredRemote),
                }
            }
            Mode::Remote => {
                self.mode = Mode::Local;
                self.anchor = None;
                Decision {
                    block: true,
                    send: vec![Msg::Leave],
                    notice: Some(Notice::LeftRemote),
                }
            }
        }
    }

    fn on_key(&mut self, id: u32, hid: Option<u16>, down: bool, now_ms: u64) -> Decision {
        // Track physical Ctrl/Alt in both modes.
        if let Some(h) = hid {
            if hid::is_ctrl(h) {
                if down { self.ctrl_down.insert(h); } else { self.ctrl_down.remove(&h); }
            } else if hid::is_alt(h) {
                if down { self.alt_down.insert(h); } else { self.alt_down.remove(&h); }
            }
        }

        // The hotkey letter's key-up is always swallowed.
        if !down && self.swallow_key_up == Some(id) {
            self.swallow_key_up = None;
            return Decision { block: true, ..Default::default() };
        }

        // Ctrl+Alt+M.
        if down
            && self.cfg.hotkey
            && hid == Some(hid::M)
            && !self.ctrl_down.is_empty()
            && !self.alt_down.is_empty()
        {
            self.swallow_key_up = Some(id);
            return self.toggle(now_ms);
        }

        match self.mode {
            Mode::Local => {
                if down {
                    self.windows_down.insert(id);
                } else {
                    self.windows_down.remove(&id);
                }
                Decision::default()
            }
            Mode::Remote => {
                if !down && self.windows_down.remove(&id) {
                    // Pressed before the switch: Windows must see it go up.
                    return Decision::default();
                }
                match hid {
                    Some(h) => Decision {
                        block: true,
                        send: vec![if down { Msg::KeyDown(h) } else { Msg::KeyUp(h) }],
                        notice: None,
                    },
                    None => Decision {
                        block: true,
                        send: vec![],
                        notice: if down { Some(Notice::Unmapped(id)) } else { None },
                    },
                }
            }
        }
    }

    fn on_button(&mut self, b: u8, down: bool, now_ms: u64) -> Decision {
        if self.cfg.toggle_button == Some(b) {
            // Neither machine ever sees the toggle button itself.
            if down {
                self.swallow_button_up = Some(b);
                return self.toggle(now_ms);
            }
            self.swallow_button_up = None;
            return Decision { block: true, ..Default::default() };
        }
        if self.mode == Mode::Remote {
            Decision { block: true, send: vec![Msg::Button { button: b, down }], notice: None }
        } else {
            Decision::default()
        }
    }

    fn on_move(&mut self, x: i32, y: i32) -> Decision {
        if self.mode != Mode::Remote {
            return Decision::default();
        }
        let Some((ax, ay)) = self.anchor else {
            // No anchor yet: take this position as the reference.
            self.anchor = Some((x, y));
            return Decision { block: true, ..Default::default() };
        };
        let dx = clamp_i16(x - ax);
        let dy = clamp_i16(y - ay);
        let send = if dx != 0 || dy != 0 { vec![Msg::MouseMove { dx, dy }] } else { vec![] };
        Decision { block: true, send, notice: None }
    }
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::hid;

    const VK_CTRL: u32 = 0xA2;
    const VK_ALT: u32 = 0xA4;
    const VK_M: u32 = 0x4D;
    const VK_A: u32 = 0x41;

    fn key(id: u32, h: u16, down: bool) -> Input {
        Input::Key { id, hid: Some(h), down }
    }

    fn connected_router() -> Router {
        let mut r = Router::new(Config::default());
        r.set_connected(true);
        r
    }

    #[test]
    fn local_mode_passes_everything() {
        let mut r = connected_router();
        let d = r.handle(key(VK_A, 0x04, true), 0);
        assert!(!d.block && d.send.is_empty());
        let d = r.handle(Input::MouseMove { x: 5, y: 5 }, 0);
        assert!(!d.block);
        let d = r.handle(Input::Button { button: button::LEFT, down: true }, 0);
        assert!(!d.block);
    }

    #[test]
    fn lift_button_toggles_and_is_swallowed() {
        let mut r = connected_router();
        let d = r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        assert!(d.block);
        assert_eq!(d.send, vec![Msg::Enter]);
        assert_eq!(d.notice, Some(Notice::EnteredRemote));
        assert_eq!(r.mode(), Mode::Remote);
        let d = r.handle(Input::Button { button: button::FORWARD, down: false }, 1050);
        assert!(d.block && d.send.is_empty());

        // Second press too soon is ignored (debounce)...
        let d = r.handle(Input::Button { button: button::FORWARD, down: true }, 1100);
        assert!(d.block && d.send.is_empty());
        assert_eq!(r.mode(), Mode::Remote);
        // ...a later one switches back.
        let d = r.handle(Input::Button { button: button::FORWARD, down: true }, 2000);
        assert_eq!(d.send, vec![Msg::Leave]);
        assert_eq!(r.mode(), Mode::Local);
    }

    #[test]
    fn other_thumb_button_is_forwarded_when_remote() {
        let mut r = connected_router();
        r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        let d = r.handle(Input::Button { button: button::BACK, down: true }, 1200);
        assert!(d.block);
        assert_eq!(d.send, vec![Msg::Button { button: button::BACK, down: true }]);
    }

    #[test]
    fn cannot_switch_without_mac() {
        let mut r = Router::new(Config::default());
        let d = r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        assert!(d.block);
        assert!(d.send.is_empty());
        assert_eq!(d.notice, Some(Notice::NotConnected));
        assert_eq!(r.mode(), Mode::Local);
    }

    #[test]
    fn keys_forwarded_while_remote() {
        let mut r = connected_router();
        r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        let d = r.handle(key(VK_A, 0x04, true), 1100);
        assert!(d.block);
        assert_eq!(d.send, vec![Msg::KeyDown(0x04)]);
        let d = r.handle(key(VK_A, 0x04, false), 1150);
        assert_eq!(d.send, vec![Msg::KeyUp(0x04)]);
    }

    #[test]
    fn hotkey_round_trip_leaves_no_stuck_keys_on_windows() {
        let mut r = connected_router();
        // Press Ctrl, Alt (Windows sees them), then M -> switch to Mac.
        assert!(!r.handle(key(VK_CTRL, hid::LEFT_CTRL, true), 0).block);
        assert!(!r.handle(key(VK_ALT, hid::LEFT_ALT, true), 10).block);
        let d = r.handle(key(VK_M, hid::M, true), 20);
        assert!(d.block);
        assert_eq!(d.send, vec![Msg::Enter]);
        // Releases: M swallowed; Ctrl/Alt ups must reach Windows, not the Mac.
        assert!(r.handle(key(VK_M, hid::M, false), 30).block);
        let d = r.handle(key(VK_ALT, hid::LEFT_ALT, false), 40);
        assert!(!d.block && d.send.is_empty());
        let d = r.handle(key(VK_CTRL, hid::LEFT_CTRL, false), 50);
        assert!(!d.block && d.send.is_empty());

        // On the Mac now: Ctrl and Alt go to the Mac, M switches back.
        let d = r.handle(key(VK_CTRL, hid::LEFT_CTRL, true), 500);
        assert_eq!(d.send, vec![Msg::KeyDown(hid::LEFT_CTRL)]);
        let d = r.handle(key(VK_ALT, hid::LEFT_ALT, true), 510);
        assert_eq!(d.send, vec![Msg::KeyDown(hid::LEFT_ALT)]);
        let d = r.handle(key(VK_M, hid::M, true), 520);
        assert_eq!(d.send, vec![Msg::Leave]);
        assert_eq!(r.mode(), Mode::Local);
        assert!(r.handle(key(VK_M, hid::M, false), 530).block);
    }

    #[test]
    fn mouse_deltas_from_anchor() {
        let mut r = connected_router();
        r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        r.set_anchor(500, 400);
        let d = r.handle(Input::MouseMove { x: 510, y: 397 }, 1001);
        assert!(d.block);
        assert_eq!(d.send, vec![Msg::MouseMove { dx: 10, dy: -3 }]);
        // Cursor never actually moved (we blocked it), so the next event is again
        // relative to the anchor.
        let d = r.handle(Input::MouseMove { x: 501, y: 400 }, 1002);
        assert_eq!(d.send, vec![Msg::MouseMove { dx: 1, dy: 0 }]);
    }

    #[test]
    fn disconnect_returns_control() {
        let mut r = connected_router();
        r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        assert_eq!(r.set_connected(false), Some(Notice::LeftRemote));
        assert_eq!(r.mode(), Mode::Local);
        let d = r.handle(key(VK_A, 0x04, true), 1100);
        assert!(!d.block);
    }

    #[test]
    fn unmapped_key_is_swallowed_while_remote() {
        let mut r = connected_router();
        r.handle(Input::Button { button: button::FORWARD, down: true }, 1000);
        let d = r.handle(Input::Key { id: 0xFF, hid: None, down: true }, 1100);
        assert!(d.block && d.send.is_empty());
        assert_eq!(d.notice, Some(Notice::Unmapped(0xFF)));
    }
}
