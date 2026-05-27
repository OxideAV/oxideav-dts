//! # oxideav-dts
//!
//! Pure-Rust DTS Coherent Acoustics decoder for the
//! [oxideav](https://github.com/OxideAV/oxideav) framework.
//!
//! **Status:** clean-room rebuild round 6 (frame-header parser +
//! 14-bit sync unpacking + trailing-flag fields + optional
//! 16-bit header CRC field + 16-bit post-CRC trailing window
//! [multirate-inter / version / copy-history / PCMR / front-sum /
//! surround-sum / dialnorm] + `oxideav-core` `Decoder`
//! integration + multi-frame iterator / resync helper).
//!
//! Round 1 (2026-05-21) landed a structural [`DtsFrameHeader`] parser
//! for the DTS Core frame sync header (per the multimedia.cx wiki
//! snapshot at `docs/audio/dts/wiki/DTS.wiki`, which mirrors the
//! ETSI TS 102 114 §5.3 bit layout). Round 2 (2026-05-21) adds a
//! 14-bit unpacker so both 14-bit container forms decode through
//! the same structural parser as the two 16-bit raw forms.
//! Round 3 (2026-05-21) extends the typed header through the 13
//! trailing single-bit / small-field flags after RATE (downmix,
//! dynamic-range, time-stamp, aux-data, HDCD, ext-audio-descr,
//! ext-audio-coding, ASPF, 2-bit LFE mode, predictor-history) plus
//! the optional 16-bit `HEADER_CRC` field that follows when
//! [`DtsFrameHeader::crc_present`] is set. The CRC polynomial is
//! not yet documented in `docs/`, so
//! [`DtsFrameHeader::verify_header_crc`] returns `None` for now;
//! the raw 16-bit field is still surfaced for pass-through callers.
//! Round 4 (2026-05-22) wires the crate into `oxideav-core`'s
//! [`oxideav_core::Decoder`] surface (behind a default-on `registry`
//! cargo feature) plus a standalone [`probe_dts`] helper. The
//! `DtsDecoderHandle` returned by the factory parses the frame
//! header eagerly inside `send_packet`; `receive_frame` returns
//! `Error::Unsupported` because PCM output is gated on the
//! SFREQ/RATE/AMODE value tables landing in `docs/`. Bitstream /
//! subframe decoding is **not** part of this round.
//!
//! Round 5 (2026-05-25) extends [`DtsFrameHeader`] through the
//! 16-bit post-CRC trailing window the wiki snapshot enumerates
//! after `HEADER_CRC`: `multirate_inter` (1), `version` (4),
//! `copy_history` (2), `source_pcm_resolution_index` (3),
//! `front_sum` (1), `surround_sum` (1), `dialog_normalization`
//! (4). These bits are consumed unconditionally (the wiki shows
//! them following the HEADER_CRC slot whether or not CRC was
//! emitted). The PCMR→bits-per-sample and DIALNORM→dB mappings
//! are still missing from `docs/`, so
//! [`DtsFrameHeader::source_pcm_bits_per_sample`] and
//! [`DtsFrameHeader::dialog_normalization_db`] return `None`
//! until those tables land. Bitstream / subframe decoding is
//! still **not** part of this round.
//!
//! Round 6 (2026-05-25) adds a multi-frame iterator and a resync
//! helper on top of the existing single-frame parsers:
//! [`find_next_sync`] scans a byte buffer for the next DTS sync
//! sequence at or after a given offset (all four documented
//! encodings); [`iter_frames`] walks a raw-16-bit DTS Core byte
//! stream frame by frame, using each frame's
//! [`DtsFrameHeader::frame_size_bytes`] to advance to the next
//! sync. A new 5-frame ffmpeg-generated fixture
//! (`tests/fixtures/dts_5_frames.bin`, 5 120 bytes) exercises the
//! iterator end-to-end and confirms every frame's header decodes
//! identically. The iterator is documented as raw-16-bit-only
//! because the wiki snapshot does not enumerate the 14-bit
//! container-byte advance rule (see `README.md`'s round-6 docs
//! gap #7); the iterator therefore yields
//! [`Error::UnsupportedFourteenBit`] and terminates if a 14-bit
//! sync is encountered.
//!
//! Round 141 (2026-05-26) closes the parse↔encode round-trip on the
//! frame-sync header window: [`encode_frame_header_be`] serialises a
//! parsed [`DtsFrameHeader`] back into the raw-BE on-wire bytes
//! prescribed by the wiki bit-table (exactly
//! [`DtsFrameHeader::header_byte_length`] bytes long — 13 or 15 — and
//! always beginning with the canonical raw-BE sync). The encoder is
//! the inverse of [`parse_frame_header`] and validates the same
//! structural bounds plus per-field bit-width bounds via the new
//! [`Error::FieldOutOfRange`] variant.
//!
//! Round 151 (2026-05-26) adds [`find_all_syncs`], the bulk-scan
//! counterpart to [`find_next_sync`]: instead of returning the first
//! sync at or after a cursor, it walks the entire input buffer and
//! returns every documented sync occurrence (all four encodings) as a
//! `Vec<SyncMatch>`. Useful for stream-integrity tooling that needs to
//! know about every resync point up front rather than walking one at a
//! time. Same `O(n)` cost as a `find_next_sync` loop from cursor + 1;
//! the bulk helper just materialises the result. The round also closes
//! a missing coverage gap by testing [`iter_frames`] against a raw-LE
//! multi-frame stream — the iterator already supported raw-LE because
//! `frame_size_bytes` is byte-equivalent across both raw encodings
//! (per the wiki), but the previous test grid only exercised raw-BE
//! via the bundled ffmpeg fixture.
//!
//! Round 159 (2026-05-27) adds an error-tolerant counterpart to
//! [`iter_frames`]: [`iter_frames_resync`] / [`FrameIteratorResync`]
//! treat a candidate sync whose subsequent header bits fail the
//! structural NBLKS / FSIZE bounds (or whose declared
//! `frame_size_bytes` overruns end-of-buffer) as a false-positive
//! sync rather than a hard fault — they surface a [`ResyncEvent`]
//! documenting the discarded offset + cause and continue scanning
//! one byte further instead of terminating. This lets demuxers and
//! stream-integrity tooling walk a partially-corrupted `.dts`
//! stream past malformed-sync patches and recover frames after the
//! damage. The fail-fast [`iter_frames`] remains exactly as
//! documented (returns the first parse error and terminates).
//!
//! Round 165 (2026-05-27) gates the inner-loop multi-byte
//! [`crate::SyncWordEncoding`] detection of [`find_next_sync`] (and
//! therefore [`find_all_syncs`], [`iter_frames`], and
//! [`iter_frames_resync`]) behind a one-byte first-byte filter. The
//! four documented sync sequences all begin with one of `0x7F` /
//! `0xFE` / `0x1F` / `0xFF` (distinct first bytes per the wiki
//! bit-table), so 252 of 256 possible payload bytes are
//! short-circuited before the multi-byte comparison fires. On
//! random payload the inner loop visits ~98.4% of positions with a
//! single byte read + branch rather than the previous 4-byte raw
//! check + 6-byte 14-bit check. The walk order, returned offsets,
//! and matched encoding tags are unchanged from round 6 — round 165
//! also adds 8 new tests (171 total) including a
//! `find_next_sync_matches_pre_optimization_reference_on_candidate_dense_payload`
//! equivalence harness, a pseudo-random-buffer cross-check against
//! a brute-force pre-optimisation reference, an all-`0xFF` payload
//! stress test (every position is a first-byte candidate so the
//! gate's negative-filter property must hold from the
//! multi-byte side), and an exhaustive 256-input check that the
//! first-byte filter accepts exactly `{0x1F, 0x7F, 0xFE, 0xFF}`.
//!
//! Round 148 (2026-05-26) completes the encoder surface across all
//! four documented sync encodings. The two new primitives,
//! [`encode_frame_header_14bit_be`] and [`encode_frame_header_14bit_le`],
//! compose [`encode_frame_header_be`] with [`pack_16bit_to_14bit`]:
//! the raw-BE 13 or 15-byte header window is zero-padded to 16 bytes
//! (the minimum the parser's 14-bit pre-unpack step needs to land a
//! 16-byte raw-BE window) and re-packed into 14-bit-payload containers
//! in the requested byte order. Both encoders emit exactly **18 bytes**
//! (nine 14-bit containers carrying 126 payload bits) regardless of
//! `crc_present`; the trailing 16 padding bits land in what would be
//! the first SUBFRAMES bits of a real frame and are inert for
//! parsing. The 14-bit-LE output is the pairwise byte-swap of the
//! 14-bit-BE output, matching the wiki's `1F FF E8 00 …` vs
//! `FF 1F 00 E8 …` sync-prefix relationship. The
//! `parse_frame_header_14bit(encode_frame_header_14bit_<be|le>(hdr))`
//! round-trip recovers `hdr` on every field except `sync_word_encoding`
//! (the parser reports the encoding it detected at the input).
//!
//! Round 145 (2026-05-26) extends the encoder side with two new
//! primitives: [`encode_frame_header_le`] emits the raw-LE on-wire
//! header window (canonical sync `FE 7F 01 80`, always 16 bytes long
//! — the parser's minimum input length for the raw-LE branch — i.e.
//! `encode_frame_header_be` zero-padded to 16 and 16-bit-word-swapped);
//! and [`pack_16bit_to_14bit`] is the inverse of
//! [`unpack_14bit_to_16bit`], packing an MSB-first 16-bit-equivalent
//! byte stream into 14-bit-payload containers with the wiki's "sign
//! bit extension" rule applied to the upper 2 bits of each container.
//! `pack_16bit_to_14bit` returns the packed bytes plus the
//! `payload_bit_count` so callers can recover the exact pre-pack bit
//! length on the receiving end; together with the existing
//! `unpack_14bit_to_16bit` it completes the bidirectional 14↔16-bit
//! container conversion the wiki snapshot prescribes. The 14-bit
//! sync prefix bytes `1F FF E8 00 …` (BE) and `FF 1F 00 E8 …` (LE)
//! the wiki documents are reproduced byte-for-byte by feeding the
//! 32-bit raw-BE syncword `7F FE 80 01` into `pack_16bit_to_14bit`
//! (the last container of the wiki's example carries 10 bits of the
//! following field that are not part of the syncword — that's why
//! the wiki shows `07 Fx` rather than a literal trailing byte).
//!
//! Round 138 (2026-05-26) surfaces the header→SUBFRAMES boundary
//! through two new accessors and one [`FrameView`] helper:
//! [`DtsFrameHeader::header_bit_length`] returns the bit-count the
//! parser consumed (104 or 120 depending on `crc_present`, both
//! exact multiples of 8 by the wiki bit-table arithmetic);
//! [`DtsFrameHeader::header_byte_length`] returns the byte-count
//! (13 or 15); and [`FrameView::payload`] carves out the SUBFRAMES
//! region (`data[header_byte_length()..]`) so downstream re-muxers
//! and the future subframe decoder can address the payload window
//! without recomputing the header boundary. The values are
//! fully derived from the wiki bit-table in
//! `docs/audio/dts/wiki/DTS.wiki`; no new doc dependency.
//!
//! The parser distinguishes the four documented bitstream encodings
//! via the 32-bit (or 40-bit) syncword (see [`SyncWordEncoding`]) and
//! decodes the structural fields whose semantics are spelled out
//! verbatim in the wiki:
//!
//! - frame type (termination vs normal),
//! - per-block sample count (`deficit + 1`),
//! - CRC-present flag,
//! - number of blocks in the frame (5..=128),
//! - frame size in bytes (95..=16384),
//! - channel configuration index (0..=15 standard, 16..=63
//!   user-defined),
//! - sample-frequency index (4 bits),
//! - transmission-bitrate index (5 bits).
//!
//! The wiki snapshot does **not** mirror the *value* tables for
//! sample frequency / bitrate / channel-configuration / source
//! PCM resolution / dialog normalization; the structural parser
//! therefore returns the raw indices and exposes `Option`
//! resolvers ([`DtsFrameHeader::sample_rate_hz`],
//! [`DtsFrameHeader::bit_rate_bps`],
//! [`DtsFrameHeader::channel_count`],
//! [`DtsFrameHeader::source_pcm_bits_per_sample`],
//! [`DtsFrameHeader::dialog_normalization_db`]) that return
//! `None` until the tables land in `docs/`. See `README.md`'s
//! "Docs gaps" section.
//!
//! ## What does *not* belong here
//!
//! - Container muxing (Wav / MP4 / Matroska carriage).
//! - DTS-HD / EXSS / XLL / X96 / XCH extension substreams.
//! - PCM decoding (subband + QMF + Huffman, all deferred to future
//!   rounds).
//!
//! ## Public API
//!
//! - [`DtsFrameHeader`] — typed parse result.
//! - [`SyncWordEncoding`] — the four documented sync variants.
//! - [`FrameType`] — termination vs normal.
//! - [`parse_frame_header`] — non-allocating single-frame parser
//!   for the two raw 16-bit syncs.
//! - [`parse_frame_header_14bit`] — single-frame parser for the two
//!   14-bit packed syncs (added in round 2).
//! - [`encode_frame_header_be`] — inverse of [`parse_frame_header`]
//!   that emits the wiki bit-table back into raw-BE bytes (added in
//!   round 141).
//! - [`encode_frame_header_le`] — raw-LE encoder variant
//!   (`encode_frame_header_be` zero-padded to 16 bytes + 16-bit-word
//!   swap; added in round 145).
//! - [`encode_frame_header_14bit_be`] / [`encode_frame_header_14bit_le`]
//!   — 14-bit-packed encoder variants (`encode_frame_header_be` padded
//!   to 16 bytes then re-packed through [`pack_16bit_to_14bit`]; added
//!   in round 148). Both emit exactly 18 bytes regardless of
//!   `crc_present`, matching the parser's minimum 14-bit input length.
//! - [`unpack_14bit_to_16bit`] / [`pack_16bit_to_14bit`] /
//!   [`FourteenBitByteOrder`] — the 14↔16-bit container conversion
//!   primitives. `unpack_14bit_to_16bit` added in round 2;
//!   `pack_16bit_to_14bit` added in round 145.
//! - [`find_next_sync`] / [`find_all_syncs`] / [`iter_frames`] /
//!   [`FrameIterator`] / [`FrameView`] / [`SyncMatch`] — multi-frame
//!   walker + resync helpers. `find_next_sync` / `iter_frames` /
//!   `FrameIterator` / `FrameView` / `SyncMatch` added in round 6;
//!   `find_all_syncs` added in round 151.
//! - [`iter_frames_resync`] / [`FrameIteratorResync`] /
//!   [`ResyncEvent`] / [`ResyncCause`] — error-tolerant walker that
//!   skips past false-positive sync candidates (added in round 159).
//! - [`Error`] — crate-local error type.
//!
//! Behind the default-on `registry` cargo feature (round 4):
//!
//! - [`register`] / [`register_codecs`] — wire the DTS decoder factory
//!   plus `dts` / `dtsc` FourCC tags into an
//!   [`oxideav_core::RuntimeContext`] / [`oxideav_core::CodecRegistry`].
//! - [`make_decoder`] — factory that builds a boxed
//!   [`oxideav_core::Decoder`] (the [`DtsDecoderHandle`]).
//! - [`DtsDecoderHandle`] — the decoder handle. `send_packet` eagerly
//!   parses the frame header; `receive_frame` returns
//!   `Error::Unsupported` because PCM output is still blocked on
//!   docs gaps.
//! - [`probe_dts`] — standalone confidence helper (1.0 / 0.5 / 0.0).
//! - [`CODEC_ID_STR`] — canonical codec id `"dts"`.
//!
//! The crate `forbid`s `unsafe`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod bitreader;
mod header;
mod iter;
mod unpack14;

#[cfg(feature = "registry")]
mod registry;

pub use crate::header::{
    encode_frame_header_14bit_be, encode_frame_header_14bit_le, encode_frame_header_be,
    encode_frame_header_le, parse_frame_header, parse_frame_header_14bit, DtsFrameHeader,
    FrameType, LfeMode, SyncWordEncoding,
};
pub use crate::iter::{
    find_all_syncs, find_next_sync, iter_frames, iter_frames_resync, FrameIterator,
    FrameIteratorResync, FrameView, ResyncCause, ResyncEvent, SyncMatch,
};
pub use crate::unpack14::{pack_16bit_to_14bit, unpack_14bit_to_16bit, FourteenBitByteOrder};

#[cfg(feature = "registry")]
pub use crate::registry::{
    make_decoder, probe_dts, register, register_codecs, DtsDecoderHandle, CODEC_ID_STR,
};

// `oxideav_core::register!("dts", register)` lives inside the
// `registry` submodule; its `__oxideav_entry` wrapper needs to be
// reachable at the crate root so `oxideav-meta`'s build-time
// discovery (which calls `<crate>::__oxideav_entry(ctx)`) finds it.
#[cfg(feature = "registry")]
pub use crate::registry::__oxideav_entry;

/// Crate-local error type. Round 1 surfaces only the parser-related
/// variants; future rounds will extend this enum as decoding stages
/// land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input buffer was too short for the field being read.
    UnexpectedEof,
    /// None of the four documented DTS sync words matched the first
    /// 4–5 bytes of the input.
    NoSync,
    /// A 14-bit DTS sync was detected at the 16-bit-input entry
    /// point [`parse_frame_header`]. Round 2 added a dedicated
    /// [`parse_frame_header_14bit`] entry point plus
    /// [`unpack_14bit_to_16bit`] for callers that want to convert
    /// 14-bit-packed bytes into the raw-BE form. This variant
    /// remains for callers that route by sync up-front.
    UnsupportedFourteenBit,
    /// The decoded `NBLKS` field reported fewer than 5 blocks per
    /// frame — the wiki/spec disallow this.
    BlockCountOutOfRange {
        /// Decoded number of blocks (after the +1 increment).
        blocks: u8,
    },
    /// The decoded frame-size field reported fewer than 95 bytes —
    /// the wiki/spec disallow this.
    FrameSizeOutOfRange {
        /// Decoded frame size in bytes (after the +1 increment).
        frame_size: u16,
    },
    /// A field passed to [`crate::encode_frame_header_be`] does not
    /// fit the bit-width the wiki bit-table documents (e.g. AMODE > 63
    /// for a 6-bit field, VERSION > 15 for a 4-bit field, or a
    /// `header_crc: Some(_)` paired with `crc_present == false`).
    /// Only the encoder returns this variant; the parser cannot
    /// produce out-of-range values because every field is read from
    /// the bit-vector through a width-bounded read.
    FieldOutOfRange {
        /// Static name of the offending [`crate::DtsFrameHeader`]
        /// field.
        field: &'static str,
        /// Caller-supplied value.
        value: u32,
        /// Maximum value the field's documented bit-width can hold.
        /// For the `header_crc` mismatch variant this is set to `0`
        /// (the value being out-of-range is the `Some` vs `None`
        /// disagreement with `crc_present`, not the integer payload).
        max: u32,
    },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::UnexpectedEof => write!(f, "oxideav-dts: unexpected end of input"),
            Error::NoSync => {
                write!(f, "oxideav-dts: no DTS sync word found at offset 0")
            }
            Error::UnsupportedFourteenBit => write!(
                f,
                "oxideav-dts: 14-bit DTS sync detected at the 16-bit-input \
                 entry point; call parse_frame_header_14bit (or \
                 unpack_14bit_to_16bit + parse_frame_header) instead"
            ),
            Error::BlockCountOutOfRange { blocks } => write!(
                f,
                "oxideav-dts: NBLKS={blocks} is out of the documented 5..=128 \
                 range (spec mandates >=5)"
            ),
            Error::FrameSizeOutOfRange { frame_size } => write!(
                f,
                "oxideav-dts: frame size {frame_size} B is out of the documented \
                 95..=16384 range (spec mandates >=95)"
            ),
            Error::FieldOutOfRange { field, value, max } => write!(
                f,
                "oxideav-dts: field `{field}` value {value} exceeds the wiki \
                 bit-table maximum {max}"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias for [`Result`] specialised to this crate's
/// [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
