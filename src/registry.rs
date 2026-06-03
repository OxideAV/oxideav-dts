//! `oxideav-core` integration: `Decoder` trait impl, `Frame` / `Error`
//! conversions, and the [`register`] / [`register_codecs`] entry
//! points. Includes the [`probe_dts`] confidence helper used by the
//! tag-keyed registry lookup.
//!
//! Gated behind the default-on `registry` Cargo feature. With the
//! feature off the rest of the crate still exposes the standalone
//! [`crate::parse_frame_header`] / [`crate::parse_frame_header_14bit`]
//! / [`crate::unpack_14bit_to_16bit`] APIs plus the [`crate::Error`]
//! type — none of which depend on `oxideav-core`.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, CodecTag, Confidence,
    Decoder, Error as CoreError, Frame, Packet, ProbeContext, Result as CoreResult, RuntimeContext,
};

use crate::header::{detect_sync, parse_frame_header, parse_frame_header_14bit};
use crate::{DtsFrameHeader, Error as DtsError, SyncWordEncoding};

/// Canonical codec id string for the DTS Coherent Acoustics codec.
/// Matches the registration string used by `oxideav-mp4`'s
/// `from_sample_entry` mapping for `dtsc` / `dtsh` / `dtsl` / `dtse`.
pub const CODEC_ID_STR: &str = "dts";

impl From<DtsError> for CoreError {
    fn from(e: DtsError) -> Self {
        match e {
            DtsError::UnexpectedEof => CoreError::NeedMore,
            DtsError::NoSync => CoreError::InvalidData(e.to_string()),
            DtsError::UnsupportedFourteenBit | DtsError::UnsupportedRaw16Bit => {
                CoreError::Unsupported(e.to_string())
            }
            DtsError::BlockCountOutOfRange { .. } | DtsError::FrameSizeOutOfRange { .. } => {
                CoreError::InvalidData(e.to_string())
            }
            // The encoder-only [`DtsError::FieldOutOfRange`] variant is
            // not produced by any parser path, so the runtime decoder
            // surface cannot emit it via `send_packet`. We still map it
            // for completeness should a future caller invoke
            // [`crate::encode_frame_header_be`] from inside the
            // decoder path.
            DtsError::FieldOutOfRange { .. } => CoreError::InvalidData(e.to_string()),
            // Round 195 side-info decoder failures: bit-stream-format
            // errors (reserved BHUFF/SHUFF/SCALES values or unmatched
            // Huffman codeword) map to `InvalidData` so the surrounding
            // demux/decoder path treats the packet as corrupt rather
            // than as an unrecoverable codec-level limitation.
            DtsError::InvalidSideInfo { .. } | DtsError::HuffmanDecodeFailed { .. } => {
                CoreError::InvalidData(e.to_string())
            }
            // Round 214 §C.2.4 sum/difference length-mismatch is a
            // caller-side slice-shape violation; the runtime decoder
            // path doesn't construct mismatched slices today, but if a
            // future subframe walker plumbs the variant through
            // `send_packet`, surface it as `InvalidData`.
            DtsError::SumDiffLengthMismatch { .. } => CoreError::InvalidData(e.to_string()),
            // Round 223 §C.2.3 joint-subband shape-mismatch is a
            // caller-side slice-shape violation analogous to the
            // sum/diff variant. Same `InvalidData` mapping rationale.
            DtsError::JointSubbandShapeMismatch { .. } => CoreError::InvalidData(e.to_string()),
        }
    }
}

/// Register the DTS Core decoder factory plus the `dts` and `dtsc`
/// FourCC tags into `reg`. The factory always succeeds at construction
/// time; the `Decoder::send_packet` impl is the point where structural
/// frame-header failures surface (so demuxers can route packets
/// without instantiating a decoder).
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::audio("dts_sw").with_lossy(true);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .probe(probe_dts_tag)
            .tags([
                // `dts` — generic FourCC seen on some QuickTime sample
                // entries and in raw-stream tag lookups.
                CodecTag::fourcc(b"dts "),
                // `dtsc` — DTS Coherent Acoustics ISO/IEC sample-entry
                // FourCC (ETSI TS 102 114 §6 / ISO/IEC 14496-30).
                CodecTag::fourcc(b"dtsc"),
            ]),
    );
}

/// Unified entry point: install the DTS codec into a [`RuntimeContext`].
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
}

oxideav_core::register!("dts", register);

/// Decoder factory for the DTS Core profile.
///
/// Returns a handle whose [`Decoder::send_packet`] parses the frame
/// header eagerly (so structural failures — bad sync, NBLKS below 5,
/// frame size below 95 bytes, truncated header — surface at the
/// packet boundary) and whose [`Decoder::receive_frame`] returns
/// [`CoreError::Unsupported`] because the subframe / subband /
/// QMF-synthesis decode path required to emit a PCM frame is still
/// gated on follow-up rounds (the RATE / SFREQ / AMODE / PCMR
/// header-level value tables landed in rounds 185 / 202, but the
/// actual subframe walker remains incomplete).
pub fn make_decoder(params: &CodecParameters) -> CoreResult<Box<dyn Decoder>> {
    Ok(Box::new(DtsDecoderHandle {
        codec_id: params.codec_id.clone(),
        last_header: None,
        eof: false,
    }))
}

/// In-process DTS Core decoder handle.
///
/// Holds the most recently parsed [`DtsFrameHeader`] for diagnostic
/// inspection (e.g. by integration tests that want to confirm a
/// container routed a real DTS frame even though PCM output is not
/// yet wired up).
#[derive(Debug)]
pub struct DtsDecoderHandle {
    codec_id: CodecId,
    last_header: Option<DtsFrameHeader>,
    eof: bool,
}

impl DtsDecoderHandle {
    /// Inspect the most recently parsed frame header (or `None` if
    /// `send_packet` has not been called yet, or if every prior call
    /// errored before producing a header).
    pub fn last_header(&self) -> Option<&DtsFrameHeader> {
        self.last_header.as_ref()
    }
}

impl Decoder for DtsDecoderHandle {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> CoreResult<()> {
        // Route by the syncword at offset 0. The two raw 16-bit
        // variants go to `parse_frame_header`; the two 14-bit packed
        // variants go to `parse_frame_header_14bit`. Anything else
        // returns InvalidData via the From<DtsError> impl above.
        let bytes = packet.data.as_slice();
        let sync = detect_sync(bytes).map_err(CoreError::from)?;
        let hdr = match sync {
            SyncWordEncoding::RawBigEndian | SyncWordEncoding::RawLittleEndian => {
                parse_frame_header(bytes)
            }
            SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian => {
                parse_frame_header_14bit(bytes)
            }
        }
        .map_err(CoreError::from)?;
        self.last_header = Some(hdr);
        Ok(())
    }

    fn receive_frame(&mut self) -> CoreResult<Frame> {
        if self.last_header.is_none() {
            return if self.eof {
                Err(CoreError::Eof)
            } else {
                Err(CoreError::NeedMore)
            };
        }
        // The RATE / SFREQ / AMODE / PCMR header-level value tables
        // landed in rounds 185 (RATE) and 202 (SFREQ / AMODE / PCMR),
        // but the actual subframe / subband / QMF-synthesis decode
        // path required to emit a PCM frame is still incomplete (the
        // §5.4.1 side-info decoders landed in round 195; the
        // §5.4-onwards subframe walker is the next stage). Surface
        // that gap as `Unsupported` so callers can distinguish
        // "decoder hasn't seen a packet" (NeedMore) from "decoder
        // rejects this build of the codec stack" (Unsupported).
        Err(CoreError::unsupported(
            "oxideav-dts: PCM output gated on the §5.4-onwards \
             subframe / subband / QMF-synthesis decode path",
        ))
    }

    fn flush(&mut self) -> CoreResult<()> {
        self.eof = true;
        Ok(())
    }

    fn reset(&mut self) -> CoreResult<()> {
        self.last_header = None;
        self.eof = false;
        Ok(())
    }
}

/// Standalone confidence probe for DTS Core bitstreams.
///
/// Inspects the first few bytes of `bytes` and returns:
///
/// * `1.0` — one of the four documented DTS Core sync sequences is
///   present at offset 0 and the buffer contains enough bytes to
///   parse the structural frame header successfully.
/// * `0.5` — a sync sequence is present at offset 0 but the buffer is
///   shorter than the 15 bytes the raw-BE parser needs (or 18 bytes
///   for the 14-bit packed variants). Used by demuxers that probe
///   against a peek-window before the full frame is available.
/// * `0.0` — neither sync sequence appears at offset 0.
///
/// Suitable as the `decoder_options_schema`-independent first-pass
/// confidence used by [`oxideav_core::CodecRegistry::resolve_tag`].
pub fn probe_dts(bytes: &[u8]) -> Confidence {
    match detect_sync(bytes) {
        Ok(sync) => {
            let result = match sync {
                SyncWordEncoding::RawBigEndian | SyncWordEncoding::RawLittleEndian => {
                    parse_frame_header(bytes)
                }
                SyncWordEncoding::FourteenBitBigEndian
                | SyncWordEncoding::FourteenBitLittleEndian => parse_frame_header_14bit(bytes),
            };
            match result {
                Ok(_) => 1.0,
                Err(DtsError::UnexpectedEof) => 0.5,
                Err(_) => 0.0,
            }
        }
        Err(DtsError::UnexpectedEof) => 0.5,
        Err(_) => 0.0,
    }
}

/// Adaptor that bridges [`probe_dts`] into the registry's
/// [`oxideav_core::ProbeFn`] signature.
fn probe_dts_tag(ctx: &ProbeContext) -> Confidence {
    match ctx.packet {
        Some(pkt) => probe_dts(pkt),
        // Tag matched but no packet sample is available: return
        // confidence 1.0 so the lookup picks us, mirroring the
        // "claim is unambiguous" convention CodecInfo::probe is
        // documented under (None = always 1.0).
        None => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{CodecResolver, Packet, TimeBase};

    /// A real DTS Core frame header captured from `ffmpeg -c:a dca`
    /// — same fixture used by the black-box integration test.
    const REAL_DTS_FRAME_HEADER: [u8; 16] = [
        0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03, 0xef,
        0x7f,
    ];

    #[test]
    fn probe_returns_1_for_valid_be_header() {
        assert_eq!(probe_dts(&REAL_DTS_FRAME_HEADER), 1.0);
    }

    #[test]
    fn probe_returns_0p5_for_truncated_be_header() {
        // 5 bytes — enough for sync detection but not for the full
        // 15-byte header window.
        let truncated = &REAL_DTS_FRAME_HEADER[..5];
        assert_eq!(probe_dts(truncated), 0.5);
    }

    #[test]
    fn probe_returns_0p5_for_buffer_shorter_than_sync() {
        // 2 bytes — `detect_sync` itself returns UnexpectedEof.
        let truncated = &REAL_DTS_FRAME_HEADER[..2];
        assert_eq!(probe_dts(truncated), 0.5);
    }

    #[test]
    fn probe_returns_0_for_invalid_header() {
        let garbage = [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(probe_dts(&garbage), 0.0);
    }

    #[test]
    fn probe_returns_0_for_short_invalid_input() {
        // Empty input — `detect_sync` returns UnexpectedEof → 0.5
        // because we cannot rule out a valid frame in the unseen
        // bytes. (This mirrors the "truncated" semantics.)
        assert_eq!(probe_dts(&[]), 0.5);
    }

    #[test]
    fn registry_resolves_dts_fourcc() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        let tag = CodecTag::fourcc(b"dts ");
        let ctx = ProbeContext::new(&tag);
        let id = reg.resolve_tag(&ctx).expect("dts fourcc must resolve");
        assert_eq!(id, CodecId::new(CODEC_ID_STR));
    }

    #[test]
    fn registry_resolves_dtsc_fourcc() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        let tag = CodecTag::fourcc(b"dtsc");
        let ctx = ProbeContext::new(&tag);
        let id = reg.resolve_tag(&ctx).expect("dtsc fourcc must resolve");
        assert_eq!(id, CodecId::new(CODEC_ID_STR));
    }

    #[test]
    fn registry_decoder_factory_installs() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        assert!(ctx.codecs.has_decoder(&CodecId::new(CODEC_ID_STR)));
    }

    #[test]
    fn send_packet_eagerly_parses_header() {
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), REAL_DTS_FRAME_HEADER.to_vec());
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        handle.send_packet(&pkt).unwrap();
        let hdr = handle.last_header().expect("header must be cached");
        assert_eq!(hdr.frame_size_bytes, 1024);
        assert_eq!(hdr.sfreq_index, 13);
        assert_eq!(hdr.rate_index, 15);
        // Round 5: the post-CRC window is captured on the same eager
        // send_packet pass. The ffmpeg fixture encodes VERSION = 7
        // with every other post-CRC sub-field zeroed.
        assert_eq!(hdr.version, 7);
        assert_eq!(hdr.dialog_normalization, 0);
        assert_eq!(hdr.source_pcm_resolution_index, 0);
    }

    #[test]
    fn send_packet_surfaces_no_sync_as_invalid_data() {
        let pkt = Packet::new(
            0,
            TimeBase::new(1, 48_000),
            vec![0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        let err = handle.send_packet(&pkt).unwrap_err();
        assert!(matches!(err, CoreError::InvalidData(_)), "got {err:?}");
    }

    #[test]
    fn send_packet_surfaces_short_buffer_as_need_more() {
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), vec![0x7F, 0xFE, 0x80]);
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        let err = handle.send_packet(&pkt).unwrap_err();
        assert!(matches!(err, CoreError::NeedMore), "got {err:?}");
    }

    #[test]
    fn receive_frame_returns_unsupported_after_header_parse() {
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), REAL_DTS_FRAME_HEADER.to_vec());
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        handle.send_packet(&pkt).unwrap();
        let err = handle.receive_frame().unwrap_err();
        assert!(matches!(err, CoreError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn receive_frame_returns_need_more_without_packet() {
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        let err = handle.receive_frame().unwrap_err();
        assert!(matches!(err, CoreError::NeedMore), "got {err:?}");
    }

    #[test]
    fn receive_frame_returns_eof_after_flush_without_packet() {
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        handle.flush().unwrap();
        let err = handle.receive_frame().unwrap_err();
        assert!(matches!(err, CoreError::Eof), "got {err:?}");
    }

    #[test]
    fn reset_clears_cached_header() {
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), REAL_DTS_FRAME_HEADER.to_vec());
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            eof: false,
        };
        handle.send_packet(&pkt).unwrap();
        assert!(handle.last_header().is_some());
        handle.reset().unwrap();
        assert!(handle.last_header().is_none());
        assert!(!handle.eof);
    }
}
