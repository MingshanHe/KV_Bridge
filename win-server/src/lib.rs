//! Windows side of kvm-bridge: capturing the keyboard and mouse and deciding which
//! machine gets each event.

#[cfg(any(windows, kvm_check))]
pub mod capture;
#[cfg(any(windows, kvm_check))]
pub mod ffi;
