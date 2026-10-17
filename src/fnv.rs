//! FNV-1a, 64-bit: a change detector, not a defence. Used where a hash is written down and compared
//! with one an older build wrote — std's hasher is free to change between Rust releases.

pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    /// One field, length first, so `ab`+`c` and `a`+`bc` differ.
    pub fn field(&mut self, bytes: &[u8]) {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// `bytes` as one field, in hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut hash = Fnv::new();
    hash.field(bytes);
    hash.hex()
}
