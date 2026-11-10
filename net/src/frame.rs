//! Frames: a big-endian u16 length followed by that many bytes.

use std::io::{self, Read, Write};

pub const MAX: usize = u16::MAX as usize;

pub fn write_frame<W: Write>(w: &mut W, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
    }
    let mut buf = Vec::with_capacity(2 + data.len());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    w.write_all(&buf)
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)?;
    let mut data = vec![0u8; u16::from_be_bytes(len) as usize];
    r.read_exact(&mut data)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut c = io::Cursor::new(buf);
        assert_eq!(read_frame(&mut c).unwrap(), b"hello");
        assert_eq!(read_frame(&mut c).unwrap(), b"");
        assert!(read_frame(&mut c).is_err());
    }
}
