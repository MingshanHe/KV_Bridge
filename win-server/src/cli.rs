//! The kvm-win command-line tool: plain TCP server for the kvm-mac command-line tool.
//! The KV Bridge app is the recommended way to run; this stays for testing.

use crate::Options;
use kvm_core::protocol::Msg;
use kvm_core::router::{Config, Notice};
use kvm_win::capture::{self, button_name, local_ip, Event};
use kvm_win::ffi::{wide, SetConsoleTitleW};
use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const HEARTBEAT: Duration = Duration::from_secs(1);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn run(opts: Options) {
    let listener = match TcpListener::bind(("0.0.0.0", opts.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot listen on port {}: {e}", opts.port);
            std::process::exit(1);
        }
    };
    capture::set_logging(opts.log_keys);
    let Some(cap) = capture::start(Config { toggle_button: opts.toggle_button, hotkey: opts.hotkey }) else {
        eprintln!("error: capture already running");
        std::process::exit(1);
    };
    print_banner(&opts);
    let quiet = opts.quiet;
    let events = cap.events;
    thread::spawn(move || ui_thread(events, quiet));
    let allow = opts.allow;
    net_thread(listener, allow, cap.outgoing);
}

fn print_banner(opts: &Options) {
    let ip = local_ip().unwrap_or_else(|| "<this PC's IP>".into());
    let switch = match (opts.toggle_button, opts.hotkey) {
        (Some(b), true) => format!("{} thumb button or Ctrl+Alt+M", button_name(b)),
        (Some(b), false) => format!("{} thumb button", button_name(b)),
        (None, _) => "Ctrl+Alt+M".into(),
    };
    println!("kvm-win {}", env!("CARGO_PKG_VERSION"));
    println!("Listening on port {}.", opts.port);
    println!("On the Mac, run:   ./kvm-mac {ip}");
    println!("Switch control:    {switch}");
    println!("Stop:              close this window or press Ctrl+C here (control returns to Windows)");
    println!();
    set_title("kvm-win — waiting for Mac");
}

fn set_title(t: &str) {
    let w = wide(t);
    unsafe {
        SetConsoleTitleW(w.as_ptr());
    }
}

fn net_thread(listener: TcpListener, allow: Option<std::net::IpAddr>, rx: Receiver<Msg>) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                println!("accept failed: {e}");
                continue;
            }
        };
        let peer = match stream.peer_addr() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Some(ip) = allow {
            if peer.ip() != ip {
                println!("rejected connection from {} (not --allow {ip})", peer.ip());
                continue;
            }
        }
        println!("Mac connected from {}", peer.ip());
        set_title("kvm-win — Mac connected (controlling Windows)");
        serve(stream, &rx);
        capture::set_connected(false);
        println!("Mac disconnected; waiting for it to reconnect");
        set_title("kvm-win — waiting for Mac");
    }
}

fn serve(mut stream: TcpStream, rx: &Receiver<Msg>) {
    let _ = stream.set_nodelay(true);
    // Drop anything queued while no Mac was attached.
    while rx.try_recv().is_ok() {}

    let alive = Arc::new(AtomicBool::new(true));
    // Reader: the Mac sends heartbeats; silence means it's gone (sleep, Wi-Fi drop).
    if let Ok(mut rs) = stream.try_clone() {
        let alive = alive.clone();
        let _ = rs.set_read_timeout(Some(PEER_TIMEOUT));
        thread::spawn(move || {
            loop {
                match Msg::read_from(&mut rs) {
                    Ok(Msg::Hello { version }) if version != kvm_core::protocol::VERSION => {
                        println!(
                            "Mac app is protocol v{version}, this is v{}; update both",
                            kvm_core::protocol::VERSION
                        );
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            alive.store(false, Ordering::Relaxed);
            let _ = rs.shutdown(Shutdown::Both);
        });
    }

    if Msg::hello().write_to(&mut stream).is_err() {
        return;
    }
    capture::set_connected(true);

    let mut buf = Vec::with_capacity(256);
    while alive.load(Ordering::Relaxed) {
        buf.clear();
        match rx.recv_timeout(HEARTBEAT) {
            Ok(m) => {
                m.encode(&mut buf);
                // Batch whatever else is already queued into one write.
                while let Ok(m) = rx.try_recv() {
                    m.encode(&mut buf);
                }
            }
            Err(RecvTimeoutError::Timeout) => Msg::Heartbeat.encode(&mut buf),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if stream.write_all(&buf).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn ui_thread(rx: Receiver<Event>, quiet: bool) {
    for ev in rx {
        match ev {
            Event::Log(s) => println!("{s}"),
            Event::Notice(n) => {
                match n {
                    Notice::EnteredRemote => {
                        println!(">>> Controlling the MAC");
                        set_title("kvm-win — controlling MAC");
                    }
                    Notice::LeftRemote => {
                        println!("<<< Controlling WINDOWS");
                        set_title("kvm-win — Mac connected (controlling Windows)");
                    }
                    Notice::NotConnected => println!("No Mac connected yet; staying on Windows"),
                    Notice::Unmapped(id) => println!("(key 0x{id:x} has no Mac equivalent; ignored)"),
                }
                if !quiet {
                    capture::beep(&n);
                }
            }
        }
    }
}
