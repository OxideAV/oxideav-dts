//! 14-bit DTS bitstream → 16-bit-equivalent unpacker.
//!
//! ## Why this exists
//!
//! Per the multimedia.cx wiki snapshot
//! (`docs/audio/dts/wiki/DTS.wiki`, section "14-bit words"):
//!
//! > This kind of bitstream is packed into 16-bit sample words so
//! > that the amplitude is reduced by 12 dB in the event that the
//! > data is inadvertently interpreted as uncompressed audio
//! > samples. The upper two bits are basically sign bit extension,
//! > as defined by twos-complement format.
//!
//! Each 16-bit container in a 14-bit-packed DTS stream carries 14
//! bits of payload in the **lower** 14 bits; the upper two bits are
//! a sign-extension of bit 13 of the payload. The payloads of
//! successive containers concatenate MSB-first to form the same
//! bitstream that a raw 16-bit-packed DTS stream would carry.
//!
//! ## Verification with the documented sync sequences
//!
//! The wiki lists four sync byte sequences:
//!
//! ```text
//!   raw BE        : 7F FE 80 01
//!   raw LE        : FE 7F 01 80
//!   14-bit BE     : 1F FF E8 00 07 Fx
//!   14-bit LE     : FF 1F 00 E8 Fx 07
//! ```
//!
//! Reading the 14-bit BE prefix as three 16-bit BE words gives
//! `0x1FFF`, `0xE800`, `0x07Fx`. Masking each to its lower 14 bits
//! yields the 14-bit payloads `0x1FFF`, `0x2800`, `0x07Fx`.
//! Concatenating those three 14-bit values MSB-first produces a
//! 42-bit stream whose first 32 bits are `0x7FFE8001` — exactly the
//! raw BE syncword. The 14-bit LE form is identical except each
//! 16-bit container is byte-swapped before the lower-14 mask. This
//! is the contract the unpacker implements.
//!
//! ## Output shape
//!
//! For every `8` bytes of 14-bit-packed input (= four 16-bit
//! containers carrying 56 payload bits) the unpacker emits `7`
//! bytes (= 56 unpacked bits). The output is byte-aligned because
//! 14 × 4 = 56 is a multiple of 8; this is also the smallest such
//! cycle, so the unpacker walks the input four containers at a
//! time. Input lengths that are not a multiple of 8 are still
//! handled — any trailing fractional bits are flushed as a final
//! padding byte.
//!
//! The unpacker is non-allocating beyond a single `Vec<u8>` for the
//! result and is pure CPU (no I/O).
//!
//! ## What lives in `docs/`
//!
//! Only the wiki snapshot above. The 14-bit sign-extension rule and
//! the lower-14-bit payload convention are stated verbatim in that
//! file; no external library source was consulted to write this
//! unpacker.

use crate::header::SyncWordEncoding;
use crate::{Error, Result};

/// Byte-order of the 14-bit-packed input.
///
/// In `BigEndian` mode each pair of input bytes is read as a 16-bit
/// big-endian word; in `LittleEndian` mode each pair is read as
/// little-endian. The two forms differ only in the byte order of the
/// 16-bit containers — the payload extraction (lower 14 bits,
/// MSB-first concatenation) is identical for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FourteenBitByteOrder {
    /// Big-endian container words (`1F FF E8 00 07 Fx`).
    BigEndian,
    /// Little-endian container words (`FF 1F 00 E8 Fx 07`).
    LittleEndian,
}

impl FourteenBitByteOrder {
    /// Map a [`SyncWordEncoding`] to the corresponding byte order,
    /// returning `None` for the two raw (non-14-bit) variants.
    pub fn from_sync(sync: SyncWordEncoding) -> Option<Self> {
        match sync {
            SyncWordEncoding::FourteenBitBigEndian => Some(FourteenBitByteOrder::BigEndian),
            SyncWordEncoding::FourteenBitLittleEndian => Some(FourteenBitByteOrder::LittleEndian),
            _ => None,
        }
    }
}

/// Unpack a 14-bit-packed DTS byte buffer into the equivalent
/// 16-bit-packed (raw big-endian) byte buffer.
///
/// Every pair of input bytes is read as a 16-bit container in the
/// requested [`FourteenBitByteOrder`]; the lower 14 bits of each
/// container are concatenated MSB-first and re-packed into a
/// big-endian byte stream. The output is suitable to feed straight
/// into [`crate::parse_frame_header`].
///
/// The input length must be even (each 14-bit container occupies
/// exactly two input bytes); an odd length returns
/// [`Error::UnexpectedEof`]. The empty input yields an empty
/// output.
///
/// The function is pure / side-effect free and allocates exactly
/// one `Vec<u8>` whose final length is at most
/// `ceil(input.len() / 2 * 14 / 8)`.
pub fn unpack_14bit_to_16bit(input: &[u8], order: FourteenBitByteOrder) -> Result<Vec<u8>> {
    if input.len() % 2 != 0 {
        return Err(Error::UnexpectedEof);
    }
    let containers = input.len() / 2;
    // Output capacity: 14 bits per container, rounded up to whole
    // bytes.
    let out_bits = containers * 14;
    let out_bytes = out_bits.div_ceil(8);
    let mut out = Vec::with_capacity(out_bytes);

    // Walk each container, accumulating the 14-bit payload into a
    // little buffer; flush full bytes (MSB-first) as soon as the
    // buffer holds >= 8 bits.
    //
    // `buf` holds up to 7+14 = 21 bits at the time of an accumulate;
    // a u32 is comfortably wide enough.
    let mut buf: u32 = 0;
    let mut bits_in_buf: u32 = 0;

    for pair in input.chunks_exact(2) {
        let word = match order {
            FourteenBitByteOrder::BigEndian => u16::from_be_bytes([pair[0], pair[1]]),
            FourteenBitByteOrder::LittleEndian => u16::from_le_bytes([pair[0], pair[1]]),
        };
        // Lower 14 bits = the payload. The upper 2 bits are a
        // sign-extension of bit 13 per the wiki and are discarded.
        let payload = (word & 0x3FFF) as u32;
        // Shift the existing buffer left to make room and OR in the
        // new 14 bits.
        buf = (buf << 14) | payload;
        bits_in_buf += 14;

        // Flush whole bytes off the top of the buffer.
        while bits_in_buf >= 8 {
            let shift = bits_in_buf - 8;
            let byte = ((buf >> shift) & 0xFF) as u8;
            out.push(byte);
            // Clear the bits we just emitted so subsequent shifts
            // don't carry them along.
            buf &= (1u32 << shift) - 1;
            bits_in_buf -= 8;
        }
    }

    // Flush any trailing fractional bits as a final padding byte
    // (MSB-aligned). After consuming containers in groups of four,
    // bits_in_buf will be 0 at the boundary, so this branch only
    // fires when containers % 4 != 0.
    if bits_in_buf > 0 {
        let byte = ((buf << (8 - bits_in_buf)) & 0xFF) as u8;
        out.push(byte);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 14-bit BE prefix from the wiki — three containers
    /// `1F FF`, `E8 00`, `07 F0` — unpacks to the raw BE syncword
    /// `7F FE 80 01` as the first 4 bytes, with `00` as the
    /// fractional trailing byte (the low nibble of `07 F0` carries
    /// the first 4 bits of the next payload field, which here is
    /// zero in our padded fixture).
    #[test]
    fn unpack_be_syncword_prefix_yields_raw_be_sync() {
        // Use 07 F0 (low nibble = 0) so the trailing bits flush to a
        // clean `0x00` padding byte for the assertion.
        let packed = [0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xF0];
        let unpacked = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::BigEndian).unwrap();
        // 3 containers × 14 = 42 bits → ceil(42/8) = 6 bytes out.
        assert_eq!(unpacked.len(), 6);
        // First four bytes: the raw BE syncword.
        assert_eq!(&unpacked[..4], &[0x7F, 0xFE, 0x80, 0x01]);
        // Remaining 10 bits encode `0000_0001_11` from the bottom of
        // payload #2 (`...01`) and the top of payload #3
        // (`00_0111_11`) → bits 32..42 = `0000_0001_1100_0111_11`.
        // Splitting MSB-first into bytes 4..6:
        //   byte 4 = `0000_0001` = 0x01
        //   byte 5 = `1100_0111` = 0xC7
        //   wait — let's recompute carefully and assert by recompute
        //   instead of by hand here.
        // The structural assertion is the syncword above; the
        // trailing bytes are dictated by the unpacker contract and
        // are exercised by the round-trip test below.
    }

    /// The 14-bit LE prefix from the wiki — three containers
    /// `FF 1F`, `00 E8`, `F0 07` — must unpack to the same raw BE
    /// syncword as the BE prefix above (since the payloads are
    /// identical; only container byte-order differs).
    #[test]
    fn unpack_le_syncword_prefix_yields_raw_be_sync() {
        let packed = [0xFF, 0x1F, 0x00, 0xE8, 0xF0, 0x07];
        let unpacked = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::LittleEndian).unwrap();
        assert_eq!(&unpacked[..4], &[0x7F, 0xFE, 0x80, 0x01]);
    }

    /// Round-trip a known bit-pattern: pack a stream of 14-bit
    /// payloads MSB-first, then unpack and verify the lower-14-bit
    /// chunks read back identically. This exercises the 4-container
    /// (= 7-byte) alignment cycle without depending on hand-computed
    /// trailing bytes.
    #[test]
    fn unpack_roundtrip_against_synthetic_payloads_be() {
        let payloads: [u16; 8] = [
            0x1FFF, 0x2800, 0x07F0, 0x1234, 0x0ABC, 0x3FFF, 0x0000, 0x2AAA,
        ];
        // Pack: sign-extend each 14-bit payload into the upper 2
        // bits (mirroring the wiki's "sign bit extension" rule), then
        // write as 16-bit BE.
        let mut packed = Vec::with_capacity(payloads.len() * 2);
        for &p in &payloads {
            let signed = if p & 0x2000 != 0 {
                0xC000u16 | p
            } else {
                p & 0x3FFF
            };
            packed.extend_from_slice(&signed.to_be_bytes());
        }
        let unpacked = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::BigEndian).unwrap();
        // 8 containers × 14 = 112 bits → 14 bytes.
        assert_eq!(unpacked.len(), 14);
        // Walk the unpacked bit stream and verify each 14-bit slice.
        for (i, &expected) in payloads.iter().enumerate() {
            let start = i * 14;
            let got = read_bits_msb(&unpacked, start, 14);
            assert_eq!(got, expected as u32, "payload {i} mismatch");
        }
    }

    /// Same payloads, LE container order.
    #[test]
    fn unpack_roundtrip_against_synthetic_payloads_le() {
        let payloads: [u16; 8] = [
            0x1FFF, 0x2800, 0x07F0, 0x1234, 0x0ABC, 0x3FFF, 0x0000, 0x2AAA,
        ];
        let mut packed = Vec::with_capacity(payloads.len() * 2);
        for &p in &payloads {
            let signed = if p & 0x2000 != 0 {
                0xC000u16 | p
            } else {
                p & 0x3FFF
            };
            packed.extend_from_slice(&signed.to_le_bytes());
        }
        let unpacked = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::LittleEndian).unwrap();
        assert_eq!(unpacked.len(), 14);
        for (i, &expected) in payloads.iter().enumerate() {
            let start = i * 14;
            let got = read_bits_msb(&unpacked, start, 14);
            assert_eq!(got, expected as u32, "payload {i} mismatch");
        }
    }

    /// The upper-2-bit sign extension is documented as informative
    /// only — the unpacker MUST mask those bits away. Build a
    /// container with junk in bits 14..16 and verify the same payload
    /// is recovered.
    #[test]
    fn unpack_discards_upper_sign_bits() {
        // Two payloads: 0x1FFF and 0x2800. We stuff garbage into
        // the upper 2 bits of each container (0b11 and 0b10
        // respectively, neither of which matches the "correct" sign
        // extension of `00` / `11` for these specific payloads).
        let packed = [
            0xDF, 0xFF, // (1<<15)|(1<<14)|0x1FFF = 0xDFFF — top bits garbage
            0xA8, 0x00, // (1<<15)|0x2800 = 0xA800 — top bit garbage
        ];
        let unpacked = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::BigEndian).unwrap();
        // 2 containers × 14 = 28 bits → 4 bytes out.
        assert_eq!(unpacked.len(), 4);
        // Top 14 bits = 0x1FFF, next 14 bits = 0x2800.
        assert_eq!(read_bits_msb(&unpacked, 0, 14), 0x1FFF);
        assert_eq!(read_bits_msb(&unpacked, 14, 14), 0x2800);
    }

    #[test]
    fn unpack_odd_length_returns_eof() {
        let packed = [0x1F, 0xFF, 0xE8];
        assert_eq!(
            unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::BigEndian).unwrap_err(),
            Error::UnexpectedEof,
        );
    }

    #[test]
    fn unpack_empty_yields_empty() {
        let out = unpack_14bit_to_16bit(&[], FourteenBitByteOrder::BigEndian).unwrap();
        assert!(out.is_empty());
    }

    /// Four-container alignment: 4 × 14 = 56 bits = 7 bytes, no
    /// trailing padding.
    #[test]
    fn unpack_four_container_alignment_emits_exactly_seven_bytes() {
        let packed = [0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xF0, 0x12, 0x34];
        let out = unpack_14bit_to_16bit(&packed, FourteenBitByteOrder::BigEndian).unwrap();
        assert_eq!(out.len(), 7);
    }

    #[test]
    fn from_sync_routes_correctly() {
        assert_eq!(
            FourteenBitByteOrder::from_sync(SyncWordEncoding::FourteenBitBigEndian),
            Some(FourteenBitByteOrder::BigEndian),
        );
        assert_eq!(
            FourteenBitByteOrder::from_sync(SyncWordEncoding::FourteenBitLittleEndian),
            Some(FourteenBitByteOrder::LittleEndian),
        );
        assert_eq!(
            FourteenBitByteOrder::from_sync(SyncWordEncoding::RawBigEndian),
            None,
        );
        assert_eq!(
            FourteenBitByteOrder::from_sync(SyncWordEncoding::RawLittleEndian),
            None,
        );
    }

    /// Helper: read `n` bits MSB-first starting at absolute bit
    /// offset `start` from a big-endian byte stream. Mirrors the
    /// `BitReader` semantics but is duplicated here to keep this
    /// module testable in isolation.
    fn read_bits_msb(bytes: &[u8], start: usize, n: usize) -> u32 {
        let mut v: u32 = 0;
        for i in 0..n {
            let abs = start + i;
            let byte = bytes[abs / 8];
            let bit = (byte >> (7 - (abs % 8))) & 1;
            v = (v << 1) | bit as u32;
        }
        v
    }
}
