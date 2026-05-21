//! # oxideav-dts
//!
//! Pure-Rust DTS Coherent Acoustics decoder for the
//! [oxideav](https://github.com/OxideAV/oxideav) framework.
//!
//! **Status:** clean-room rebuild round 2 (frame-header parser +
//! 14-bit sync unpacking).
//!
//! Round 1 (2026-05-21) landed a structural [`DtsFrameHeader`] parser
//! for the DTS Core frame sync header (per the multimedia.cx wiki
//! snapshot at `docs/audio/dts/wiki/DTS.wiki`, which mirrors the
//! ETSI TS 102 114 §5.3 bit layout). Round 2 (2026-05-21) adds a
//! 14-bit unpacker so both 14-bit container forms decode through
//! the same structural parser as the two 16-bit raw forms.
//! Bitstream / subframe decoding is **not** part of this round.
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
//! The crate `forbid`s `unsafe`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

use oxideav_core::RuntimeContext;

mod bitreader;
mod header;
mod unpack14;

pub use crate::header::{
    parse_frame_header, parse_frame_header_14bit, DtsFrameHeader, FrameType, SyncWordEncoding,
};
pub use crate::unpack14::{unpack_14bit_to_16bit, FourteenBitByteOrder};

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

/// No-op codec registration. Round 1 does not wire a
/// [`oxideav_core::Decoder`] into the runtime yet — only the
/// structural parser is exported as plain functions. Subsequent
/// rounds will register a real decoder here.
pub fn register(_ctx: &mut RuntimeContext) {}

oxideav_core::register!("dts", register);
