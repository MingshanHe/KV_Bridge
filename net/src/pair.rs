//! Pairing with a short code.
//!
//! The Mac shows a 6-digit code; the user types it on Windows. Both run SPAKE2 with
//! the code as password: each side learns the same 32-byte key only if the codes
//! match, and someone watching the network learns nothing they could brute-force
//! offline. Both then prove they have the key before saving it.

use crate::frame::{read_frame, write_frame};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::fmt;
use std::io::{self, Read, Write};

/// The result of a successful pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// The shared key for later sessions.
    pub psk: [u8; 32],
}

#[derive(Debug)]
pub enum PairError {
    Io(io::Error),
    /// The codes didn't match.
    WrongCode,
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairError::Io(e) => write!(f, "{e}"),
            PairError::WrongCode => write!(f, "wrong pairing code"),
        }
    }
}

impl From<io::Error> for PairError {
    fn from(e: io::Error) -> Self {
        PairError::Io(e)
    }
}

const ID_A: &[u8] = b"kvbridge-pair-windows";
const ID_B: &[u8] = b"kvbridge-pair-mac";

fn confirm(role: &[u8], key: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"kvbridge confirm v1");
    h.update(role);
    h.update(key);
    h.finalize().into()
}

fn derive_psk(key: &[u8], client_id: &str, server_id: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"kvbridge psk v1");
    h.update(key);
    h.update([client_id.len() as u8]);
    h.update(client_id.as_bytes());
    h.update([server_id.len() as u8]);
    h.update(server_id.as_bytes());
    h.finalize().into()
}

/// Run the exchange. `client` = the side that typed the code (Windows).
fn exchange<S: Read + Write>(
    s: &mut S,
    client: bool,
    code: &str,
    client_id: &str,
    server_id: &str,
) -> Result<Paired, PairError> {
    let pw = Password::new(code.trim().as_bytes());
    let (state, out) = if client {
        Spake2::<Ed25519Group>::start_a(&pw, &Identity::new(ID_A), &Identity::new(ID_B))
    } else {
        Spake2::<Ed25519Group>::start_b(&pw, &Identity::new(ID_A), &Identity::new(ID_B))
    };
    write_frame(s, &out)?;
    let inbound = read_frame(s)?;
    let key = state.finish(&inbound).map_err(|_| PairError::WrongCode)?;

    let (mine, theirs) = if client { (&b"A"[..], &b"B"[..]) } else { (&b"B"[..], &b"A"[..]) };
    write_frame(s, &confirm(mine, &key))?;
    let got = read_frame(s)?;
    if got.as_slice() != confirm(theirs, &key) {
        return Err(PairError::WrongCode);
    }
    Ok(Paired { psk: derive_psk(&key, client_id, server_id) })
}

/// Windows side: after sending `Hello::Pair` and being accepted.
pub fn as_client<S: Read + Write>(
    s: &mut S,
    code: &str,
    my_id: &str,
    mac_id: &str,
) -> Result<Paired, PairError> {
    exchange(s, true, code, my_id, mac_id)
}

/// Mac side: after accepting a `Hello::Pair` from `windows_id`.
pub fn as_server<S: Read + Write>(
    s: &mut S,
    code: &str,
    my_id: &str,
    windows_id: &str,
) -> Result<Paired, PairError> {
    exchange(s, false, code, windows_id, my_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn run(code_win: &'static str, code_mac: &'static str) -> (Result<Paired, PairError>, Result<Paired, PairError>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let mac = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            as_server(&mut s, code_mac, "mac1", "win1")
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let w = as_client(&mut c, code_win, "win1", "mac1");
        (w, mac.join().unwrap())
    }

    #[test]
    fn same_code_gives_same_key() {
        let (w, m) = run("123456", "123456");
        let (w, m) = (w.unwrap(), m.unwrap());
        assert_eq!(w, m);
        assert_ne!(w.psk, [0u8; 32]);
    }

    #[test]
    fn different_code_fails_on_both_sides() {
        let (w, m) = run("123456", "654321");
        assert!(matches!(w, Err(PairError::WrongCode) | Err(PairError::Io(_))));
        assert!(matches!(m, Err(PairError::WrongCode) | Err(PairError::Io(_))));
        assert!(w.is_err() && m.is_err());
    }
}
