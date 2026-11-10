//! The kvm-mac command-line tool (plain TCP, no pairing). The KV Bridge app is the
//! recommended way to run the Mac side; this stays for testing.

use crate::Options;
use kvm_core::protocol::{Msg, VERSION};
use kvm_mac::platform::{accessibility_ok, is_timeout, Event, Output};
use kvm_mac::state::{Injector, NoCtx, Settings};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::thread;
use std::time::Duration;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_secs(2);

pub fn run(opts: Options) {
    if !accessibility_ok(true) {
        eprintln!(
            "kvm-mac needs the Accessibility permission to type and click.\n\
             System Settings has been asked to show the prompt. Then:\n  \
             System Settings > Privacy & Security > Accessibility\n  \
             turn on the app you ran this from (e.g. Terminal), then run kvm-mac again."
        );
        std::process::exit(1);
    }

    let mut out = Output::new(opts.settings.clone(), opts.verbose);
    if opts.test_type {
        test_type(&out);
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
    loop {
        println!("Connecting to {addr} ...");
        match TcpStream::connect_timeout(&addr, PEER_TIMEOUT) {
            Ok(stream) => {
                println!("Connected. Press the Lift button on Windows to control this Mac.");
                let reason = session(stream, &mut out);
                out.reset();
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

fn session(mut stream: TcpStream, out: &mut Output) -> String {
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

    let reason = loop {
        let msg = match Msg::read_from(&mut stream) {
            Ok(m) => m,
            Err(e) if is_timeout(&e) => break "no heartbeat from Windows".to_string(),
            Err(e) => break e.to_string(),
        };
        match out.handle(msg) {
            Some(Event::Entered { layout }) => {
                println!(">>> Windows keyboard and mouse are controlling this Mac");
                match layout {
                    Some(l) => println!("    {l} keyboard: symbols are typed as on the Windows keyboard"),
                    None => println!("    keyboard layout not translated (keys sent by position)"),
                }
            }
            Some(Event::Left) => println!("<<< Control returned to Windows"),
            None => {}
        }
    };
    let _ = stream.shutdown(Shutdown::Both);
    reason
}

/// Milestone-1 check: types a sentence into whatever app is focused.
fn test_type(out: &Output) {
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
            out.post(&inj.handle(msg, 0, &mut NoCtx));
            thread::sleep(Duration::from_millis(15));
        }
    }
    println!("Done. If nothing was typed, check the Accessibility permission.");
}
