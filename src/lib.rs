//! Pure-Rust **DTS Coherent Acoustics (DCA) Core** audio decoder.
//!
//! Round-1 scope: the original 1995-vintage DTS Core layer — the
//! mandatory backwards-compatible foundation that every DTS frame
//! carries. Optional extensions (XCH, XXCH, X96, XBR, LBR, EXSS,
//! XLL) are deferred to round 2+; see `README.md` for the backlog.
//!
//! # Architecture
//!
//! Pipeline mirrors `docs/audio/dts/dts-trace-reverse-engineering.md`
//! §3:
//!
//!   1. [`syncwords`] — 32-bit BE sync words (only `CORE_BE` is
//!      decoded in round 1).
//!   2. [`header`] — 104/120-bit Core frame header.
//!   3. [`audblk`] — audio coding header + per-subframe per-band
//!      ABITS / scale-factor / sample dequantization.
//!   4. [`pqf`] — 32-band polyphase quadrature filterbank synthesis
//!      (sub-bands → PCM).
//!   5. [`decoder`] — `oxideav_core::Decoder` impl that wires the
//!      above into the framework.
//!
//! Static functional tables ([`tables`], [`huffman`], [`vq`]) are
//! all sourced from the clean-room `docs/audio/dts/data/*.md`
//! sidecars — no third-party library source has been consulted.

#![allow(clippy::needless_range_loop)]

pub mod audblk;
pub mod bits;
pub mod decoder;
pub mod header;
pub mod huffman;
pub mod pqf;
pub mod syncwords;
pub mod tables;
pub mod vq;

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, CodecTag, Decoder,
    Result,
};

pub const CODEC_ID_STR: &str = "dts";

/// Register the DTS Core decoder with the supplied codec registry.
pub fn register(reg: &mut CodecRegistry) {
    let cid = CodecId::new(CODEC_ID_STR);
    let dec_caps = CodecCapabilities::audio("dts_sw_dec")
        .with_lossy(true)
        .with_intra_only(true)
        .with_max_channels(8)
        .with_max_sample_rate(192_000);

    // Container tag claims (the values are public per
    // `dts-trace-reverse-engineering.md` §13):
    //   - WAVEFORMATEX::wFormatTag = 0x2001 (AVI / WAV)
    //   - MP4 ObjectTypeIndication 0xA9 (DTS audio)
    //   - Matroska CodecID "A_DTS"
    reg.register(
        CodecInfo::new(cid.clone())
            .capabilities(dec_caps.clone())
            .decoder(make_decoder)
            .tag(CodecTag::wave_format(0x2001)),
    );
    reg.register(
        CodecInfo::new(cid.clone())
            .capabilities(dec_caps.clone())
            .decoder(make_decoder)
            .tag(CodecTag::mp4_object_type(0xA9)),
    );
    reg.register(
        CodecInfo::new(cid)
            .capabilities(dec_caps)
            .decoder(make_decoder)
            .tag(CodecTag::matroska("A_DTS")),
    );
}

fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    decoder::make_decoder(params)
}
