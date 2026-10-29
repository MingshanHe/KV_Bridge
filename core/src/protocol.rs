//! Wire protocol.
//!
//! Every message is one type byte followed by a fixed-size, big-endian payload,
//! so the reader always knows how many bytes to expect.
//!
//! | id   | message    | payload                      |
//! |------|------------|------------------------------|
//! | 0x01 | Hello      | magic "KVMB" + version (u8)  |
//! | 0x02 | Enter      | –                            |
//! | 0x03 | Leave      | –                            |
//! | 0x10 | KeyDown    | HID usage (u16)              |
//! | 0x11 | KeyUp      | HID usage (u16)              |
//! | 0x20 | MouseMove  | dx (i16), dy (i16)           |
//! | 0x21 | Button     | button (u8), down (u8)       |
//! | 0x22 | Scroll     | dx (i16), dy (i16)           |
//! | 0x30 | Heartbeat  | –                            |

use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"KVMB";
pub const VERSION: u8 = 1;

/// Mouse buttons on the wire.
pub mod button {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const MIDDLE: u8 = 3;
    /// Thumb "Back" button (Windows XBUTTON1).
    pub const BACK: u8 = 4;
    /// Thumb "Forward" button (Windows XBUTTON2).
    pub const FORWARD: u8 = 5;
}

/// One unit of wheel movement on Windows (one notch).
pub const WHEEL_DELTA: i16 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Hello { version: u8 },
    /// Control has moved to the Mac.
    Enter,
    /// Control has moved back to Windows; the Mac must release everything it holds.
    Leave,
    KeyDown(u16),
    KeyUp(u16),
    MouseMove { dx: i16, dy: i16 },
    Button { button: u8, down: bool },
    /// Windows wheel units: 120 = one notch. Positive dy = away from the user (scroll up),
    /// positive dx = tilt right.
    Scroll { dx: i16, dy: i16 },
    Heartbeat,
}

impl Msg {
    pub fn hello() -> Msg {
        Msg::Hello { version: VERSION }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Msg::Hello { version } => {
                out.push(0x01);
                out.extend_from_slice(&MAGIC);
                out.push(version);
            }
            Msg::Enter => out.push(0x02),
            Msg::Leave => out.push(0x03),
            Msg::KeyDown(hid) => {
                out.push(0x10);
                out.extend_from_slice(&hid.to_be_bytes());
            }
            Msg::KeyUp(hid) => {
                out.push(0x11);
                out.extend_from_slice(&hid.to_be_bytes());
            }
            Msg::MouseMove { dx, dy } => {
                out.push(0x20);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            Msg::Button { button, down } => {
                out.push(0x21);
                out.push(button);
                out.push(down as u8);
            }
            Msg::Scroll { dx, dy } => {
                out.push(0x22);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            Msg::Heartbeat => out.push(0x30),
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let mut buf = Vec::with_capacity(8);
        self.encode(&mut buf);
        w.write_all(&buf)
    }

    /// Read exactly one message. Returns `InvalidData` for unknown ids or a bad Hello.
    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Msg> {
        let id = read_u8(r)?;
        let msg = match id {
            0x01 => {
                let mut magic = [0u8; 4];
                r.read_exact(&mut magic)?;
                if magic != MAGIC {
                    return Err(invalid("bad magic: peer is not kvm-bridge"));
                }
                Msg::Hello { version: read_u8(r)? }
            }
            0x02 => Msg::Enter,
            0x03 => Msg::Leave,
            0x10 => Msg::KeyDown(read_u16(r)?),
            0x11 => Msg::KeyUp(read_u16(r)?),
            0x20 => Msg::MouseMove { dx: read_i16(r)?, dy: read_i16(r)? },
            0x21 => Msg::Button { button: read_u8(r)?, down: read_u8(r)? != 0 },
            0x22 => Msg::Scroll { dx: read_i16(r)?, dy: read_i16(r)? },
            0x30 => Msg::Heartbeat,
            other => return Err(invalid(&format!("unknown message id 0x{other:02x}"))),
        };
        Ok(msg)
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16<R: Read>(r: &mut R) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_i16<R: Read>(r: &mut R) -> io::Result<i16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(i16::from_be_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_all_messages() {
        let msgs = [
            Msg::hello(),
            Msg::Enter,
            Msg::Leave,
            Msg::KeyDown(0x04),
            Msg::KeyUp(0xE3),
            Msg::MouseMove { dx: -300, dy: 17 },
            Msg::Button { button: button::FORWARD, down: true },
            Msg::Button { button: button::LEFT, down: false },
            Msg::Scroll { dx: 0, dy: -120 },
            Msg::Heartbeat,
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            m.encode(&mut buf);
        }
        let mut cur = Cursor::new(buf);
        for m in &msgs {
            assert_eq!(&Msg::read_from(&mut cur).unwrap(), m);
        }
        // Stream is exhausted: next read is a clean EOF error.
        assert_eq!(
            Msg::read_from(&mut cur).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rejects_garbage() {
        let mut cur = Cursor::new(vec![0xFFu8]);
        assert_eq!(
            Msg::read_from(&mut cur).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut cur = Cursor::new(b"\x01HTTP\x01".to_vec());
        assert_eq!(
            Msg::read_from(&mut cur).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
