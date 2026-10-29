//! Shared code for kvm-bridge.
//!
//! * [`protocol`] – the wire format between the Windows sender and the Mac receiver.
//! * [`keymap`]   – Windows scan code -> USB HID usage -> macOS virtual keycode tables.
//! * [`router`]   – the Windows-side decision logic: which machine gets each input event,
//!   and when the Logitech Lift button / hotkey switches control.

pub mod keymap;
pub mod protocol;
pub mod router;

/// Default TCP port. (Synergy/Deskflow use 24800; we stay off it so both can coexist.)
pub const DEFAULT_PORT: u16 = 24900;
