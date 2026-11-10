//! The first frame of every connection (plaintext), and the server's reply.
//!
//! Client -> server: `KVB2` + kind + id + name   (kind: 'P' pair, 'C' connect)
//! Server -> client: 1 + its id + name = go ahead, or 0 + a reason (UTF-8) and the
//! connection closes.

use crate::frame::{read_frame, write_frame};
use std::io::{self, Read, Write};

const MAGIC: &[u8; 4] = b"KVB2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hello {
    /// "Let's pair": the client's id and display name.
    Pair { id: String, name: String },
    /// "Reconnect": the client's id (the server looks up the shared key).
    Connect { id: String, name: String },
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(255);
    buf.push(n as u8);
    buf.extend_from_slice(&b[..n]);
}

fn get_str(data: &[u8], pos: &mut usize) -> io::Result<String> {
    let n = *data.get(*pos).ok_or_else(bad)? as usize;
    let s = data.get(*pos + 1..*pos + 1 + n).ok_or_else(bad)?;
    *pos += 1 + n;
    Ok(String::from_utf8_lossy(s).into_owned())
}

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "not a KV Bridge peer")
}

impl Hello {
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let mut buf = MAGIC.to_vec();
        let (kind, id, name) = match self {
            Hello::Pair { id, name } => (b'P', id, name),
            Hello::Connect { id, name } => (b'C', id, name),
        };
        buf.push(kind);
        put_str(&mut buf, id);
        put_str(&mut buf, name);
        write_frame(w, &buf)
    }

    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Hello> {
        let data = read_frame(r)?;
        if data.len() < 5 || &data[..4] != MAGIC {
            return Err(bad());
        }
        let mut pos = 5;
        let id = get_str(&data, &mut pos)?;
        let name = get_str(&data, &mut pos)?;
        match data[4] {
            b'P' => Ok(Hello::Pair { id, name }),
            b'C' => Ok(Hello::Connect { id, name }),
            _ => Err(bad()),
        }
    }
}

/// Server: accept the hello, saying who we are.
pub fn accept<W: Write>(w: &mut W, my_id: &str, my_name: &str) -> io::Result<()> {
    let mut b = vec![1u8];
    put_str(&mut b, my_id);
    put_str(&mut b, my_name);
    write_frame(w, &b)
}

/// Server: refuse with a reason the client can show.
pub fn refuse<W: Write>(w: &mut W, reason: &str) -> io::Result<()> {
    let mut b = vec![0u8];
    b.extend_from_slice(reason.as_bytes());
    write_frame(w, &b)
}

/// Client: wait for the server's answer: Ok((id, name)) or Err(reason) if refused.
pub fn read_answer<R: Read>(r: &mut R) -> io::Result<Result<(String, String), String>> {
    let f = read_frame(r)?;
    match f.first() {
        Some(1) => {
            let mut pos = 1;
            let id = get_str(&f, &mut pos)?;
            let name = get_str(&f, &mut pos)?;
            Ok(Ok((id, name)))
        }
        Some(0) => Ok(Err(String::from_utf8_lossy(&f[1..]).into_owned())),
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for h in [
            Hello::Pair { id: "abc".into(), name: "My PC".into() },
            Hello::Connect { id: "0123456789abcdef".into(), name: "MacBook 的电脑".into() },
        ] {
            let mut buf = Vec::new();
            h.write_to(&mut buf).unwrap();
            assert_eq!(Hello::read_from(&mut io::Cursor::new(buf)).unwrap(), h);
        }
        let mut buf = Vec::new();
        accept(&mut buf, "mac1", "Mac").unwrap();
        assert_eq!(read_answer(&mut io::Cursor::new(buf)).unwrap(), Ok(("mac1".into(), "Mac".into())));
        let mut buf = Vec::new();
        refuse(&mut buf, "not pairing").unwrap();
        assert_eq!(read_answer(&mut io::Cursor::new(buf)).unwrap(), Err("not pairing".into()));
    }
}
