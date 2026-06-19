//! DTS Coherent Acoustics — §5.5 + §C.2.5 end-to-end subframe→PCM
//! bridge (ETSI TS 102 114 V1.3.1).
//!
//! Round 346 (2026-06-20) composes the two already-landed halves of the
//! Core reconstruction chain into one per-subframe call:
//!
//! 1. the round-340 §5.5 [`decode_audio_data_subframe_at`] walk, which
//!    turns the §5.4.1 side information + the §5.5 `Audio Data` arrays
//!    into the per-channel subband-sample matrices
//!    `aPrmCh[ch].aSubband[n].aSample[m]`, and
//! 2. the round-330 §C.2.5 [`MultiChannelQmf`] driver, which runs the
//!    per-channel `aPrmCh[ch].QMFInterpolation(FILTS, nSUBS[ch])` 32-band
//!    synthesis filterbank over those matrices to produce PCM.
//!
//! The bridge is the missing composition step the crate README's "Not
//! yet implemented" tail named first: *"The §5.5 `Audio Data` walker
//! that composes the side-info, dispatch, dequantization, ADPCM, and QMF
//! primitives into reconstructed subband samples — and thus PCM
//! output."* The walker (#1) and the synthesis (#2) both landed in
//! prior rounds; this module is the one-call subframe driver that wires
//! the walker's output directly into the synthesis input.
//!
//! # The per-subframe loop (§5.4 + §5.5 + §C.2.5)
//!
//! For one audio subframe the spec runs (PDF p.28-33, then the §C.2.5
//! driver per channel):
//!
//! ```text
//! // §5.5 Audio Data: nSSC subsubframes of 8 samples each ->
//! //   aPrmCh[ch].aSubband[n].aSample[0 .. nSSC*8]
//! decode_audio_data_subframe_at(...);
//! // §C.2.5 Filter Bank Reconstruction, once per channel:
//! for (ch=0; ch<nPCHS; ch++)
//!     aPrmCh[ch].QMFInterpolation(FILTS, nSUBS[ch]);
//! ```
//!
//! Each channel's `nSSC*8` per-sample subband rows synthesise to
//! `nSSC*8*32` PCM samples (the §C.2.5 driver emits 32 PCM samples per
//! subband-sample row). A subframe therefore yields `nSSC * 256` PCM
//! samples per channel.
//!
//! # Persistence across subframes
//!
//! [`SubframePcmDecoder`] owns one persistent [`MultiChannelQmf`] so a
//! caller decoding a frame's subframes (or a stream's frames) in order
//! carries each channel's inter-subframe filter tail (`raX[]` / `raZ[]`)
//! exactly as the §C.2.5 driver requires. Construct it once for the
//! frame's channel count, then call [`SubframePcmDecoder::decode_subframe`]
//! for each subframe.
//!
//! # Scope
//!
//! The walker's §D.10.1 ADPCM-coefficient-VQ (`PMODE != 0`) and §D.10.2
//! high-frequency-VQ (`nVQSUB < nSUBS`) blockers are still surfaced as
//! typed [`AudioArrayError::VqCodebookUnavailable`] errors (those Annex D
//! VQ code books are not transcribed in `docs/audio/dts/`). A subframe
//! whose primary channels are all linearly / Huffman / block coded with
//! `PMODE == 0` and `nVQSUB == nSUBS` — the common Core case — now
//! reconstructs to PCM end-to-end through this one call.
//!
//! Joint-intensity subband coding (`JOINX[ch] > 0`) is not applied here:
//! the §C.2.3 joint-subband decode is landed
//! ([`crate::joint_subband_decode_range_f64`]) but it needs the
//! `JOIN_SCALES[ch][n]` Huffman factors, whose §5.4.x bit-stream decode
//! is not yet wired. [`SubframePcmDecoder::decode_subframe`] therefore
//! surfaces [`SubframePcmError::JointSubbandUnsupported`] when any
//! channel carries `JOINX[ch] > 0`, rather than silently skipping the
//! joint step.

use crate::audio_array::{
    decode_audio_data_subframe_at, AudioArrayDecodeError, SubbandSampleMatrix,
};
use crate::audio_header::AudioCodingHeader;
use crate::cos_mod::NUM_SUBBAND;
use crate::filter_bank::FilterBankSelection;
use crate::header::DtsFrameHeader;
use crate::qmf_multichannel::{MultiChannelQmf, MultiChannelQmfError};
use crate::step_size::StepSizeTable;
use crate::subframe::ChannelSideInfo;

/// One subframe's reconstructed PCM, planar (one `Vec<i32>` per
/// channel). Every channel's vec has the same length — `nSSC * 256`
/// samples (`nSSC` subsubframes × 8 samples × 32 PCM samples per
/// subband-sample row).
pub type SubframePcm = Vec<Vec<i32>>;

/// PCM samples one §C.2.5 subband-sample row expands to (the driver
/// emits 32 PCM samples per row — the `NumSubband` bands of one
/// polyphase output block).
pub const PCM_PER_SUBBAND_ROW: usize = NUM_SUBBAND;

/// Errors from the §5.5 + §C.2.5 end-to-end subframe→PCM bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubframePcmError {
    /// The §5.5 [`decode_audio_data_subframe_at`] walk failed: a
    /// bit-stream-level error, or an Annex D VQ-codebook blocker
    /// (`PMODE != 0` / `nVQSUB < nSUBS`). Carries the underlying
    /// [`AudioArrayDecodeError`].
    AudioData(AudioArrayDecodeError),
    /// The §C.2.5 [`MultiChannelQmf`] synthesis failed (a length or
    /// row-count mismatch between the walker's matrices and the driver's
    /// channel count, or a per-channel synthesis error).
    Synthesis(MultiChannelQmfError),
    /// A channel carried `JOINX[ch] > 0` (joint-intensity subband
    /// coding). The §C.2.3 joint-subband decode is landed but its
    /// `JOIN_SCALES` Huffman side-info decode is not yet wired, so the
    /// bridge declines rather than producing incorrect PCM. Carries the
    /// 0-based channel index and the one-based `JOINX` source.
    JointSubbandUnsupported {
        /// 0-based destination channel carrying `JOINX > 0`.
        ch: usize,
        /// The one-based `JOINX[ch]` source-channel selector.
        joinx: u8,
    },
    /// The frame header's `PCMR` source-PCM-resolution code is one of
    /// the two reserved values, so the §C.2.5 output `rScale` (the
    /// post-filterbank float→PCM full-scale gain derived from `PCMR`) is
    /// undefined and no PCM can be produced. Carries the raw `PCMR`
    /// index.
    ReservedPcmResolution {
        /// The raw §5.3.1 Table 5-17 `PCMR` index.
        pcmr: u8,
    },
    /// The caller-supplied per-channel side-info / loop-bound slices did
    /// not all agree on the channel count. Carries the channel count the
    /// driver expected (the [`SubframePcmDecoder`]'s configured count)
    /// and the mismatching slice length.
    ChannelCountMismatch {
        /// The driver's configured channel count.
        expected: usize,
        /// The mismatching supplied slice length.
        got: usize,
    },
}

impl core::fmt::Display for SubframePcmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SubframePcmError::AudioData(e) => write!(f, "audio-data walk failed: {e}"),
            SubframePcmError::Synthesis(e) => write!(f, "QMF synthesis failed: {e}"),
            SubframePcmError::JointSubbandUnsupported { ch, joinx } => write!(
                f,
                "channel {ch} carries JOINX={joinx} (joint-intensity subband \
                 coding); the JOIN_SCALES side-info decode is not yet wired"
            ),
            SubframePcmError::ReservedPcmResolution { pcmr } => write!(
                f,
                "frame header PCMR index {pcmr} is reserved; the output rScale \
                 is undefined"
            ),
            SubframePcmError::ChannelCountMismatch { expected, got } => write!(
                f,
                "channel-count mismatch: driver expects {expected}, slice carries {got}"
            ),
        }
    }
}

impl std::error::Error for SubframePcmError {}

impl From<AudioArrayDecodeError> for SubframePcmError {
    fn from(e: AudioArrayDecodeError) -> Self {
        SubframePcmError::AudioData(e)
    }
}

impl From<MultiChannelQmfError> for SubframePcmError {
    fn from(e: MultiChannelQmfError) -> Self {
        SubframePcmError::Synthesis(e)
    }
}

/// Persistent per-frame §5.5 + §C.2.5 subframe→PCM decoder.
///
/// Owns one [`MultiChannelQmf`] for the frame's channel count, so the
/// per-channel filter state (`raX[]` / `raZ[]`) carries across
/// subframes (and across frames if the same decoder instance is reused
/// for a stream). Construct once with the channel count from the frame
/// header, then call [`SubframePcmDecoder::decode_subframe`] for each of
/// the `nSUBFS` subframes the §5.3.2 header declares.
#[derive(Debug, Clone)]
pub struct SubframePcmDecoder {
    qmf: MultiChannelQmf,
}

impl SubframePcmDecoder {
    /// Construct a decoder for `channels` primary audio channels — the
    /// §5.3.2 `nPCHS` (e.g. [`AudioCodingHeader::n_pchs`]). Each
    /// channel's §C.2.5 filter starts with cleared history.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            qmf: MultiChannelQmf::new(channels),
        }
    }

    /// The configured channel count (`nPCHS`).
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.qmf.channel_count()
    }

    /// Borrow the persistent §C.2.5 driver (e.g. to inspect a channel's
    /// inter-subframe filter tail).
    #[must_use]
    pub fn qmf(&self) -> &MultiChannelQmf {
        &self.qmf
    }

    /// Decode one §5.4/§5.5 audio subframe to planar PCM, end to end.
    ///
    /// Runs the §5.5 [`decode_audio_data_subframe_at`] walk to get the
    /// per-channel subband-sample matrices, then the §C.2.5
    /// [`MultiChannelQmf`] synthesis to turn them into PCM. The
    /// per-channel filter state persists into the next call.
    ///
    /// * `bytes` / `bit_offset` — the bit stream positioned at the first
    ///   §5.5 `Audio Data` bit of this subframe (after the subframe's
    ///   §5.4.1 side information).
    /// * `header` — the parsed §5.3.1 [`DtsFrameHeader`]; supplies the
    ///   frame-wide `FILTS` ([`DtsFrameHeader::filter_bank_selection`])
    ///   and the output `rScale` ([`DtsFrameHeader::output_r_scale`]).
    /// * `coding` — the §5.3.2 [`AudioCodingHeader`]; supplies the
    ///   `SEL` / `arADJ` planes and the per-channel `nSUBS` / `nVQSUB`
    ///   loop bounds and `JOINX`.
    /// * `side` — the per-channel decoded §5.4.1 [`ChannelSideInfo`].
    /// * `n_ssc` — this subframe's subsubframe count (`SSC + 1`).
    /// * `aspf` — the §5.3.1 Audio Sync-Word Insertion Flag.
    ///
    /// Returns `(SubframePcm, bits_consumed)`: planar PCM (one
    /// `Vec<i32>` per channel, `n_ssc * 256` samples each) plus the
    /// number of §5.5 bits the audio-data walk consumed (so the caller
    /// can advance to the next subframe).
    ///
    /// # Errors
    ///
    /// * [`SubframePcmError::ChannelCountMismatch`] if `side`'s length
    ///   differs from the configured channel count;
    /// * [`SubframePcmError::JointSubbandUnsupported`] if any channel
    ///   carries `JOINX[ch] > 0`;
    /// * [`SubframePcmError::ReservedPcmResolution`] if the header's
    ///   `PCMR` code is reserved;
    /// * [`SubframePcmError::AudioData`] for any §5.5 walk failure
    ///   (including the §D.10 VQ blockers);
    /// * [`SubframePcmError::Synthesis`] for any §C.2.5 driver failure.
    #[allow(clippy::too_many_arguments)]
    pub fn decode_subframe(
        &mut self,
        bytes: &[u8],
        bit_offset: usize,
        header: &DtsFrameHeader,
        coding: &AudioCodingHeader,
        side: &[ChannelSideInfo],
        n_ssc: usize,
        aspf: bool,
    ) -> Result<(SubframePcm, usize), SubframePcmError> {
        let channels = self.qmf.channel_count();
        if side.len() != channels {
            return Err(SubframePcmError::ChannelCountMismatch {
                expected: channels,
                got: side.len(),
            });
        }
        if coding.n_pchs != channels {
            return Err(SubframePcmError::ChannelCountMismatch {
                expected: channels,
                got: coding.n_pchs,
            });
        }

        // The §C.2.5 output rScale must be defined (PCMR not reserved)
        // before any decode work runs, so a reserved-PCMR frame fails
        // cleanly without disturbing the persistent filter state.
        let Some(r_scale) = header.output_r_scale() else {
            return Err(SubframePcmError::ReservedPcmResolution {
                pcmr: header.source_pcm_resolution_index,
            });
        };
        let filter: FilterBankSelection = header.filter_bank_selection();

        // Joint-intensity subband coding (JOINX > 0) needs the
        // JOIN_SCALES side-info that is not yet wired; decline rather
        // than emit incorrect PCM.
        for (ch, &joinx) in coding.joinx.iter().enumerate().take(channels) {
            if joinx > 0 {
                return Err(SubframePcmError::JointSubbandUnsupported { ch, joinx });
            }
        }

        // Per-channel loop bounds for the §5.5 walk and the §C.2.5
        // driver come straight off the §5.3.2 header.
        let n_subs = coding.n_subs();
        let n_vqsub = coding.n_vqsub();

        let table = StepSizeTable::for_rate(header.rate_index);

        // (1) §5.5 Audio Data -> per-channel subband-sample matrices.
        let (matrices, bits_consumed): (Vec<SubbandSampleMatrix>, usize) =
            decode_audio_data_subframe_at(
                bytes,
                bit_offset,
                side,
                |ch, abits| coding.sel(ch, abits),
                |ch, abits| coding.adj(ch, abits),
                &n_vqsub,
                &n_subs,
                n_ssc,
                table,
                aspf,
            )?;

        // (2) §C.2.5 per-channel 32-band synthesis -> planar PCM.
        let channel_samples: Vec<&[[f64; NUM_SUBBAND]]> =
            matrices.iter().map(|m| m.as_slice()).collect();
        let mut pcm: SubframePcm = vec![Vec::new(); channels];
        self.qmf
            .synthesize_planar(&channel_samples, &n_subs, filter, r_scale, &mut pcm)?;

        Ok((pcm, bits_consumed))
    }

    /// Decode all `nSUBFS` subframes of one core frame to a single block
    /// of planar PCM, appending each subframe's output (in order) onto
    /// the per-channel vectors so the persistent §C.2.5 filter tail
    /// carries across subframe boundaries (§5.3.2 `nSUBFS`; §C.2.5
    /// per-channel filter continuity).
    ///
    /// `bytes` is the frame's bit-stream buffer; `first_audio_bit` is the
    /// bit offset of the **first** subframe's §5.5 `Audio Data` region
    /// (the cursor a caller is left at after the first subframe's §5.4.1
    /// side info). Each [`Subframe`] supplies that subframe's already-
    /// decoded §5.4.1 [`ChannelSideInfo`], its `n_ssc`, and the byte gap
    /// (`side_info_bits`) the caller must skip between this subframe's
    /// §5.5 region and the next subframe's §5.5 region — i.e. the bits of
    /// the *next* subframe's side info, which this driver does not itself
    /// decode (that §5.4.x region — `JOIN_SHUFF` onward — is not yet
    /// transcribed). The last subframe's `side_info_bits` is ignored.
    ///
    /// Returns the concatenated planar PCM (one `Vec<i32>` per channel,
    /// `Σ nSSC · 256` samples each) plus the total bits consumed from
    /// `first_audio_bit`.
    ///
    /// # Errors
    ///
    /// The same errors as [`SubframePcmDecoder::decode_subframe`], plus
    /// [`SubframePcmError::ChannelCountMismatch`] if a subframe's
    /// side-info channel count disagrees with the driver. A failure on
    /// the *k*-th subframe leaves the PCM from subframes `0..k` already
    /// appended (the §C.2.5 filter state is likewise advanced through
    /// `k-1`); callers that need all-or-nothing semantics should clone
    /// the decoder first.
    pub fn decode_frame(
        &mut self,
        bytes: &[u8],
        first_audio_bit: usize,
        header: &DtsFrameHeader,
        coding: &AudioCodingHeader,
        subframes: &[Subframe<'_>],
        aspf: bool,
    ) -> Result<(SubframePcm, usize), SubframePcmError> {
        let channels = self.qmf.channel_count();
        let mut pcm: SubframePcm = vec![Vec::new(); channels];
        let mut bit = first_audio_bit;

        for (k, sf) in subframes.iter().enumerate() {
            let (block, audio_bits) =
                self.decode_subframe(bytes, bit, header, coding, sf.side, sf.n_ssc, aspf)?;
            for (ch, samples) in block.into_iter().enumerate() {
                pcm[ch].extend(samples);
            }
            bit += audio_bits;
            // Skip the next subframe's §5.4.1 side-info region (the bits
            // the caller pre-measured); the last subframe has no
            // successor side info to skip.
            if k + 1 < subframes.len() {
                bit += sf.side_info_bits;
            }
        }

        Ok((pcm, bit - first_audio_bit))
    }
}

/// One subframe's already-decoded inputs for
/// [`SubframePcmDecoder::decode_frame`].
///
/// The driver decodes each subframe's §5.5 `Audio Data` region from the
/// shared bit-stream buffer; this struct carries the per-subframe §5.4.1
/// side information the audio-data walk needs plus the framing offsets
/// the driver uses to step from one subframe's §5.5 region to the next.
#[derive(Debug, Clone, Copy)]
pub struct Subframe<'a> {
    /// This subframe's decoded §5.4.1 per-channel side information (the
    /// round-281 [`crate::decode_primary_side_info_at`] output).
    pub side: &'a [ChannelSideInfo],
    /// This subframe's subsubframe count `nSSC = SSC + 1` (§5.4.1).
    pub n_ssc: usize,
    /// The bit length of the **next** subframe's §5.4.1 side-info region
    /// — the gap the driver skips after this subframe's §5.5 region to
    /// reach the next subframe's §5.5 region. Ignored for the last
    /// subframe of the frame.
    pub side_info_bits: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_info::ScaleFactorAdjustment;
    use crate::step_size::SAMPLES_PER_SUBSUBFRAME;
    use crate::subframe::ChannelSideInfo;

    /// Pack a list of `(value, width)` MSB-first into bytes.
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

    /// A single-channel single-subsubframe NFE subframe reconstructs to
    /// PCM end to end, and the PCM equals running the §5.5 walk + the
    /// §C.2.5 driver by hand.
    #[test]
    fn nfe_subframe_round_trips_to_pcm() {
        // ABITS 8 -> NFE width 5; SEL 7 selects the terminal NFE entry.
        let mut ch = ChannelSideInfo::cleared();
        ch.abits[0] = 8;
        ch.scales[0][0] = 4;
        let side = vec![ch];

        let vals = [3i32, -3, 5, -5, 7, -7, 2, -2];
        let mut fields: Vec<(u32, u8)> = vals.iter().map(|&v| ((v as u32) & 0x1f, 5u8)).collect();
        fields.push((0xffff, 16)); // DSYNC
        let stream = pack_fields(&fields);

        // Build a parsed header carrying FILTS / PCMR via the public
        // parser: reuse the registry test fixture's real BE header
        // (PCMR index 0 -> 16-bit -> rScale 32768, FILTS = 0).
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let header = crate::parse_frame_header(&hdr_bytes).unwrap();
        assert_eq!(header.output_r_scale(), Some(32768.0));

        // A one-channel AudioCodingHeader with nSUBS=nVQSUB=1, JOINX=0,
        // SEL[ch][ABITS 8-1] = 7 (terminal NFE). Build it through the
        // public test constructor.
        let coding = AudioCodingHeader::single_channel_for_test(1, 1, 7);

        let mut dec = SubframePcmDecoder::new(1);
        let (pcm, bits) = dec
            .decode_subframe(&stream, 0, &header, &coding, &side, 1, false)
            .unwrap();

        // Reference: walk + driver by hand.
        let table = StepSizeTable::for_rate(header.rate_index);
        let (mats, ref_bits) = decode_audio_data_subframe_at(
            &stream,
            0,
            &side,
            |_, _| 7,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            1,
            table,
            false,
        )
        .unwrap();
        let refs: Vec<&[[f64; NUM_SUBBAND]]> = mats.iter().map(|m| m.as_slice()).collect();
        let mut mc = MultiChannelQmf::new(1);
        let mut expect = vec![Vec::new(); 1];
        mc.synthesize_planar(
            &refs,
            &[1],
            header.filter_bank_selection(),
            32768.0,
            &mut expect,
        )
        .unwrap();

        assert_eq!(bits, ref_bits);
        assert_eq!(pcm, expect);
        // One subsubframe of 8 rows -> 8 * 32 = 256 PCM samples.
        assert_eq!(pcm[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(pcm[0].iter().any(|&s| s != 0));
    }

    /// A reserved PCMR code fails cleanly without disturbing the filter
    /// state.
    #[test]
    fn reserved_pcmr_declines() {
        // PCMR index 4 (0b100) is one of the reserved codes -> rScale
        // None. Construct a header with that PCMR via the test setter.
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let mut header = crate::parse_frame_header(&hdr_bytes).unwrap();
        header.source_pcm_resolution_index = 4; // reserved
        assert_eq!(header.output_r_scale(), None);

        let side = vec![ChannelSideInfo::cleared()];
        let coding = AudioCodingHeader::single_channel_for_test(1, 1, 0);
        let mut dec = SubframePcmDecoder::new(1);
        let err = dec
            .decode_subframe(&[0u8; 4], 0, &header, &coding, &side, 1, false)
            .unwrap_err();
        assert!(matches!(
            err,
            SubframePcmError::ReservedPcmResolution { pcmr: 4 }
        ));
        // Filter untouched.
        assert!(dec
            .qmf()
            .channels()
            .iter()
            .all(|q| q.x_history().iter().all(|&v| v == 0.0)));
    }

    /// A JOINX > 0 channel is declined.
    #[test]
    fn joint_subband_declined() {
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let header = crate::parse_frame_header(&hdr_bytes).unwrap();
        let side = vec![ChannelSideInfo::cleared()];
        let mut coding = AudioCodingHeader::single_channel_for_test(1, 1, 0);
        coding.set_joinx_for_test(0, 2);
        let mut dec = SubframePcmDecoder::new(1);
        let err = dec
            .decode_subframe(&[0u8; 4], 0, &header, &coding, &side, 1, false)
            .unwrap_err();
        assert!(matches!(
            err,
            SubframePcmError::JointSubbandUnsupported { ch: 0, joinx: 2 }
        ));
    }

    /// A channel-count mismatch between the decoder and the side-info
    /// slice is rejected before any decode.
    #[test]
    fn channel_count_mismatch_rejected() {
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let header = crate::parse_frame_header(&hdr_bytes).unwrap();
        let side = vec![ChannelSideInfo::cleared(), ChannelSideInfo::cleared()];
        let coding = AudioCodingHeader::single_channel_for_test(1, 1, 0);
        let mut dec = SubframePcmDecoder::new(1);
        let err = dec
            .decode_subframe(&[0u8; 4], 0, &header, &coding, &side, 1, false)
            .unwrap_err();
        assert!(matches!(
            err,
            SubframePcmError::ChannelCountMismatch {
                expected: 1,
                got: 2
            }
        ));
    }

    /// A no-bits subframe yields all-zero PCM of the right length.
    #[test]
    fn no_bits_subframe_zero_pcm() {
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let header = crate::parse_frame_header(&hdr_bytes).unwrap();
        let side = vec![ChannelSideInfo::cleared()]; // ABITS all 0
        let coding = AudioCodingHeader::single_channel_for_test(1, 1, 0);
        // nSSC = 2 -> two DSYNC trailers (last subsubframe only; ASPF
        // false means only the final one).
        let stream = pack_fields(&[(0xffff, 16)]);
        let mut dec = SubframePcmDecoder::new(1);
        let (pcm, _) = dec
            .decode_subframe(&stream, 0, &header, &coding, &side, 1, false)
            .unwrap();
        assert_eq!(pcm.len(), 1);
        assert_eq!(pcm[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(pcm[0].iter().all(|&s| s == 0));
    }

    /// `decode_frame` over two NFE subframes equals running
    /// `decode_subframe` twice on the same persistent decoder — the
    /// §C.2.5 filter tail carries across the subframe boundary and the
    /// PCM is concatenated in order.
    #[test]
    fn decode_frame_concatenates_and_carries_filter_state() {
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let header = crate::parse_frame_header(&hdr_bytes).unwrap();
        let coding = AudioCodingHeader::single_channel_for_test(1, 1, 7);

        // Two subframes, each: 8 NFE 5-bit values + a 16-bit DSYNC. The
        // second subframe's §5.5 region directly follows the first (no
        // inter-subframe side info in this synthetic stream, so
        // side_info_bits = 0).
        let mut ch = ChannelSideInfo::cleared();
        ch.abits[0] = 8;
        ch.scales[0][0] = 4;
        let side = vec![ch];

        let mk_sf = |base: i32| -> Vec<(u32, u8)> {
            let mut f: Vec<(u32, u8)> = (0..8).map(|i| (((base + i) as u32) & 0x1f, 5u8)).collect();
            f.push((0xffff, 16));
            f
        };
        let mut fields = mk_sf(1);
        let sf0_bits: usize = fields.iter().map(|(_, w)| *w as usize).sum();
        fields.extend(mk_sf(-4));
        let stream = pack_fields(&fields);

        let subframes = [
            Subframe {
                side: &side,
                n_ssc: 1,
                side_info_bits: 0, // next subframe's §5.5 immediately follows
            },
            Subframe {
                side: &side,
                n_ssc: 1,
                side_info_bits: 0,
            },
        ];

        let mut frame_dec = SubframePcmDecoder::new(1);
        let (frame_pcm, frame_bits) = frame_dec
            .decode_frame(&stream, 0, &header, &coding, &subframes, false)
            .unwrap();

        // Reference: two decode_subframe calls on one persistent decoder.
        let mut seq_dec = SubframePcmDecoder::new(1);
        let (b0, n0) = seq_dec
            .decode_subframe(&stream, 0, &header, &coding, &side, 1, false)
            .unwrap();
        let (b1, n1) = seq_dec
            .decode_subframe(&stream, n0, &header, &coding, &side, 1, false)
            .unwrap();
        let mut expect = b0;
        for (ch, samples) in b1.into_iter().enumerate() {
            expect[ch].extend(samples);
        }

        assert_eq!(frame_pcm, expect);
        assert_eq!(frame_bits, n0 + n1);
        // Each subframe is one subsubframe -> 256 PCM samples; two -> 512.
        assert_eq!(
            frame_pcm[0].len(),
            2 * SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW
        );
        // Sanity: the first subframe's §5.5 region was sf0_bits long.
        assert_eq!(n0, sf0_bits);
        assert!(frame_pcm[0].iter().any(|&s| s != 0));
    }
}
