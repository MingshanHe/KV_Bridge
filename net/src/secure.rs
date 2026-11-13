//! Encrypted sessions: Noise NNpsk0 (X25519, ChaCha20-Poly1305, BLAKE2s) keyed with the
//! pairing key. A peer without the key fails the handshake.

use crate::frame::{read_frame, write_frame, MAX};
use kvm_core::protocol::Msg;
use snow::{Builder, TransportState};
use std::io::{self, Cursor};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

const PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const TAG: usize = 16;

fn err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("encryption: {e}"))
}

fn handshake(mut stream: TcpStream, psk: &[u8; 32], initiator: bool) -> io::Result<(Sender, Receiver)> {
    let params = PARAMS.parse().map_err(err)?;
    let b = Builder::new(params).psk(0, psk);
    let mut hs = if initiator { b.build_initiator() } else { b.build_responder() }.map_err(err)?;
    let mut buf = vec![0u8; MAX];
    if initiator {
        let n = hs.write_message(&[], &mut buf).map_err(err)?;
        write_frame(&mut stream, &buf[..n])?;
        let m = read_frame(&mut stream)?;
        hs.read_message(&m, &mut buf).map_err(err)?;
    } else {
        let m = read_frame(&mut stream)?;
        hs.read_message(&m, &mut buf).map_err(err)?;
        let n = hs.write_message(&[], &mut buf).map_err(err)?;
        write_frame(&mut stream, &buf[..n])?;
    }
    let state = Arc::new(Mutex::new(hs.into_transport_mode().map_err(err)?));
    let reader = stream.try_clone()?;
    Ok((
        Sender { stream, state: state.clone(), buf: vec![0u8; MAX] },
        Receiver { stream: reader, state, buf: vec![0u8; MAX] },
    ))
}

/// Windows side (connects).
pub fn client(stream: TcpStream, psk: &[u8; 32]) -> io::Result<(Sender, Receiver)> {
    handshake(stream, psk, true)
}

/// Mac side (accepts).
pub fn server(stream: TcpStream, psk: &[u8; 32]) -> io::Result<(Sender, Receiver)> {
    handshake(stream, psk, false)
}

fn lock(s: &Mutex<TransportState>) -> std::sync::MutexGuard<'_, TransportState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sending half.
pub struct Sender {
    stream: TcpStream,
    state: Arc<Mutex<TransportState>>,
    buf: Vec<u8>,
}

impl Sender {
    /// Send messages, batched into as few encrypted frames as possible.
    pub fn send(&mut self, msgs: &[Msg]) -> io::Result<()> {
        // Split only between messages, so each frame decodes on its own.
        const LIMIT: usize = MAX - TAG - 16;
        let mut wire = Vec::new();
        let mut plain = Vec::with_capacity(64);
        let mut one = Vec::with_capacity(8);
        let mut it = msgs.iter().peekable();
        while let Some(m) = it.next() {
            one.clear();
            m.encode(&mut one);
            plain.extend_from_slice(&one);
            let next_len = it.peek().map(|_| 16).unwrap_or(0); // longest message is 8 bytes
            if it.peek().is_none() || plain.len() + next_len > LIMIT {
                let n = lock(&self.state).write_message(&plain, &mut self.buf).map_err(err)?;
                wire.extend_from_slice(&(n as u16).to_be_bytes());
                wire.extend_from_slice(&self.buf[..n]);
                plain.clear();
            }
        }
        use std::io::Write;
        self.stream.write_all(&wire)
    }

    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }
}

/// Receiving half.
pub struct Receiver {
    stream: TcpStream,
    state: Arc<Mutex<TransportState>>,
    buf: Vec<u8>,
}

impl Receiver {
    /// Wait for the next frame and decode the messages in it.
    pub fn recv(&mut self) -> io::Result<Vec<Msg>> {
        let f = read_frame(&mut self.stream)?;
        let n = lock(&self.state).read_message(&f, &mut self.buf).map_err(err)?;
        let mut cur = Cursor::new(&self.buf[..n]);
        let mut out = Vec::new();
        while (cur.position() as usize) < n {
            out.push(Msg::read_from(&mut cur)?);
        }
        Ok(out)
    }

    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn pair(psk_c: [u8; 32], psk_s: [u8; 32]) -> (io::Result<(Sender, Receiver)>, io::Result<(Sender, Receiver)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let srv = thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            server(s, &psk_s)
        });
        let c = client(TcpStream::connect(addr).unwrap(), &psk_c);
        (c, srv.join().unwrap())
    }

    #[test]
    fn messages_both_ways() {
        let (c, s) = pair([9; 32], [9; 32]);
        let (mut ctx, mut crx) = c.unwrap();
        let (mut stx, mut srx) = s.unwrap();
        ctx.send(&[Msg::Enter, Msg::KeyDown(4), Msg::MouseMove { dx: 3, dy: -2 }]).unwrap();
        ctx.send(&[Msg::Leave]).unwrap();
        assert_eq!(srx.recv().unwrap(), vec![Msg::Enter, Msg::KeyDown(4), Msg::MouseMove { dx: 3, dy: -2 }]);
        assert_eq!(srx.recv().unwrap(), vec![Msg::Leave]);
        stx.send(&[Msg::Heartbeat]).unwrap();
        assert_eq!(crx.recv().unwrap(), vec![Msg::Heartbeat]);
        // Lots of messages in one go.
        let many = vec![Msg::KeyUp(5); 30_000];
        ctx.send(&many).unwrap();
        let mut got = Vec::new();
        while got.len() < many.len() {
            got.extend(srx.recv().unwrap());
        }
        assert_eq!(got, many);
    }

    #[test]
    fn wrong_key_is_rejected() {
        let (c, s) = pair([1; 32], [2; 32]);
        assert!(c.is_err() || s.is_err());
        assert!(s.is_err());
    }
}
