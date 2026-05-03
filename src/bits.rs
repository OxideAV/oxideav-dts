//! Big-endian MSB-first bit reader used throughout the DTS Core
//! decoder. The bitstream packs MSB-first within bytes and reads can
//! straddle byte boundaries arbitrarily — so we shift on demand
//! rather than maintaining a cached word.
//!
//! Returns `None` on EOS.

use oxideav_core::{Error, Result};

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bit cursor — `bit_pos / 8` is the byte index, `bit_pos % 8` is
    /// the bit-within-byte (MSB = 0).
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub fn bits_remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bit_pos)
    }

    pub fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    pub fn byte_align(&mut self) {
        let r = self.bit_pos & 7;
        if r != 0 {
            self.bit_pos += 8 - r;
        }
    }

    /// Skip `n` bits.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.bits_remaining() < n {
            return Err(Error::invalid("dts: bit reader underflow"));
        }
        self.bit_pos += n;
        Ok(())
    }

    /// Read up to 32 bits, MSB-first, and return as `u32`.
    pub fn read(&mut self, n: usize) -> Result<u32> {
        debug_assert!(n <= 32);
        if self.bits_remaining() < n {
            return Err(Error::invalid("dts: bit reader underflow"));
        }
        let mut acc: u32 = 0;
        let mut left = n;
        while left > 0 {
            let byte_idx = self.bit_pos >> 3;
            let bit_idx = self.bit_pos & 7;
            let byte = self.data[byte_idx] as u32;
            let avail = 8 - bit_idx;
            let take = left.min(avail);
            let shift = avail - take;
            let mask = (1u32 << take) - 1;
            let chunk = (byte >> shift) & mask;
            acc = (acc << take) | chunk;
            self.bit_pos += take;
            left -= take;
        }
        Ok(acc)
    }

    /// Read `n` bits as a signed two's-complement value.
    pub fn read_signed(&mut self, n: usize) -> Result<i32> {
        let raw = self.read(n)?;
        let sign_bit = 1u32 << (n - 1);
        let val = if raw & sign_bit != 0 {
            (raw as i64) - (1i64 << n)
        } else {
            raw as i64
        };
        Ok(val as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_aligned_byte() {
        let data = [0xAB, 0xCD];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(8).unwrap(), 0xAB);
        assert_eq!(r.read(8).unwrap(), 0xCD);
    }

    #[test]
    fn read_straddle() {
        let data = [0b1100_0011, 0b1010_0101];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(4).unwrap(), 0b1100);
        assert_eq!(r.read(4).unwrap(), 0b0011);
        assert_eq!(r.read(8).unwrap(), 0b1010_0101);
    }

    #[test]
    fn read_unaligned_long() {
        // 0x7FFE_8001 sync word — verify cross-byte reads.
        let data = [0x7F, 0xFE, 0x80, 0x01, 0xFF];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(32).unwrap(), 0x7FFE_8001);
    }

    #[test]
    fn signed_read() {
        // 0b1110 with n=4: top bit set → negative.
        let data = [0b1110_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_signed(4).unwrap(), -2);
    }

    #[test]
    fn underflow_errors() {
        let data = [0x00];
        let mut r = BitReader::new(&data);
        r.read(8).unwrap();
        assert!(r.read(1).is_err());
    }
}
