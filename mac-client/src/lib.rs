//! Mac side of kvm-bridge: turns protocol messages into keyboard and mouse input.
//!
//! * [`state`]    – pure event logic (shortcut rules, layouts, mouse), unit tested.
//! * [`platform`] – CoreGraphics / Accessibility glue and [`platform::Output`] (macOS).

pub mod state;
#[cfg(any(target_os = "macos", kvm_check))]
pub mod ffi;
#[cfg(any(target_os = "macos", kvm_check))]
pub mod platform;
