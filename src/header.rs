//! DTS Coherent Acoustics frame-sync header parser.
//!
//! All field layouts and value-range bounds in this module come
//! verbatim from the mirrored multimedia.cx snapshot at
//! `docs/audio/dts/wiki/DTS.wiki`, which in turn mirrors the ETSI
//! TS 102 114 §5.3 frame-header description. The wiki notes four
//! sync encodings:
//!
//! ```text
//!   7F FE 80 01           — raw big-endian
//!   FE 7F 01 80           — raw little-endian (byte-swapped)
//!   1F FF E8 00 07 Fx     — 14-bit packed big-endian
//!   FF 1F 00 E8 Fx 07     — 14-bit packed little-endian
//! ```
//!
//! Round 1 fully parses the two 16-bit raw variants and returns
//! [`Error::UnsupportedFourteenBit`] for the 14-bit variants — the
//! 14→16 bit unpacking step is a follow-up task.
//!
//! ## Field layout (after the 32-bit sync, MSB-first)
//!
//! | Bits | Name                  | Notes                              |
//! | ---- | --------------------- | ---------------------------------- |
//! | 1    | FTYPE                 | 0 = termination, 1 = normal        |
//! | 5    | SHORT (sample count)  | raw value; samples-in-block = +1   |
//! | 1    | CRC_PRESENT           |                                    |
//! | 7    | NBLKS (block count)   | raw 5..=127                        |
//! | 14   | FSIZE-1               | frame size in bytes = +1, 95..=16384 |
//! | 6    | AMODE (channel cfg)   | 0..=15 standard, 16..=63 user      |
//! | 4    | SFREQ                 | sample-freq index (tables missing) |
//! | 5    | RATE                  | bitrate index (tables missing)     |
//!
//! Round 1 stops after RATE; the remaining bits (downmix, dynrng,
//! timstp, auxdata, HDCD, ext-audio-descr, ext-audio, ASPF, LFE,
//! predictor-history, header CRC) are documented in the wiki but
//! not surfaced via the typed header until a future round needs
//! them.

use crate::bitreader::BitReader;
use crate::{Error, Result};

/// The four documented DTS Core syncword encodings (per the wiki
/// snapshot's "How to distinguish different versions" table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SyncWordEncoding {
    /// `7F FE 80 01` — native big-endian raw 16-bit-per-word DTS.
    /// The wiki notes this is the **native** DTS byte order.
    RawBigEndian,
    /// `FE 7F 01 80` — byte-swapped little-endian raw 16-bit-per-word
    /// DTS. Commonly seen inside DTS-in-WAV / CD-DA encapsulation.
    RawLittleEndian,
    /// `1F FF E8 00 07 Fx` — 14-bit big-endian packed DTS. Round 1
    /// does not unpack this variant.
    FourteenBitBigEndian,
    /// `FF 1F 00 E8 Fx 07` — 14-bit little-endian packed DTS. Round
    /// 1 does not unpack this variant.
    FourteenBitLittleEndian,
}

/// Frame-type flag (FTYPE bit, 1 bit wide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// `FTYPE == 0` — termination frame. Per the wiki this marks the
    /// last frame in a continuous stream.
    Termination,
    /// `FTYPE == 1` — normal frame.
    Normal,
}

/// Parsed DTS Core frame-sync header.
///
/// Round 1 surfaces only the structural fields whose semantics are
/// unambiguous in the wiki snapshot. The sample-rate / bitrate /
/// channel-count *value* tables are not in `docs/` yet — see
/// [`Self::sample_rate_hz`], [`Self::bit_rate_bps`], and
/// [`Self::channel_count`] for the `Option` semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DtsFrameHeader {
    /// Which of the four documented sync encodings was found at
    /// offset zero.
    pub sync_word_encoding: SyncWordEncoding,
    /// Decoded frame type (termination vs normal).
    pub frame_type: FrameType,
    /// Samples per sub-block (the wiki's "Deficit sample count + 1",
    /// nominally 32 for a normal frame).
    pub sample_count_per_block: u8,
    /// Whether the header CRC field is present (the 16-bit field
    /// that follows the predictor-history bit). Round 1 does not
    /// verify it; the flag is exposed so a future round can.
    pub crc_present: bool,
    /// Number of sub-blocks in the frame (raw NBLKS, 5..=127).
    pub blocks_per_frame: u8,
    /// Frame size in bytes (`FSIZE-1 + 1`, 95..=16384).
    pub frame_size_bytes: u16,
    /// Channel-configuration code (AMODE, 0..=63). 0..=15 are
    /// standard layouts; 16..=63 are user-defined per the wiki.
    pub amode: u8,
    /// Sample-frequency index (SFREQ, 0..=15). The Hz mapping is
    /// not yet in `docs/`; see [`Self::sample_rate_hz`].
    pub sfreq_index: u8,
    /// Transmission-bitrate index (RATE, 0..=31). The bps mapping
    /// is not yet in `docs/`; see [`Self::bit_rate_bps`].
    pub rate_index: u8,
}

impl DtsFrameHeader {
    /// Resolve [`Self::sfreq_index`] to a sample-rate in Hertz.
    ///
    /// Returns `None` for now: the index→Hz table is missing from
    /// `docs/audio/dts/`. The wiki snapshot says "See table below"
    /// but the table itself was not mirrored. Once a clean-room
    /// source for the table lands the resolver will be filled in.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        let _ = self.sfreq_index;
        None
    }

    /// Resolve [`Self::rate_index`] to a transmission bit-rate in
    /// bits per second.
    ///
    /// Returns `None` for now: the index→bps table is missing from
    /// `docs/audio/dts/`. The wiki snapshot says "See table below"
    /// but the table itself was not mirrored.
    pub fn bit_rate_bps(&self) -> Option<u32> {
        let _ = self.rate_index;
        None
    }

    /// Resolve [`Self::amode`] to a count of audio channels (LFE
    /// excluded; the LFE field lives later in the header and is
    /// not surfaced in round 1).
    ///
    /// Returns `None` for now: the AMODE→channel-layout table is
    /// missing from `docs/audio/dts/`. The wiki snapshot only says
    /// "0..=15 standard, 16..=63 user-defined" without spelling
    /// out the layouts.
    pub fn channel_count(&self) -> Option<u8> {
        let _ = self.amode;
        None
    }
}

/// Parse a single DTS Core frame-sync header from the start of
/// `bytes`.
///
/// The buffer must contain at least the 32-bit sync (or 40 bits for
/// a 14-bit sync) plus the ~10 bytes of header that follow. Returns
/// [`Error::UnexpectedEof`] on a short buffer, [`Error::NoSync`] if
/// no documented sync sequence matches at offset zero, and
/// [`Error::UnsupportedFourteenBit`] if a 14-bit sync is found
/// (round-1 limitation).
///
/// The parser is non-allocating and side-effect free.
pub fn parse_frame_header(bytes: &[u8]) -> Result<DtsFrameHeader> {
    let sync = detect_sync(bytes)?;
    match sync {
        SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian => {
            return Err(Error::UnsupportedFourteenBit);
        }
        _ => {}
    }

    // Normalise the buffer so that we always read the header from
    // a slice whose first 4 bytes are the big-endian sync. For
    // RawLittleEndian we byte-swap each 16-bit word in a small
    // scratch buffer; only the first ~16 bytes are needed.
    let normalised: Vec<u8>;
    let header_bytes: &[u8] = match sync {
        SyncWordEncoding::RawBigEndian => bytes,
        SyncWordEncoding::RawLittleEndian => {
            // We need 4 sync bytes + ceil(82 / 8) = 11 header bytes.
            // Round up to 16 (eight 16-bit words) so any 16-bit
            // word straddle stays inside the slice.
            let needed = 16;
            if bytes.len() < needed {
                return Err(Error::UnexpectedEof);
            }
            let mut scratch = Vec::with_capacity(needed);
            for chunk in bytes[..needed].chunks_exact(2) {
                scratch.push(chunk[1]);
                scratch.push(chunk[0]);
            }
            normalised = scratch;
            &normalised
        }
        // unreachable: 14-bit branches returned above.
        SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian => {
            unreachable!()
        }
    };

    // Need at least 4 sync + 11 header bytes = 15 bytes to read the
    // 82 header bits this round consumes. We accept 15.
    if header_bytes.len() < 15 {
        return Err(Error::UnexpectedEof);
    }

    let mut br = BitReader::from_byte_offset(header_bytes, 4);

    let ftype_raw = br.read_bit()?;
    let frame_type = if ftype_raw {
        FrameType::Normal
    } else {
        FrameType::Termination
    };
    let sample_count_minus_one = br.read_bits(5)? as u8;
    let sample_count_per_block = sample_count_minus_one + 1;
    let crc_present = br.read_bit()?;
    let nblks = br.read_bits(7)? as u8;
    if nblks < 5 {
        return Err(Error::BlockCountOutOfRange { blocks: nblks });
    }
    let fsize_minus_one = br.read_bits(14)? as u16;
    let frame_size_bytes = fsize_minus_one + 1;
    if frame_size_bytes < 95 {
        return Err(Error::FrameSizeOutOfRange {
            frame_size: frame_size_bytes,
        });
    }
    let amode = br.read_bits(6)? as u8;
    let sfreq_index = br.read_bits(4)? as u8;
    let rate_index = br.read_bits(5)? as u8;

    Ok(DtsFrameHeader {
        sync_word_encoding: sync,
        frame_type,
        sample_count_per_block,
        crc_present,
        blocks_per_frame: nblks,
        frame_size_bytes,
        amode,
        sfreq_index,
        rate_index,
    })
}

/// Detect which of the four documented sync sequences (if any)
/// appears at the start of `bytes`. Public to the crate so tests can
/// exercise sync detection independently of header decoding.
pub(crate) fn detect_sync(bytes: &[u8]) -> Result<SyncWordEncoding> {
    if bytes.len() < 4 {
        return Err(Error::UnexpectedEof);
    }
    // Raw 16-bit sequences (4 bytes).
    if bytes[..4] == [0x7F, 0xFE, 0x80, 0x01] {
        return Ok(SyncWordEncoding::RawBigEndian);
    }
    if bytes[..4] == [0xFE, 0x7F, 0x01, 0x80] {
        return Ok(SyncWordEncoding::RawLittleEndian);
    }
    // 14-bit sequences (5 bytes, with low nibble of byte 5 wildcarded
    // per the wiki: `1F FF E8 00 07 Fx` and `FF 1F 00 E8 Fx 07`).
    if bytes.len() >= 6 && bytes[..5] == [0x1F, 0xFF, 0xE8, 0x00, 0x07] && (bytes[5] & 0xF0) == 0xF0
    {
        return Ok(SyncWordEncoding::FourteenBitBigEndian);
    }
    if bytes.len() >= 6
        && bytes[..4] == [0xFF, 0x1F, 0x00, 0xE8]
        && (bytes[4] & 0xF0) == 0xF0
        && bytes[5] == 0x07
    {
        return Ok(SyncWordEncoding::FourteenBitLittleEndian);
    }
    Err(Error::NoSync)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic raw-BE DTS frame header with explicit field
    /// values, in the bit order documented above.
    ///
    /// `extra_bits_after_rate` are the 12 trailing header bits the
    /// parser does not consume in round 1 (downmix..predictor) plus
    /// any padding; passed as a `u32` (only the bottom 12 bits used)
    /// to keep the test inputs explicit.
    #[allow(clippy::too_many_arguments)]
    fn build_be_header(
        ftype: u32,
        sample_count_m1: u32, // 5 bits
        crc_present: u32,     // 1 bit
        nblks: u32,           // 7 bits
        fsize_m1: u32,        // 14 bits
        amode: u32,           // 6 bits
        sfreq: u32,           // 4 bits
        rate: u32,            // 5 bits
        extra_bits: u32,      // 12 bits (downmix..predictor)
    ) -> Vec<u8> {
        // We will accumulate a bit-vector MSB-first and then chunk to
        // bytes.
        let mut bv: Vec<bool> = Vec::new();

        fn push(bv: &mut Vec<bool>, value: u32, width: u32) {
            for i in (0..width).rev() {
                bv.push(((value >> i) & 1) == 1);
            }
        }

        // 32-bit sync = 0x7FFE8001
        push(&mut bv, 0x7FFE_8001, 32);
        push(&mut bv, ftype, 1);
        push(&mut bv, sample_count_m1, 5);
        push(&mut bv, crc_present, 1);
        push(&mut bv, nblks, 7);
        push(&mut bv, fsize_m1, 14);
        push(&mut bv, amode, 6);
        push(&mut bv, sfreq, 4);
        push(&mut bv, rate, 5);
        push(&mut bv, extra_bits, 12);
        // pad to whole bytes
        while bv.len() % 8 != 0 {
            bv.push(false);
        }
        // pad to 16 bytes so the LE byte-swap path always has 16
        // bytes too if a caller chooses to reuse this builder.
        let mut bytes = Vec::with_capacity(bv.len() / 8);
        for chunk in bv.chunks(8) {
            let mut b: u8 = 0;
            for (i, bit) in chunk.iter().enumerate() {
                if *bit {
                    b |= 1 << (7 - i);
                }
            }
            bytes.push(b);
        }
        while bytes.len() < 16 {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn detect_raw_be_sync() {
        let mut buf = vec![0; 16];
        buf[0] = 0x7F;
        buf[1] = 0xFE;
        buf[2] = 0x80;
        buf[3] = 0x01;
        assert_eq!(detect_sync(&buf).unwrap(), SyncWordEncoding::RawBigEndian);
    }

    #[test]
    fn detect_raw_le_sync() {
        let buf = [0xFE, 0x7F, 0x01, 0x80];
        assert_eq!(
            detect_sync(&buf).unwrap(),
            SyncWordEncoding::RawLittleEndian
        );
    }

    #[test]
    fn detect_14bit_be_sync() {
        let buf = [0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xFA];
        assert_eq!(
            detect_sync(&buf).unwrap(),
            SyncWordEncoding::FourteenBitBigEndian
        );
    }

    #[test]
    fn detect_14bit_le_sync() {
        let buf = [0xFF, 0x1F, 0x00, 0xE8, 0xF3, 0x07];
        assert_eq!(
            detect_sync(&buf).unwrap(),
            SyncWordEncoding::FourteenBitLittleEndian
        );
    }

    #[test]
    fn detect_no_sync_returns_error() {
        let buf = [0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(detect_sync(&buf).unwrap_err(), Error::NoSync);
    }

    #[test]
    fn detect_short_buffer_returns_eof() {
        assert_eq!(detect_sync(&[0x7F]).unwrap_err(), Error::UnexpectedEof);
    }

    #[test]
    fn parse_normal_frame_be_typical() {
        // Typical values seen on a 48 kHz 1509 kbps 5.1 frame
        // (per the wiki's general bit-layout description; we do
        // not yet know the actual SFREQ/RATE/AMODE *codes* for
        // those Hz/bps/channels — pick arbitrary codes since the
        // parser only roundtrips the raw indices).
        let bytes = build_be_header(
            1,                // FTYPE = normal
            31,               // sample_count_m1 = 31 → 32 samples/block
            1,                // CRC present
            15,               // NBLKS = 15  (16 blocks)
            1023,             // FSIZE-1 = 1023 → frame size = 1024 bytes
            9,                // AMODE = 9 (raw index)
            13,               // SFREQ = 13
            25,               // RATE = 25
            0b1010_0101_0011, // extra trailing 12 bits
        );

        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.sync_word_encoding, SyncWordEncoding::RawBigEndian);
        assert_eq!(hdr.frame_type, FrameType::Normal);
        assert_eq!(hdr.sample_count_per_block, 32);
        assert!(hdr.crc_present);
        assert_eq!(hdr.blocks_per_frame, 15);
        assert_eq!(hdr.frame_size_bytes, 1024);
        assert_eq!(hdr.amode, 9);
        assert_eq!(hdr.sfreq_index, 13);
        assert_eq!(hdr.rate_index, 25);
    }

    #[test]
    fn parse_termination_frame_be() {
        let bytes = build_be_header(0, 0, 0, 5, 94, 0, 0, 0, 0);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.frame_type, FrameType::Termination);
        assert_eq!(hdr.sample_count_per_block, 1);
        assert!(!hdr.crc_present);
        assert_eq!(hdr.blocks_per_frame, 5);
        assert_eq!(hdr.frame_size_bytes, 95);
    }

    #[test]
    fn parse_rejects_nblks_below_5() {
        let bytes = build_be_header(1, 31, 1, 4, 1023, 0, 0, 0, 0);
        assert_eq!(
            parse_frame_header(&bytes).unwrap_err(),
            Error::BlockCountOutOfRange { blocks: 4 }
        );
    }

    #[test]
    fn parse_rejects_frame_size_below_95() {
        let bytes = build_be_header(1, 31, 1, 16, 93, 0, 0, 0, 0);
        assert_eq!(
            parse_frame_header(&bytes).unwrap_err(),
            Error::FrameSizeOutOfRange { frame_size: 94 }
        );
    }

    #[test]
    fn parse_accepts_largest_documented_values() {
        // NBLKS = 127, FSIZE-1 = 16383 → 16384 bytes, AMODE = 63,
        // SFREQ = 15, RATE = 31 — all the max-index values the
        // wiki allows for these fields.
        let bytes = build_be_header(1, 31, 1, 127, 16383, 63, 15, 31, 0);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.blocks_per_frame, 127);
        assert_eq!(hdr.frame_size_bytes, 16384);
        assert_eq!(hdr.amode, 63);
        assert_eq!(hdr.sfreq_index, 15);
        assert_eq!(hdr.rate_index, 31);
    }

    #[test]
    fn parse_short_buffer_returns_eof() {
        let mut bytes = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0);
        bytes.truncate(8);
        assert_eq!(
            parse_frame_header(&bytes).unwrap_err(),
            Error::UnexpectedEof
        );
    }

    #[test]
    fn parse_le_byteswapped_matches_be() {
        // Build BE bytes then byte-swap each 16-bit word; the
        // parsed structural fields must match the BE version
        // exactly (only the sync_word_encoding differs).
        let be = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0);
        let mut le = Vec::with_capacity(be.len());
        for chunk in be.chunks_exact(2) {
            le.push(chunk[1]);
            le.push(chunk[0]);
        }
        // Sanity-check the sync was swapped to the LE variant.
        assert_eq!(&le[..4], &[0xFE, 0x7F, 0x01, 0x80]);
        let hdr_be = parse_frame_header(&be).unwrap();
        let hdr_le = parse_frame_header(&le).unwrap();
        assert_eq!(hdr_le.sync_word_encoding, SyncWordEncoding::RawLittleEndian);
        assert_eq!(hdr_le.frame_type, hdr_be.frame_type);
        assert_eq!(hdr_le.sample_count_per_block, hdr_be.sample_count_per_block);
        assert_eq!(hdr_le.crc_present, hdr_be.crc_present);
        assert_eq!(hdr_le.blocks_per_frame, hdr_be.blocks_per_frame);
        assert_eq!(hdr_le.frame_size_bytes, hdr_be.frame_size_bytes);
        assert_eq!(hdr_le.amode, hdr_be.amode);
        assert_eq!(hdr_le.sfreq_index, hdr_be.sfreq_index);
        assert_eq!(hdr_le.rate_index, hdr_be.rate_index);
    }

    #[test]
    fn parse_14bit_be_returns_unsupported() {
        let mut buf = vec![0; 16];
        buf[..6].copy_from_slice(&[0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xFA]);
        assert_eq!(
            parse_frame_header(&buf).unwrap_err(),
            Error::UnsupportedFourteenBit
        );
    }

    #[test]
    fn parse_14bit_le_returns_unsupported() {
        let mut buf = vec![0; 16];
        buf[..6].copy_from_slice(&[0xFF, 0x1F, 0x00, 0xE8, 0xF3, 0x07]);
        assert_eq!(
            parse_frame_header(&buf).unwrap_err(),
            Error::UnsupportedFourteenBit
        );
    }

    #[test]
    fn parse_no_sync_returns_no_sync() {
        let buf = [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_frame_header(&buf).unwrap_err(), Error::NoSync);
    }

    #[test]
    fn value_resolvers_return_none_until_tables_land() {
        let bytes = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.sample_rate_hz(), None);
        assert_eq!(hdr.bit_rate_bps(), None);
        assert_eq!(hdr.channel_count(), None);
    }
}
