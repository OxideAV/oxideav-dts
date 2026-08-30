//! MSB-first bit writer — the encoder-side mirror of
//! [`crate::bitreader::BitReader`].
//!
//! The DTS Core bitstream is a plain MSB-first bit sequence (§5.2 /
//! Table 5-1 `ExtractBits`); this writer appends fields in the same
//! order and orientation the crate's readers consume them.

/// MSB-first bit sink.
#[derive(Debug, Clone, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    /// Empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writer seeded with whole bytes already produced elsewhere
    /// (e.g. the §5.3.1 header from
    /// [`crate::encode_frame_header_be`]); subsequent pushes append
    /// after them.
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let bit_len = bytes.len() * 8;
        Self { bytes, bit_len }
    }

    /// Bits written so far.
    #[must_use]
    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    /// Append the low `width` bits of `value`, MSB-first.
    ///
    /// # Panics
    ///
    /// If `width > 32`, or if `value` has bits above `width` set (a
    /// field-overflow programming error the §5.x tables never allow).
    pub fn push(&mut self, value: u32, width: u32) {
        assert!(width <= 32, "field width {width} > 32");
        if width < 32 {
            assert!(
                value >> width == 0,
                "value {value:#x} does not fit in {width} bits"
            );
        }
        for i in (0..width).rev() {
            let bit = ((value >> i) & 1) as u8;
            if self.bit_len % 8 == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.last_mut().expect("pushed above");
            *last |= bit << (7 - (self.bit_len % 8));
            self.bit_len += 1;
        }
    }

    /// Append a signed value as `width`-bit two's complement (the
    /// §5.5 "no further encoding" / LFE sample form).
    ///
    /// # Panics
    ///
    /// If the value does not fit in `width` signed bits.
    pub fn push_signed(&mut self, value: i32, width: u32) {
        assert!((1..=32).contains(&width));
        let min = -(1i64 << (width - 1));
        let max = (1i64 << (width - 1)) - 1;
        assert!(
            (min..=max).contains(&i64::from(value)),
            "value {value} does not fit in {width} signed bits"
        );
        let mask = if width == 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        self.push((value as u32) & mask, width);
    }

    /// Zero-pad to exactly `len` bytes.
    ///
    /// # Panics
    ///
    /// If more than `len * 8` bits were already written.
    pub fn pad_to_bytes(&mut self, len: usize) {
        assert!(
            self.bit_len <= len * 8,
            "{} bits already written, cannot pad to {len} bytes",
            self.bit_len
        );
        self.bytes.resize(len, 0);
        self.bit_len = len * 8;
    }

    /// Finish, returning the byte buffer (final partial byte
    /// zero-padded on the right).
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitreader::BitReader;

    #[test]
    fn round_trips_through_bitreader() {
        let mut w = BitWriter::new();
        w.push(0b101, 3);
        w.push(0x7FFE8001, 32);
        w.push_signed(-3, 5);
        w.push(0, 1);
        assert_eq!(w.bit_len(), 41);
        let bytes = w.into_bytes();
        let mut r = BitReader::from_byte_offset(&bytes, 0);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(32).unwrap(), 0x7FFE8001);
        let raw = r.read_bits(5).unwrap();
        // Sign-extend 5 bits.
        assert_eq!(((raw << 27) as i32) >> 27, -3);
    }

    #[test]
    fn from_bytes_appends_after_seed() {
        let mut w = BitWriter::from_bytes(vec![0xAB]);
        w.push(0xF, 4);
        assert_eq!(w.into_bytes(), vec![0xAB, 0xF0]);
    }

    #[test]
    fn pad_to_bytes_zero_fills() {
        let mut w = BitWriter::new();
        w.push(1, 1);
        w.pad_to_bytes(3);
        let b = w.into_bytes();
        assert_eq!(b, vec![0x80, 0, 0]);
    }

    #[test]
    #[should_panic(expected = "does not fit")]
    fn overflowing_field_panics() {
        BitWriter::new().push(4, 2);
    }
}
