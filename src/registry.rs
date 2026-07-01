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
    AudioFrame, CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, CodecTag,
    Confidence, Decoder, Error as CoreError, Frame, Packet, ProbeContext, Result as CoreResult,
    RuntimeContext,
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
            // Round 228 §C.2.2 inverse-ADPCM shape-mismatch is the
            // same flavour of caller-side slice-shape violation: the
            // history or coefficient slice has a length other than
            // the spec's `NumADPCMCoeff = 4`. `InvalidData` for parity
            // with the sum/diff and joint-subband mappings.
            DtsError::InverseAdpcmShapeMismatch { .. } => CoreError::InvalidData(e.to_string()),
            // Round 232 §C.2.1 block-code errors. `n_levels < 2` is a
            // structural / caller-side violation analogous to the
            // §C.2.2/3/4 shape-mismatch variants; a residual block code
            // word indicates bit-stream corruption (the §C.2.1 success
            // criterion `nCode == 0` is unmet). Both map to
            // `InvalidData` for parity with the surrounding §C.2.x
            // failure modes.
            DtsError::BlockCodeLevelsOutOfRange { .. } | DtsError::BlockCodeResidual { .. } => {
                CoreError::InvalidData(e.to_string())
            }
            // Round 293 §D.2 / §5.5 dequantization errors. A reserved
            // `ABITS` step-size index (`27..=31`) indicates a corrupt
            // bit stream (the §5.5 `Audio Data` block only ever selects
            // a quantizer for a defined `ABITS`), so `InvalidData`; the
            // §5.5 eight-sample shape-mismatch is the same caller-side
            // slice-shape violation as the §C.2.x variants above.
            DtsError::InvalidStepSize { .. } | DtsError::SampleCountMismatch { .. } => {
                CoreError::InvalidData(e.to_string())
            }
            // Round 306 §5.5 DSYNC trailer mismatch: a subsubframe
            // synchronization check word other than `0xffff` is the
            // Core profile's in-band integrity signal for a corrupt
            // audio-data array, so map it to `InvalidData` alongside the
            // other bit-stream-corruption variants above.
            DtsError::DsyncMismatch { .. } => CoreError::InvalidData(e.to_string()),
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
/// packet boundary) and caches the raw-16-bit frame bytes, and whose
/// [`Decoder::receive_frame`] runs the §5.3/§5.4/§5.5 + §C.2.5
/// [`crate::decode_core_frame`] reconstruction to emit a planar S32
/// [`AudioFrame`] for the common Core case. Frames with a Table 5-28
/// side-info tail (`DYNF`/`CPF`/`JOINX`), a §D.10 VQ/ADPCM blocker, or a
/// 14-bit container payload surface [`CoreError::Unsupported`].
pub fn make_decoder(params: &CodecParameters) -> CoreResult<Box<dyn Decoder>> {
    Ok(Box::new(DtsDecoderHandle {
        codec_id: params.codec_id.clone(),
        last_header: None,
        last_frame_bytes: None,
        last_pts: 0,
        stream: None,
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
    /// The most recent packet's full (unpacked, raw-16-bit) frame bytes,
    /// kept so [`Decoder::receive_frame`] can run the §5.3/§5.4/§5.5 +
    /// §C.2.5 reconstruction over the whole frame, not just the header.
    last_frame_bytes: Option<Vec<u8>>,
    /// The PTS carried by the most recent packet, forwarded onto the
    /// emitted [`Frame`].
    last_pts: i64,
    /// The persistent §C.2.5 stream decoder, carrying each channel's
    /// inter-frame filter tail (`raX[]` / `raZ[]`) across packets — a
    /// DTS elementary stream's QMF filter is continuous, so resetting it
    /// per packet would inject a warmup transient at every frame
    /// boundary. Lazily (re)constructed on the first packet and whenever
    /// the channel count changes (a stream's `nPCHS` is constant in
    /// practice, but the handle tolerates a mid-stream change by
    /// restarting the filter for the new layout).
    stream: Option<crate::CoreStreamDecoder>,
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
                let hdr = parse_frame_header(bytes).map_err(CoreError::from)?;
                // Raw 16-bit frames are already in the domain
                // `decode_core_frame` operates on; keep the bytes so
                // receive_frame can reconstruct PCM.
                self.last_frame_bytes = Some(bytes.to_vec());
                hdr
            }
            SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian => {
                // 14-bit container frames would need unpacking to the
                // 16-bit-word domain before reconstruction; that path is
                // not wired here yet, so cache only the header.
                self.last_frame_bytes = None;
                parse_frame_header_14bit(bytes).map_err(CoreError::from)?
            }
        };
        self.last_header = Some(hdr);
        self.last_pts = packet.pts.unwrap_or(0);
        Ok(())
    }

    fn receive_frame(&mut self) -> CoreResult<Frame> {
        let Some(header) = self.last_header.take() else {
            return if self.eof {
                Err(CoreError::Eof)
            } else {
                Err(CoreError::NeedMore)
            };
        };

        // 14-bit container frames are not reconstructed here (the bytes
        // would need unpacking to the 16-bit-word domain first); surface
        // the same Unsupported gap as before for those.
        let Some(bytes) = self.last_frame_bytes.take() else {
            return Err(CoreError::unsupported(
                "oxideav-dts: 14-bit container frame reconstruction is not \
                 wired; only raw 16-bit Core frames decode to PCM",
            ));
        };

        // Run the §5.3/§5.4/§5.5 + §C.2.5 reconstruction for the common
        // Core case through the persistent §C.2.5 stream decoder so the
        // per-channel filter tail carries across packets (a DTS
        // elementary stream's QMF filter is continuous; resetting it per
        // packet injects a warmup transient at every frame boundary).
        // The frame's channel count (§5.3.2 nPCHS) sizes the filter; a
        // mismatch with the running stream restarts it for the new
        // layout. Joint-intensity frames (Table 5-28 JOINX > 0) now
        // decode through the §D.3 JScaleTbl + §C.2.3 sub-band copy; only
        // a §D.10 VQ/ADPCM blocker still surfaces as Unsupported so
        // callers can distinguish "needs another packet" from "this
        // frame's feature isn't decoded yet".
        let channels = frame_channel_count(&bytes, &header)
            .map_err(|e| CoreError::unsupported(format!("oxideav-dts: {e}")))?;
        let stream = match self.stream.take() {
            Some(s) if s.channel_count() == channels => s,
            // First packet, or a channel-count change: (re)start the
            // continuous filter for this layout.
            _ => crate::CoreStreamDecoder::new(channels),
        };
        let mut stream = stream;
        let mut pcm = stream
            .decode_frame(&bytes, &header)
            .map_err(|e| CoreError::unsupported(format!("oxideav-dts: {e}")))?;
        // For an LFE-bearing frame (LFF != 0), append the §5.5/§C.2.6 LFE
        // channel as a trailing plane. The §C.2.6 interpolation expands
        // the decimated LFE samples to exactly the primary per-frame
        // sample rate (2·LFF·nSSC·nDeciFactor == nSSC·256 for both LFF
        // modes), so the LFE plane is the same length as the primary
        // planes and slots in as one more channel.
        let lfe = stream.take_last_lfe_pcm();
        if !lfe.is_empty() {
            pcm.push(lfe);
        }
        self.stream = Some(stream);

        // Planar S32: one plane per channel, each sample little-endian.
        let channels = pcm.len();
        let samples_per_channel = pcm.first().map_or(0, Vec::len) as u32;
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(channels);
        for plane in &pcm {
            let mut bytes = Vec::with_capacity(plane.len() * 4);
            for &s in plane {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            data.push(bytes);
        }

        Ok(Frame::Audio(AudioFrame {
            samples: samples_per_channel,
            pts: Some(self.last_pts),
            data,
        }))
    }

    fn flush(&mut self) -> CoreResult<()> {
        self.eof = true;
        Ok(())
    }

    fn reset(&mut self) -> CoreResult<()> {
        self.last_header = None;
        self.last_frame_bytes = None;
        self.last_pts = 0;
        // Drop the persistent §C.2.5 stream decoder so the next packet
        // starts a fresh continuous filter (cleared history) — a reset
        // means the caller seeked / restarted, so the inter-frame filter
        // tail must not bleed across the discontinuity.
        self.stream = None;
        self.eof = false;
        Ok(())
    }
}

/// Read the §5.3.2 Primary Audio Coding Header just enough to recover
/// the frame's primary-channel count (`nPCHS`), which sizes the
/// persistent §C.2.5 stream filter. Returns the §5.3.2 audio-coding
/// header decode error on a structurally-bad frame.
fn frame_channel_count(bytes: &[u8], header: &DtsFrameHeader) -> Result<usize, DtsError> {
    let header_bits = header.header_bit_length() as usize;
    let (coding, _ach_bits) =
        crate::decode_audio_coding_header_at(bytes, header_bits, header.crc_present)?;
    Ok(coding.n_pchs)
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
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
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
            eof: false,
        };
        handle.send_packet(&pkt).unwrap();
        assert!(handle.last_header().is_some());
        handle.reset().unwrap();
        assert!(handle.last_header().is_none());
        assert!(!handle.eof);
    }

    /// Pack `(value, width)` fields MSB-first.
    fn pack_fields(fields: &[(u32, u8)]) -> Vec<u8> {
        let total_bits: usize = fields.iter().map(|(_, w)| *w as usize).sum();
        let mut out = vec![0u8; total_bits.div_ceil(8)];
        let mut bit_pos = 0usize;
        for &(value, width) in fields {
            for i in (0..width).rev() {
                let bit = ((value >> i) & 1) as u8;
                out[bit_pos / 8] |= bit << (7 - (bit_pos % 8));
                bit_pos += 1;
            }
        }
        out
    }

    /// A complete raw-16-bit Core frame (clean header + a one-channel
    /// all-`ABITS==0` body) decodes end to end through the registry
    /// `Decoder`, emitting a planar S32 `AudioFrame` of the right shape
    /// instead of the historical `Unsupported`.
    #[test]
    fn receive_frame_decodes_common_core_case_to_audio() {
        // Clean header: parse the fixture, clear DYNF/CPF/ASPF, re-encode.
        let mut header = parse_frame_header(&REAL_DTS_FRAME_HEADER).unwrap();
        header.dynamic_range = false;
        header.predictor_history = false;
        header.aspf = false;
        let mut bytes = crate::encode_frame_header_be(&header).unwrap();

        // One-channel ACH (nSUBS=2, nVQSUB=2, BHUFF=6 Linear5Bit), then
        // a one-subframe all-ABITS-0 side info, then a single DSYNC.
        let mut body: Vec<(u32, u8)> = vec![
            (0, 4),
            (0, 3),
            (0, 5),
            (1, 5),
            (0, 3),
            (0, 2),
            (0, 3),
            (6, 3),
        ];
        body.push((0, 1));
        for _ in 1..5 {
            body.push((0, 2));
        }
        for _ in 5..10 {
            body.push((0, 3));
        }
        for _ in 0..10 {
            body.push((0, 2));
        }
        body.push((0, 2)); // SSC
        body.push((0, 3)); // PSC
        body.push((0, 1)); // PMODE[0][0]
        body.push((0, 1)); // PMODE[0][1]
        body.push((0, 5)); // ABITS[0][0]
        body.push((0, 5)); // ABITS[0][1]
        body.push((0xffff, 16)); // DSYNC
        bytes.extend_from_slice(&pack_fields(&body));
        bytes.extend_from_slice(&[0u8; 4]);

        let pkt = Packet::new(0, TimeBase::new(1, 48_000), bytes);
        let mut handle = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
            eof: false,
        };
        handle.send_packet(&pkt).unwrap();
        let frame = handle
            .receive_frame()
            .expect("common Core case must decode");
        match frame {
            Frame::Audio(a) => {
                // One channel, one subframe, one subsubframe -> 256 samples.
                assert_eq!(a.data.len(), 1);
                assert_eq!(a.samples, 256);
                assert_eq!(a.data[0].len(), 256 * 4); // S32 planar
                assert!(a.data[0].iter().all(|&b| b == 0)); // all-zero PCM
            }
            other => panic!("expected an audio frame, got {other:?}"),
        }
    }

    /// The bundled 5-frame `ffmpeg -c:a dca` fixture (real 48 kHz stereo
    /// DTS Core). Each 1024-byte frame is a self-delimited raw-16-bit
    /// Core frame.
    const FIXTURE_5_FRAMES: &[u8] = include_bytes!("../tests/fixtures/dts_5_frames.bin");

    /// Driving two consecutive real Core frames through the registry
    /// `Decoder` emits two 512-sample stereo S32 frames, and the handle's
    /// persistent §C.2.5 stream filter carries the inter-frame tail: the
    /// second frame's PCM differs from what a freshly-reset decoder would
    /// produce for that same frame in isolation. This is the registry-
    /// level expression of the round-356 CoreStreamDecoder continuity
    /// fix.
    #[test]
    fn registry_decoder_carries_filter_state_across_real_packets() {
        // Frame 0 and frame 1 are the first two 1024-byte frames.
        let f0 = &FIXTURE_5_FRAMES[0..1024];
        let f1 = &FIXTURE_5_FRAMES[1024..2048];

        let decode_two = || {
            let mut handle = DtsDecoderHandle {
                codec_id: CodecId::new(CODEC_ID_STR),
                last_header: None,
                last_frame_bytes: None,
                last_pts: 0,
                stream: None,
                eof: false,
            };
            let tb = TimeBase::new(1, 48_000);
            handle
                .send_packet(&Packet::new(0, tb, f0.to_vec()))
                .unwrap();
            let a0 = match handle.receive_frame().unwrap() {
                Frame::Audio(a) => a,
                other => panic!("expected audio, got {other:?}"),
            };
            handle
                .send_packet(&Packet::new(1, tb, f1.to_vec()))
                .unwrap();
            let a1 = match handle.receive_frame().unwrap() {
                Frame::Audio(a) => a,
                other => panic!("expected audio, got {other:?}"),
            };
            (a0, a1)
        };

        let (a0, a1) = decode_two();
        // Real 48 kHz stereo: 16 blocks * 32 samples = 512 samples/ch.
        assert_eq!(a0.data.len(), 2);
        assert_eq!(a0.samples, 512);
        assert_eq!(a0.data[0].len(), 512 * 4);
        assert_eq!(a1.samples, 512);
        // Both frames carry real (non-silent) audio.
        assert!(a1.data[0].iter().any(|&b| b != 0));

        // Decode frame 1 in isolation with a fresh handle (no carried
        // tail). Its PCM must differ from the streamed frame 1 — the
        // persistent filter bled frame 0's tail into the streamed result.
        let mut fresh = DtsDecoderHandle {
            codec_id: CodecId::new(CODEC_ID_STR),
            last_header: None,
            last_frame_bytes: None,
            last_pts: 0,
            stream: None,
            eof: false,
        };
        fresh
            .send_packet(&Packet::new(1, TimeBase::new(1, 48_000), f1.to_vec()))
            .unwrap();
        let isolated1 = match fresh.receive_frame().unwrap() {
            Frame::Audio(a) => a,
            other => panic!("expected audio, got {other:?}"),
        };
        assert_ne!(
            a1.data, isolated1.data,
            "streamed frame 1 must carry frame 0's §C.2.5 filter tail, \
             differing from an isolated decode of frame 1"
        );
    }
}
