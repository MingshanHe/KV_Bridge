//! Networking for the KV Bridge app.
//!
//! * [`frame`]     – length-prefixed frames on a TCP stream.
//! * [`hello`]     – the first (plaintext) frame: "pair with me" or "connect, I'm X".
//! * [`pair`]      – pairing with a 6-digit code (SPAKE2): both sides end up with the
//!   same secret key without the code or the key ever crossing the network.
//! * [`secure`]    – encrypted sessions (Noise NNpsk0) using that key.
//! * [`discovery`] – finding the other computer on the LAN (mDNS / Bonjour).

pub mod discovery;
pub mod frame;
pub mod hello;
pub mod pair;
pub mod secure;

/// TCP port the Mac app listens on.
pub const PORT: u16 = 24901;

/// A random device id (16 hex characters).
pub fn new_device_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    to_hex(&b)
}

/// A random 6-digit pairing code.
pub fn new_pairing_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32))
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_roundtrip() {
        let k = [7u8; 32];
        assert_eq!(super::from_hex32(&super::to_hex(&k)), Some(k));
        assert_eq!(super::from_hex32("zz"), None);
        assert_eq!(super::new_pairing_code().len(), 6);
        assert_eq!(super::new_device_id().len(), 16);
    }
}
