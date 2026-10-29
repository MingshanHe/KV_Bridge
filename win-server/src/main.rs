//! kvm-win — the Windows side of kvm-bridge.
//!
//! Captures the keyboard and the Logitech Lift with low-level hooks. Press the Lift's
//! Forward thumb button (or Ctrl+Alt+M) to send everything to the Mac; press it again
//! to come back.

use kvm_core::protocol::button;
use std::net::IpAddr;

#[cfg(any(windows, kvm_check))]
mod app;
#[cfg(any(windows, kvm_check))]
mod ffi;

#[derive(Debug, Clone)]
pub struct Options {
    pub port: u16,
    pub toggle_button: Option<u8>,
    pub hotkey: bool,
    pub allow: Option<IpAddr>,
    pub log_keys: bool,
    pub quiet: bool,
}

const USAGE: &str = "\
kvm-win: share this Windows keyboard and mouse with a Mac

USAGE:
    kvm-win [OPTIONS]

OPTIONS:
    --port <N>           TCP port to listen on (default 24900)
    --button <B>         Lift button that switches control:
                         forward (default), back, or none
    --no-hotkey          Disable the Ctrl+Alt+M keyboard toggle
    --allow <IP>         Only accept a Mac connecting from this address
    --log                Print every key and button event (for testing)
    --quiet              No beep when switching
    -h, --help           Show this help
";

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        port: kvm_core::DEFAULT_PORT,
        toggle_button: Some(button::FORWARD),
        hotkey: true,
        allow: None,
        log_keys: false,
        quiet: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--port" => o.port = value("--port")?.parse().map_err(|_| "bad --port")?,
            "--button" => {
                o.toggle_button = match value("--button")?.as_str() {
                    "forward" => Some(button::FORWARD),
                    "back" => Some(button::BACK),
                    "none" => None,
                    other => return Err(format!("unknown button '{other}'")),
                }
            }
            "--no-hotkey" => o.hotkey = false,
            "--allow" => o.allow = Some(value("--allow")?.parse().map_err(|_| "bad --allow IP")?),
            "--log" => o.log_keys = true,
            "--quiet" => o.quiet = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if o.toggle_button.is_none() && !o.hotkey {
        return Err("--button none and --no-hotkey together leave no way to switch".into());
    }
    Ok(o)
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    #[cfg(any(windows, kvm_check))]
    app::run(opts);
    #[cfg(not(any(windows, kvm_check)))]
    {
        let _ = opts;
        eprintln!("kvm-win only runs on Windows.");
        std::process::exit(1);
    }
}
