//! # oxideav-dts
//!
//! Pure-Rust DTS Coherent Acoustics decoder for the
//! [oxideav](https://github.com/OxideAV/oxideav) framework.
//!
//! **Status:** clean-room rebuild round 4 (frame-header parser +
//! 14-bit sync unpacking + trailing-flag fields + optional
//! 16-bit header CRC field + `oxideav-core` `Decoder` integration).
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
//! sample frequency / bitrate / channel-configuration; the structural
//! parser therefore returns the raw indices and exposes
//! `Option<u32>` resolvers ([`DtsFrameHeader::sample_rate_hz`],
//! [`DtsFrameHeader::bit_rate_bps`],
//! [`DtsFrameHeader::channel_count`]) that return `None` until the
//! tables land in `docs/`. See `README.md`'s "Docs gaps" section.
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
//! - [`unpack_14bit_to_16bit`] / [`FourteenBitByteOrder`] — the
//!   underlying 14→16-bit unpacker (added in round 2).
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
mod unpack14;

#[cfg(feature = "registry")]
mod registry;

pub use crate::header::{
    parse_frame_header, parse_frame_header_14bit, DtsFrameHeader, FrameType, LfeMode,
    SyncWordEncoding,
};
pub use crate::unpack14::{unpack_14bit_to_16bit, FourteenBitByteOrder};

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
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias for [`Result`] specialised to this crate's
/// [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
