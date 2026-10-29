//! kvm-mac — the Mac side of kvm-bridge.
//!
//! Connects to kvm-win on the Windows laptop and replays its keyboard and mouse here
//! whenever control has been switched to the Mac.

#[cfg(any(target_os = "macos", kvm_check))]
mod app;
#[cfg(any(target_os = "macos", kvm_check))]
mod ffi;
mod state;

#[derive(Debug, Clone)]
pub struct Options {
    pub host: Option<String>,
    pub port: u16,
    pub settings: state::Settings,
    pub verbose: bool,
    pub test_type: bool,
}

const USAGE: &str = "\
kvm-mac: let the Windows laptop's keyboard and mouse control this Mac

USAGE:
    kvm-mac <WINDOWS-IP> [OPTIONS]
    kvm-mac --test-type

OPTIONS:
    --port <N>            Port kvm-win listens on (default 24900)
    --no-swap             Keep Ctrl as Ctrl (default: Ctrl->Cmd, Win->Ctrl)
    --invert-scroll       Reverse the scroll direction
    --scroll-speed <PX>   Pixels per wheel notch (default 40)
    --jis                 Japanese keyboard: 英数/Caps -> Eisu, 半角/全角 toggles Eisu/Kana
    --verbose             Print every message received
    --test-type           Type a test sentence here after 3 s, then exit
                          (checks the Accessibility permission without Windows)
    -h, --help            Show this help
";

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        host: None,
        port: kvm_core::DEFAULT_PORT,
        settings: state::Settings::default(),
        verbose: false,
        test_type: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--port" => o.port = value("--port")?.parse().map_err(|_| "bad --port")?,
            "--no-swap" => o.settings.swap_ctrl_cmd = false,
            "--invert-scroll" => o.settings.invert_scroll = true,
            "--scroll-speed" => {
                let v: f64 = value("--scroll-speed")?.parse().map_err(|_| "bad --scroll-speed")?;
                if !(v > 0.0 && v <= 1000.0) {
                    return Err("--scroll-speed must be between 0 and 1000".into());
                }
                o.settings.scroll_px_per_notch = v;
            }
            "--jis" => o.settings.jis = true,
            "--verbose" => o.verbose = true,
            "--test-type" => o.test_type = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
            s => {
                if o.host.replace(s.to_string()).is_some() {
                    return Err("give only one Windows address".into());
                }
            }
        }
    }
    if o.host.is_none() && !o.test_type {
        return Err("missing the Windows laptop's IP address".into());
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
    #[cfg(any(target_os = "macos", kvm_check))]
    app::run(opts);
    #[cfg(not(any(target_os = "macos", kvm_check)))]
    {
        let _ = opts;
        eprintln!("kvm-mac only runs on macOS.");
        std::process::exit(1);
    }
}
