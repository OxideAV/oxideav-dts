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
//! [`Error::UnsupportedFourteenBit`] for the 14-bit variants from
//! [`parse_frame_header`]. Round 2 adds [`parse_frame_header_14bit`]
//! plus a [`crate::unpack_14bit_to_16bit`] primitive that converts a
//! 14-bit-packed buffer into its 16-bit-equivalent raw-BE form so the
//! existing parser can consume both encodings uniformly.
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
//! | 1    | DOWNMIX               | embedded downmix-coefficients flag |
//! | 1    | DYNRANGE              | embedded dynamic-range data flag   |
//! | 1    | TIMSTP                | timestamp-field-present flag       |
//! | 1    | AUXDATA               | auxiliary-data-field-present flag  |
//! | 1    | HDCD                  | HDCD-encoded-source flag           |
//! | 3    | EXT_DESCR             | extension-audio-descriptor (0..=7) |
//! | 1    | EXT_CODING            | extension-audio-coding flag        |
//! | 1    | ASPF                  | audio-sync-word in subframes flag  |
//! | 2    | LFE                   | LFE channel mode (0..=3)           |
//! | 1    | PRED_HISTORY          | predictor-history-enabled flag     |
//! | 16   | HEADER_CRC            | only present when CRC_PRESENT == 1 |
//!
//! Round 3 (2026-05-21) surfaces every field in the table above
//! through [`DtsFrameHeader`]; the remaining wiki entries
//! (multirate-inter, version, copy-history, source-PCM-resolution,
//! front-sum, surround-sum, dialog-normalization) are deferred
//! to a future round.

use crate::bitreader::BitReader;
use crate::unpack14::{unpack_14bit_to_16bit, FourteenBitByteOrder};
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
    /// `1F FF E8 00 07 Fx` — 14-bit big-endian packed DTS. The
    /// `unpack14` module (round 2) converts this into the raw-BE
    /// form for [`parse_frame_header_14bit`].
    FourteenBitBigEndian,
    /// `FF 1F 00 E8 Fx 07` — 14-bit little-endian packed DTS. The
    /// `unpack14` module (round 2) converts this into the raw-BE
    /// form for [`parse_frame_header_14bit`].
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

/// LFE-channel mode (`LFE`, 2 bits wide).
///
/// The wiki snapshot lists the field as a 2-bit code without naming
/// the four values. ETSI TS 102 114 §5.3.1 documents the codes as
/// "no LFE channel" (0), "128-sample-decimated LFE" (1),
/// "64-sample-decimated LFE" (2), and "reserved/invalid" (3); the
/// wiki snapshot itself does not include those labels, so this enum
/// keeps the names neutral — `code` is the raw 2-bit value and
/// [`Self::is_present`] discriminates "no LFE" (code 0) from the
/// three present-LFE codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LfeMode {
    /// Raw LFE code 0. The wiki implies this is "no LFE channel"
    /// because the LFE field is the gate to the LFE-stream
    /// subblocks; this implementation does not assert it.
    None,
    /// Raw LFE code 1 — present, mode-1 (see `docs/audio/dts/wiki/`).
    Mode1,
    /// Raw LFE code 2 — present, mode-2.
    Mode2,
    /// Raw LFE code 3 — reserved-or-mode-3 per the wiki snapshot.
    Mode3,
}

impl LfeMode {
    /// Construct from the raw 2-bit code (`0..=3`).
    fn from_raw(code: u8) -> Self {
        match code & 0b11 {
            0 => LfeMode::None,
            1 => LfeMode::Mode1,
            2 => LfeMode::Mode2,
            _ => LfeMode::Mode3,
        }
    }

    /// Recover the raw 2-bit LFE code.
    pub fn code(self) -> u8 {
        match self {
            LfeMode::None => 0,
            LfeMode::Mode1 => 1,
            LfeMode::Mode2 => 2,
            LfeMode::Mode3 => 3,
        }
    }

    /// Whether *any* LFE channel is present. Codes 1..=3 all signal a
    /// present LFE channel per the wiki; only code 0 marks its
    /// absence.
    pub fn is_present(self) -> bool {
        !matches!(self, LfeMode::None)
    }
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
    /// Embedded-downmix-coefficients flag (`DOWNMIX`, 1 bit).
    pub downmix: bool,
    /// Embedded-dynamic-range-data flag (`DYNRANGE`, 1 bit).
    pub dynamic_range: bool,
    /// Timestamp-field-present flag (`TIMSTP`, 1 bit). The wiki
    /// snapshot only names the bit; round 3 does not interpret the
    /// optional timestamp payload that may appear later in the
    /// bitstream.
    pub time_stamp: bool,
    /// Auxiliary-data-field-present flag (`AUXDATA`, 1 bit).
    pub aux_data: bool,
    /// HDCD-encoded-source flag (`HDCD`, 1 bit).
    pub hdcd: bool,
    /// Extension-audio-descriptor (`EXT_DESCR`, 3 bits, 0..=7). The
    /// wiki snapshot does not enumerate the value semantics; the raw
    /// 3-bit code is preserved verbatim.
    pub ext_descr: u8,
    /// Extension-audio-coding flag (`EXT_CODING`, 1 bit). Indicates
    /// whether an extension substream (X96 / XCH / XXCH / EXSS) is
    /// muxed alongside the Core stream.
    pub ext_coding: bool,
    /// Audio-sync-word-in-subframes flag (`ASPF`, 1 bit).
    pub aspf: bool,
    /// LFE-channel mode (`LFE`, 2 bits). See [`LfeMode`].
    pub lfe: LfeMode,
    /// Predictor-history-enabled flag (`PRED_HISTORY`, 1 bit).
    pub predictor_history: bool,
    /// 16-bit header-CRC value (`HEADER_CRC`). Present iff
    /// [`Self::crc_present`] is `true`; `None` otherwise. The CRC
    /// polynomial is **not** documented in the wiki snapshot under
    /// `docs/audio/dts/`, so [`Self::verify_header_crc`] currently
    /// returns `None` — the field is exposed for round-3 callers
    /// that want to forward the raw value, but verification waits
    /// for the polynomial to land in `docs/`.
    pub header_crc: Option<u16>,
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
    /// excluded; round 3 surfaces the LFE field separately via
    /// [`Self::lfe`] / [`LfeMode::is_present`]).
    ///
    /// Returns `None` for now: the AMODE→channel-layout table is
    /// missing from `docs/audio/dts/`. The wiki snapshot only says
    /// "0..=15 standard, 16..=63 user-defined" without spelling
    /// out the layouts.
    pub fn channel_count(&self) -> Option<u8> {
        let _ = self.amode;
        None
    }

    /// Verify the 16-bit [`Self::header_crc`] against the bits
    /// covered by the DTS Core header-CRC contract.
    ///
    /// Returns:
    /// - `None` if [`Self::crc_present`] is `false` (no CRC field
    ///   was emitted), or if the CRC polynomial is not yet
    ///   documented in `docs/audio/dts/`. As of round 3 the wiki
    ///   snapshot (`docs/audio/dts/wiki/DTS.wiki`) only names the
    ///   field (`16 bits | Header CRC | if CRC present above is
    ///   set`) without spelling out the polynomial, the seed
    ///   value, the byte order, or the bit range the CRC covers.
    /// - `Some(true)` / `Some(false)` if a future round lands the
    ///   polynomial specification.
    ///
    /// The caller can use [`Self::header_crc`] directly for
    /// pass-through scenarios that do not need verification (e.g.
    /// re-muxing).
    pub fn verify_header_crc(&self) -> Option<bool> {
        // Polynomial undocumented; see the comment above.
        let _ = self.header_crc?;
        None
    }
}

/// Parse a single DTS Core frame-sync header from the start of
/// `bytes`.
///
/// The buffer must begin with one of the two **raw 16-bit** sync
/// sequences (`7F FE 80 01` or its byte-swapped form
/// `FE 7F 01 80`) and contain ~15 bytes total (4-byte sync + 82
/// header bits). Returns:
/// - [`Error::UnexpectedEof`] on a short buffer.
/// - [`Error::NoSync`] if no documented sync sequence matches at
///   offset zero.
/// - [`Error::UnsupportedFourteenBit`] if a 14-bit-packed sync is
///   found at offset zero — callers with 14-bit input should use
///   [`parse_frame_header_14bit`] (or pre-unpack with
///   [`crate::unpack_14bit_to_16bit`]) instead.
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

    // Round 3: 13 bits of trailing single-bit / small-field flags.
    // Per the wiki snapshot, in this order:
    //   1 DOWNMIX, 1 DYNRANGE, 1 TIMSTP, 1 AUXDATA, 1 HDCD,
    //   3 EXT_DESCR, 1 EXT_CODING, 1 ASPF, 2 LFE, 1 PRED_HISTORY.
    let downmix = br.read_bit()?;
    let dynamic_range = br.read_bit()?;
    let time_stamp = br.read_bit()?;
    let aux_data = br.read_bit()?;
    let hdcd = br.read_bit()?;
    let ext_descr = br.read_bits(3)? as u8;
    let ext_coding = br.read_bit()?;
    let aspf = br.read_bit()?;
    let lfe_raw = br.read_bits(2)? as u8;
    let lfe = LfeMode::from_raw(lfe_raw);
    let predictor_history = br.read_bit()?;

    // Round 3: optional 16-bit HEADER_CRC field — present iff
    // CRC_PRESENT was set above.
    let header_crc = if crc_present {
        Some(br.read_bits(16)? as u16)
    } else {
        None
    };

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
        downmix,
        dynamic_range,
        time_stamp,
        aux_data,
        hdcd,
        ext_descr,
        ext_coding,
        aspf,
        lfe,
        predictor_history,
        header_crc,
    })
}

/// Parse a single DTS Core frame-sync header from a 14-bit-packed
/// buffer.
///
/// The buffer must start with one of the two 14-bit sync sequences
/// documented in `docs/audio/dts/wiki/DTS.wiki`
/// (`1F FF E8 00 07 Fx` for big-endian containers,
/// `FF 1F 00 E8 Fx 07` for little-endian containers). The function
/// runs [`crate::unpack_14bit_to_16bit`] to convert the input into
/// the raw-BE 16-bit form and then delegates to
/// [`parse_frame_header`].
///
/// Returns:
/// - [`Error::NoSync`] if the buffer does not start with a 14-bit
///   sync (callers should route raw 16-bit inputs to
///   [`parse_frame_header`] instead).
/// - [`Error::UnexpectedEof`] if the buffer has an odd length, or
///   if the unpacked stream is shorter than the 15 bytes the
///   header parser requires.
/// - the same out-of-range / EOF errors as [`parse_frame_header`]
///   once the unpack succeeds.
///
/// The unpacker output is byte-aligned every four containers
/// (4 × 14 = 56 bits); the header parser walks at most
/// sync + 56 header bits + 16 CRC bits = 104 bits → 13 bytes for
/// raw-BE input. The 14-bit-packed input therefore needs at least
/// `ceil(104 / 14) * 2 = 16` bytes (= eight 14-bit containers =
/// 112 bits ≥ 104). We require 18 bytes to keep a small margin and
/// to ensure the unpacked stream meets the 15-byte minimum the
/// raw-BE parser asserts up-front.
pub fn parse_frame_header_14bit(bytes: &[u8]) -> Result<DtsFrameHeader> {
    let sync = detect_sync(bytes)?;
    let order = match FourteenBitByteOrder::from_sync(sync) {
        Some(o) => o,
        None => {
            // Caller supplied a raw 16-bit sync to the 14-bit entry
            // point. Report NoSync to keep the two entry points'
            // accepted-input sets disjoint and unambiguous.
            return Err(Error::NoSync);
        }
    };
    // Need at least 18 input bytes (= 9 containers = 126 payload
    // bits = 15.75 unpacked bytes, rounded up to 16) so the parser
    // can read its 15-byte header window.
    if bytes.len() < 18 {
        return Err(Error::UnexpectedEof);
    }
    let unpacked = unpack_14bit_to_16bit(bytes, order)?;
    if unpacked.len() < 15 {
        return Err(Error::UnexpectedEof);
    }
    // After unpacking, the stream is raw-BE; delegate to the
    // existing parser. We override the returned sync_word_encoding
    // so callers see the original 14-bit variant rather than the
    // synthesised RawBigEndian one.
    let mut hdr = parse_frame_header(&unpacked)?;
    hdr.sync_word_encoding = sync;
    Ok(hdr)
}

/// Detect which of the four documented sync sequences (if any)
/// appears at the start of `bytes`. Public to the crate so tests can
/// exercise sync detection independently of header decoding.
///
/// For the two raw (16-bit) variants this is a literal byte-pattern
/// match against the wiki's documented prefixes.
///
/// For the two 14-bit variants the detector matches on the **lower
/// 14 bits** of each of the first three 16-bit containers, ignoring
/// the upper 2 bits of each container. This mirrors the unpacker
/// semantics (`docs/audio/dts/wiki/DTS.wiki` says the upper 2 bits
/// are sign-extension, which is informative-only when interpreting
/// the bytes as audio samples). The wiki's literal documented
/// prefixes (`1F FF E8 00 07 Fx` BE and `FF 1F 00 E8 Fx 07` LE) are
/// one specific instantiation of those payloads; sign-extended
/// instantiations encoding the same payloads are also valid 14-bit
/// DTS sync.
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
    // 14-bit sequences (6 bytes = three 16-bit containers carrying
    // 42 payload bits). The DTS syncword is 32 payload bits
    // (0x7FFE8001); a 14-bit-packed stream encodes those 32 bits
    // across containers 0/1 in full (14 + 14 = 28 bits) and the top
    // 4 bits of container 2 (28..32). Container 2's bottom 10 bits
    // carry frame-header data (FTYPE..NBLKS_high) and must NOT
    // participate in sync detection — earlier round-1 code matched
    // them too, which incidentally only accepted frames whose
    // FTYPE/deficit/CRC/NBLKS_high happened to be `1/31/1/000`.
    //
    // We confirm bits 0..31 of the unpacked payload equal
    // 0x7FFE_8001 by:
    //   container 0 lower 14 bits == 0x1FFF (covers bits 0..13)
    //   container 1 lower 14 bits == 0x2800 (covers bits 14..27)
    //   container 2 lower 14 bits, top 4 == 0b0001 (covers bits 28..31)
    if bytes.len() >= 6 {
        let c0_be = u16::from_be_bytes([bytes[0], bytes[1]]) & 0x3FFF;
        let c1_be = u16::from_be_bytes([bytes[2], bytes[3]]) & 0x3FFF;
        let c2_be = u16::from_be_bytes([bytes[4], bytes[5]]) & 0x3FFF;
        // c2's top 4 bits within its 14-bit payload: shift right 10
        // and mask to 4 bits.
        if c0_be == 0x1FFF && c1_be == 0x2800 && ((c2_be >> 10) & 0xF) == 0x1 {
            return Ok(SyncWordEncoding::FourteenBitBigEndian);
        }
        let c0_le = u16::from_le_bytes([bytes[0], bytes[1]]) & 0x3FFF;
        let c1_le = u16::from_le_bytes([bytes[2], bytes[3]]) & 0x3FFF;
        let c2_le = u16::from_le_bytes([bytes[4], bytes[5]]) & 0x3FFF;
        if c0_le == 0x1FFF && c1_le == 0x2800 && ((c2_le >> 10) & 0xF) == 0x1 {
            return Ok(SyncWordEncoding::FourteenBitLittleEndian);
        }
    }
    Err(Error::NoSync)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic raw-BE DTS frame header with explicit field
    /// values, in the bit order documented above.
    ///
    /// `extra_bits` are the 13 trailing header bits the parser
    /// consumes after RATE in round 3 (downmix .. predictor history),
    /// passed as a `u32` (only the bottom 13 bits used) so callers
    /// can spell the bit-pattern out literally. If
    /// `header_crc` is `Some`, the 16-bit CRC is emitted after the
    /// 13 trailing bits and `crc_present` should be `1`.
    #[allow(clippy::too_many_arguments)]
    fn build_be_header(
        ftype: u32,
        sample_count_m1: u32,    // 5 bits
        crc_present: u32,        // 1 bit
        nblks: u32,              // 7 bits
        fsize_m1: u32,           // 14 bits
        amode: u32,              // 6 bits
        sfreq: u32,              // 4 bits
        rate: u32,               // 5 bits
        extra_bits: u32,         // 13 bits (downmix..predictor)
        header_crc: Option<u32>, // 16 bits, only when crc_present == 1
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
        push(&mut bv, extra_bits, 13);
        if let Some(crc) = header_crc {
            push(&mut bv, crc, 16);
        }
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
            1,                  // FTYPE = normal
            31,                 // sample_count_m1 = 31 → 32 samples/block
            1,                  // CRC present
            15,                 // NBLKS = 15  (16 blocks)
            1023,               // FSIZE-1 = 1023 → frame size = 1024 bytes
            9,                  // AMODE = 9 (raw index)
            13,                 // SFREQ = 13
            25,                 // RATE = 25
            0b1_0100_1010_0011, // extra trailing 13 bits
            Some(0xC0DE),       // CRC field present
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
        // Round 3: trailing-13-bit flags decoded MSB-first from
        // 0b1_0100_1010_0011 → downmix=1, dyn=0, time=1, aux=0,
        // hdcd=0, ext_descr=101=5, ext_coding=0, aspf=0, lfe=01,
        // predictor=1.
        assert!(hdr.downmix);
        assert!(!hdr.dynamic_range);
        assert!(hdr.time_stamp);
        assert!(!hdr.aux_data);
        assert!(!hdr.hdcd);
        assert_eq!(hdr.ext_descr, 0b101);
        assert!(!hdr.ext_coding);
        assert!(!hdr.aspf);
        assert_eq!(hdr.lfe, LfeMode::Mode1);
        assert!(hdr.predictor_history);
        // CRC field present.
        assert_eq!(hdr.header_crc, Some(0xC0DE));
    }

    #[test]
    fn parse_termination_frame_be() {
        let bytes = build_be_header(0, 0, 0, 5, 94, 0, 0, 0, 0, None);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.frame_type, FrameType::Termination);
        assert_eq!(hdr.sample_count_per_block, 1);
        assert!(!hdr.crc_present);
        assert_eq!(hdr.blocks_per_frame, 5);
        assert_eq!(hdr.frame_size_bytes, 95);
        // All trailing flags zero by construction.
        assert!(!hdr.downmix);
        assert!(!hdr.dynamic_range);
        assert!(!hdr.time_stamp);
        assert!(!hdr.aux_data);
        assert!(!hdr.hdcd);
        assert_eq!(hdr.ext_descr, 0);
        assert!(!hdr.ext_coding);
        assert!(!hdr.aspf);
        assert_eq!(hdr.lfe, LfeMode::None);
        assert!(!hdr.predictor_history);
        // crc_present == 0 means no CRC field follows.
        assert_eq!(hdr.header_crc, None);
    }

    #[test]
    fn parse_rejects_nblks_below_5() {
        let bytes = build_be_header(1, 31, 1, 4, 1023, 0, 0, 0, 0, Some(0));
        assert_eq!(
            parse_frame_header(&bytes).unwrap_err(),
            Error::BlockCountOutOfRange { blocks: 4 }
        );
    }

    #[test]
    fn parse_rejects_frame_size_below_95() {
        let bytes = build_be_header(1, 31, 1, 16, 93, 0, 0, 0, 0, Some(0));
        assert_eq!(
            parse_frame_header(&bytes).unwrap_err(),
            Error::FrameSizeOutOfRange { frame_size: 94 }
        );
    }

    #[test]
    fn parse_accepts_largest_documented_values() {
        // NBLKS = 127, FSIZE-1 = 16383 → 16384 bytes, AMODE = 63,
        // SFREQ = 15, RATE = 31 — all the max-index values the
        // wiki allows for these fields. Also exercises the
        // largest documented trailing-field codes: ext_descr=7,
        // lfe code 3 (Mode3), and all flag bits set.
        let bytes = build_be_header(
            1,
            31,
            1,
            127,
            16383,
            63,
            15,
            31,
            0b1_1111_1111_1111,
            Some(0xFFFF),
        );
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.blocks_per_frame, 127);
        assert_eq!(hdr.frame_size_bytes, 16384);
        assert_eq!(hdr.amode, 63);
        assert_eq!(hdr.sfreq_index, 15);
        assert_eq!(hdr.rate_index, 31);
        assert!(hdr.downmix);
        assert!(hdr.dynamic_range);
        assert!(hdr.time_stamp);
        assert!(hdr.aux_data);
        assert!(hdr.hdcd);
        assert_eq!(hdr.ext_descr, 0b111);
        assert!(hdr.ext_coding);
        assert!(hdr.aspf);
        assert_eq!(hdr.lfe, LfeMode::Mode3);
        assert!(hdr.predictor_history);
        assert_eq!(hdr.header_crc, Some(0xFFFF));
    }

    #[test]
    fn parse_short_buffer_returns_eof() {
        let mut bytes = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0, Some(0));
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
        let be = build_be_header(
            1,
            31,
            1,
            16,
            1023,
            9,
            13,
            25,
            0b1_0100_1010_0011,
            Some(0xBEEF),
        );
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
        // Round 3 fields must also round-trip identically through
        // the LE byte-swap path.
        assert_eq!(hdr_le.downmix, hdr_be.downmix);
        assert_eq!(hdr_le.dynamic_range, hdr_be.dynamic_range);
        assert_eq!(hdr_le.time_stamp, hdr_be.time_stamp);
        assert_eq!(hdr_le.aux_data, hdr_be.aux_data);
        assert_eq!(hdr_le.hdcd, hdr_be.hdcd);
        assert_eq!(hdr_le.ext_descr, hdr_be.ext_descr);
        assert_eq!(hdr_le.ext_coding, hdr_be.ext_coding);
        assert_eq!(hdr_le.aspf, hdr_be.aspf);
        assert_eq!(hdr_le.lfe, hdr_be.lfe);
        assert_eq!(hdr_le.predictor_history, hdr_be.predictor_history);
        assert_eq!(hdr_le.header_crc, hdr_be.header_crc);
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

    /// Build a 14-bit BE-packed buffer carrying the same DTS frame
    /// the `build_be_header` helper produces in raw-BE form.
    #[allow(clippy::too_many_arguments)]
    fn build_14bit_packed_header(
        order: FourteenBitByteOrder,
        ftype: u32,
        sample_count_m1: u32,
        crc_present: u32,
        nblks: u32,
        fsize_m1: u32,
        amode: u32,
        sfreq: u32,
        rate: u32,
        extra_bits: u32,
        header_crc: Option<u32>,
    ) -> Vec<u8> {
        // Step 1: build the equivalent raw-BE byte buffer using the
        // existing helper.
        let raw_be = build_be_header(
            ftype,
            sample_count_m1,
            crc_present,
            nblks,
            fsize_m1,
            amode,
            sfreq,
            rate,
            extra_bits,
            header_crc,
        );
        // Step 2: walk the raw bit stream MSB-first, emitting 14-bit
        // payloads packed into 16-bit containers in the requested
        // byte order.
        let mut packed: Vec<u8> = Vec::new();
        let mut bit_pos: usize = 0;
        let total_bits = raw_be.len() * 8;
        while bit_pos + 14 <= total_bits {
            let mut payload: u16 = 0;
            for i in 0..14 {
                let abs = bit_pos + i;
                let bit = (raw_be[abs / 8] >> (7 - (abs % 8))) & 1;
                payload = (payload << 1) | bit as u16;
            }
            // Sign-extend bit 13 into bits 14..16 per the wiki's
            // "upper two bits are sign bit extension" rule.
            let container = if payload & 0x2000 != 0 {
                payload | 0xC000
            } else {
                payload & 0x3FFF
            };
            let bytes = match order {
                FourteenBitByteOrder::BigEndian => container.to_be_bytes(),
                FourteenBitByteOrder::LittleEndian => container.to_le_bytes(),
            };
            packed.extend_from_slice(&bytes);
            bit_pos += 14;
        }
        packed
    }

    #[test]
    fn parse_frame_header_14bit_be_matches_raw_be() {
        let raw = build_be_header(
            1,
            31,
            1,
            16,
            1023,
            9,
            13,
            25,
            0b1_0100_1010_0011,
            Some(0xC0DE),
        );
        let packed = build_14bit_packed_header(
            FourteenBitByteOrder::BigEndian,
            1,
            31,
            1,
            16,
            1023,
            9,
            13,
            25,
            0b1_0100_1010_0011,
            Some(0xC0DE),
        );
        let hdr_raw = parse_frame_header(&raw).unwrap();
        let hdr_packed = parse_frame_header_14bit(&packed).unwrap();
        assert_eq!(
            hdr_packed.sync_word_encoding,
            SyncWordEncoding::FourteenBitBigEndian,
        );
        // Every structural field must agree with the raw-BE parse.
        assert_eq!(hdr_packed.frame_type, hdr_raw.frame_type);
        assert_eq!(
            hdr_packed.sample_count_per_block,
            hdr_raw.sample_count_per_block,
        );
        assert_eq!(hdr_packed.crc_present, hdr_raw.crc_present);
        assert_eq!(hdr_packed.blocks_per_frame, hdr_raw.blocks_per_frame);
        assert_eq!(hdr_packed.frame_size_bytes, hdr_raw.frame_size_bytes);
        assert_eq!(hdr_packed.amode, hdr_raw.amode);
        assert_eq!(hdr_packed.sfreq_index, hdr_raw.sfreq_index);
        assert_eq!(hdr_packed.rate_index, hdr_raw.rate_index);
        // Round 3: trailing flags + optional CRC must round-trip
        // identically through 14-bit packing.
        assert_eq!(hdr_packed.downmix, hdr_raw.downmix);
        assert_eq!(hdr_packed.dynamic_range, hdr_raw.dynamic_range);
        assert_eq!(hdr_packed.time_stamp, hdr_raw.time_stamp);
        assert_eq!(hdr_packed.aux_data, hdr_raw.aux_data);
        assert_eq!(hdr_packed.hdcd, hdr_raw.hdcd);
        assert_eq!(hdr_packed.ext_descr, hdr_raw.ext_descr);
        assert_eq!(hdr_packed.ext_coding, hdr_raw.ext_coding);
        assert_eq!(hdr_packed.aspf, hdr_raw.aspf);
        assert_eq!(hdr_packed.lfe, hdr_raw.lfe);
        assert_eq!(hdr_packed.predictor_history, hdr_raw.predictor_history);
        assert_eq!(hdr_packed.header_crc, hdr_raw.header_crc);
    }

    #[test]
    fn parse_frame_header_14bit_le_matches_raw_be() {
        let raw = build_be_header(0, 0, 0, 5, 94, 0, 0, 0, 0, None);
        let packed = build_14bit_packed_header(
            FourteenBitByteOrder::LittleEndian,
            0,
            0,
            0,
            5,
            94,
            0,
            0,
            0,
            0,
            None,
        );
        let hdr_raw = parse_frame_header(&raw).unwrap();
        let hdr_packed = parse_frame_header_14bit(&packed).unwrap();
        assert_eq!(
            hdr_packed.sync_word_encoding,
            SyncWordEncoding::FourteenBitLittleEndian,
        );
        assert_eq!(hdr_packed.frame_type, FrameType::Termination);
        assert_eq!(hdr_packed.frame_type, hdr_raw.frame_type);
        assert_eq!(hdr_packed.blocks_per_frame, hdr_raw.blocks_per_frame);
        assert_eq!(hdr_packed.frame_size_bytes, hdr_raw.frame_size_bytes);
        // No CRC when crc_present == 0.
        assert_eq!(hdr_packed.header_crc, None);
    }

    /// `parse_frame_header_14bit` must reject a raw-16-bit buffer
    /// with `NoSync` so the two entry points stay disjoint.
    #[test]
    fn parse_frame_header_14bit_rejects_raw_be_input() {
        let raw = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0, None);
        assert_eq!(parse_frame_header_14bit(&raw).unwrap_err(), Error::NoSync,);
    }

    #[test]
    fn parse_frame_header_14bit_short_buffer_returns_eof() {
        // Just the 6-byte sync prefix is below the 18-byte minimum.
        let buf = [0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xF0];
        assert_eq!(
            parse_frame_header_14bit(&buf).unwrap_err(),
            Error::UnexpectedEof,
        );
    }

    #[test]
    fn parse_frame_header_14bit_value_resolvers_still_none() {
        let packed = build_14bit_packed_header(
            FourteenBitByteOrder::BigEndian,
            1,
            31,
            1,
            16,
            1023,
            9,
            13,
            25,
            0,
            Some(0),
        );
        let hdr = parse_frame_header_14bit(&packed).unwrap();
        // The SFREQ/RATE/AMODE tables remain missing from docs/; the
        // resolvers must still return None.
        assert_eq!(hdr.sample_rate_hz(), None);
        assert_eq!(hdr.bit_rate_bps(), None);
        assert_eq!(hdr.channel_count(), None);
    }

    #[test]
    fn parse_no_sync_returns_no_sync() {
        let buf = [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_frame_header(&buf).unwrap_err(), Error::NoSync);
    }

    #[test]
    fn value_resolvers_return_none_until_tables_land() {
        let bytes = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0, Some(0));
        let hdr = parse_frame_header(&bytes).unwrap();
        assert_eq!(hdr.sample_rate_hz(), None);
        assert_eq!(hdr.bit_rate_bps(), None);
        assert_eq!(hdr.channel_count(), None);
    }

    // ---------------------------------------------------------------
    // Round 3 — trailing-13-bit field + optional 16-bit HEADER_CRC.
    // ---------------------------------------------------------------

    /// Walk every 2-bit LFE code (0..=3) and verify the [`LfeMode`]
    /// round-trips through the parser.
    #[test]
    fn lfe_mode_codes_round_trip() {
        for code in 0..=3u32 {
            // extra_bits layout (13 bits MSB-first): 11 leading
            // zeros + 2-bit LFE code + 0 predictor.
            //   bits 0..10 = 0  (downmix..aspf, 11 bits total)
            //   bits 11..12 = lfe code (we shift left 1 so
            //                 predictor bit stays 0)
            let extra = (code & 0b11) << 1;
            let bytes = build_be_header(1, 31, 0, 16, 1023, 9, 13, 25, extra, None);
            let hdr = parse_frame_header(&bytes).unwrap();
            assert_eq!(hdr.lfe.code(), code as u8, "code {code}");
            assert_eq!(hdr.lfe.is_present(), code != 0, "is_present({code})");
            // Spot-check the typed enum mapping.
            let expected = match code {
                0 => LfeMode::None,
                1 => LfeMode::Mode1,
                2 => LfeMode::Mode2,
                _ => LfeMode::Mode3,
            };
            assert_eq!(hdr.lfe, expected, "enum mapping for code {code}");
        }
    }

    /// When `crc_present == 0` the parser must NOT consume the
    /// optional 16-bit CRC field; `header_crc` must be `None`.
    #[test]
    fn header_crc_absent_when_crc_present_bit_is_zero() {
        let bytes = build_be_header(1, 31, 0, 16, 1023, 9, 13, 25, 0, None);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert!(!hdr.crc_present);
        assert_eq!(hdr.header_crc, None);
        // verify_header_crc returns None when there is nothing to
        // verify.
        assert_eq!(hdr.verify_header_crc(), None);
    }

    /// When `crc_present == 1` the parser captures the 16-bit field
    /// verbatim; verification still returns `None` because the
    /// polynomial is undocumented.
    #[test]
    fn header_crc_present_returns_raw_field_and_unverified() {
        let bytes = build_be_header(1, 31, 1, 16, 1023, 9, 13, 25, 0, Some(0x1234));
        let hdr = parse_frame_header(&bytes).unwrap();
        assert!(hdr.crc_present);
        assert_eq!(hdr.header_crc, Some(0x1234));
        // Polynomial undocumented -> still None.
        assert_eq!(hdr.verify_header_crc(), None);
    }

    /// All-zeros 13-bit trailing window decodes to all-false flags,
    /// `ext_descr == 0`, and `LfeMode::None`.
    #[test]
    fn trailing_bits_all_zero_decodes_clean() {
        let bytes = build_be_header(1, 31, 0, 16, 1023, 9, 13, 25, 0, None);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert!(!hdr.downmix);
        assert!(!hdr.dynamic_range);
        assert!(!hdr.time_stamp);
        assert!(!hdr.aux_data);
        assert!(!hdr.hdcd);
        assert_eq!(hdr.ext_descr, 0);
        assert!(!hdr.ext_coding);
        assert!(!hdr.aspf);
        assert_eq!(hdr.lfe, LfeMode::None);
        assert!(!hdr.predictor_history);
    }

    /// All-ones 13-bit trailing window decodes to all-true flags,
    /// `ext_descr == 7`, and `LfeMode::Mode3`.
    #[test]
    fn trailing_bits_all_one_decodes_max() {
        let bytes = build_be_header(1, 31, 0, 16, 1023, 9, 13, 25, 0b1_1111_1111_1111, None);
        let hdr = parse_frame_header(&bytes).unwrap();
        assert!(hdr.downmix);
        assert!(hdr.dynamic_range);
        assert!(hdr.time_stamp);
        assert!(hdr.aux_data);
        assert!(hdr.hdcd);
        assert_eq!(hdr.ext_descr, 0b111);
        assert!(hdr.ext_coding);
        assert!(hdr.aspf);
        assert_eq!(hdr.lfe, LfeMode::Mode3);
        assert!(hdr.predictor_history);
    }
}
