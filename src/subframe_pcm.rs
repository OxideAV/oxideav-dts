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
use crate::subframe::{ChannelSideInfo, SideInfoTail};

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

/// Why a Core frame could not be decoded straight from its bytes by
/// [`decode_core_frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreFrameDecodeError {
    /// The frame carries a Table 5-28 joint-intensity side-info tail
    /// this crate does not yet decode: some channel has `JOINX > 0`,
    /// so a variable-length `JOIN_SHUFF` / `JOIN_SCALES` block (gated on
    /// the unstaged joint-scale table) sits between a subframe's side
    /// info and its §5.5 `Audio Data` region, and the audio-data bit
    /// offset cannot be located. The `DYNF` (`RANGE`) and `CPF`
    /// (`SICRC`) tail fields are decoded (see [`decode_core_frame`]);
    /// only joint-intensity surfaces here.
    UnsupportedSideInfoTail {
        /// `DYNF != 0` — embedded dynamic-range `RANGE` field present.
        /// Retained for source compatibility; no longer a decline
        /// reason (the `RANGE` field is decoded and applied post-QMF).
        dynamic_range: bool,
        /// `CPF != 0` — a 16-bit `SICRC` side-info CRC trailer present.
        /// Retained for source compatibility; no longer a decline
        /// reason (the `SICRC` word is consumed for framing).
        side_info_crc: bool,
        /// Some channel carries `JOINX > 0` — a `JOIN_SHUFF`/`JOIN_SCALES`
        /// block present. This is the sole remaining decline reason.
        joint_intensity: bool,
    },
    /// A §5.3.2 / §5.4.1 / §5.5 decode step failed. Carries the
    /// underlying [`SubframePcmError`] (or a wrapped bit-stream
    /// [`crate::Error`] for the header/side-info walks).
    Decode(SubframePcmError),
    /// A structural bit-stream error in the §5.3.2 audio-coding-header or
    /// §5.4.1 side-info walk (EOF, reserved selector, …).
    Bitstream(crate::Error),
}

impl core::fmt::Display for CoreFrameDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CoreFrameDecodeError::UnsupportedSideInfoTail {
                dynamic_range,
                side_info_crc,
                joint_intensity,
            } => write!(
                f,
                "frame carries an undecoded §5.4.x side-info tail \
                 (DYNF={dynamic_range}, CPF/SICRC={side_info_crc}, \
                 JOINX>0={joint_intensity}); only the empty-tail common \
                 Core case is decoded to PCM"
            ),
            CoreFrameDecodeError::Decode(e) => write!(f, "{e}"),
            CoreFrameDecodeError::Bitstream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CoreFrameDecodeError {}

impl From<SubframePcmError> for CoreFrameDecodeError {
    fn from(e: SubframePcmError) -> Self {
        CoreFrameDecodeError::Decode(e)
    }
}

impl From<crate::Error> for CoreFrameDecodeError {
    fn from(e: crate::Error) -> Self {
        CoreFrameDecodeError::Bitstream(e)
    }
}

/// Decode one whole DTS Core frame to planar PCM straight from its
/// bytes, for the common Core case (§5.3 / §5.4 / §5.5 + §C.2.5).
///
/// This is the top-level orchestrator that chains the landed stages:
///
/// 1. the §5.3.2 [`crate::decode_audio_coding_header_at`] (Table 5-21)
///    from the bit just after the §5.3.1 frame header
///    ([`DtsFrameHeader::header_bit_length`]);
/// 2. for each of the `nSUBFS` subframes, the §5.4.1
///    [`crate::decode_primary_side_info_at`] (Table 5-28) walk, then the
///    §5.5 + §C.2.5 [`SubframePcmDecoder::decode_subframe`] reconstruction;
///    the per-channel §C.2.5 filter tail carries across subframes.
///
/// `header` is the already-parsed §5.3.1 frame header; `bytes` is the
/// frame's unpacked (16-bit-word-domain) bit-stream buffer.
///
/// # Scope
///
/// Every channel must have `JOINX == 0` (no joint-intensity sub-band
/// coding): a `JOINX > 0` channel carries a variable-length
/// `JOIN_SHUFF` / `JOIN_SCALES` Table 5-28 tail gated on the unstaged
/// joint-scale table, so the §5.5 bit offset cannot be located and the
/// function returns
/// [`CoreFrameDecodeError::UnsupportedSideInfoTail`] rather than guess.
///
/// The frame header's `DYNF` (embedded dynamic range) and `CPF`
/// (side-info CRC) tail fields **are** handled: each subframe's
/// [`crate::decode_primary_side_info_tail_at`] consumes the 8-bit
/// `RANGE` index (`DYNF != 0`) and the 16-bit `SICRC` word (`CPF == 1`),
/// and the §D.4 [`crate::drc_range`] multiplier is applied to that
/// subframe's reconstructed PCM after QMF synthesis (per §5.4.1).
///
/// The §D.10 VQ / ADPCM blockers (from the §5.5 walk) surface as
/// [`CoreFrameDecodeError::Decode`].
///
/// Returns planar PCM (one `Vec<i32>` per channel, `Σ nSSC · 256`
/// samples each).
///
/// # Errors
///
/// * [`CoreFrameDecodeError::UnsupportedSideInfoTail`] for a
///   joint-intensity (`JOINX > 0`) side-info tail;
/// * [`CoreFrameDecodeError::Bitstream`] for a §5.3.2 / §5.4.1 walk
///   failure;
/// * [`CoreFrameDecodeError::Decode`] for a §5.5 / §C.2.5 failure
///   (including the §D.10 VQ blockers and a reserved `PCMR`).
pub fn decode_core_frame(
    bytes: &[u8],
    header: &DtsFrameHeader,
) -> Result<SubframePcm, CoreFrameDecodeError> {
    // §5.3.2 Primary Audio Coding Header begins right after the §5.3.1
    // frame header; the channel count it declares sizes the per-channel
    // §C.2.5 filter bank. A fresh per-call decoder gives single-frame
    // semantics (cleared filter history) — for a multi-frame elementary
    // stream use [`CoreStreamDecoder`], which persists the per-channel
    // §C.2.5 filter tail across frame boundaries (the spec's filter is a
    // continuous per-channel object, not reset between frames).
    let header_bits = header.header_bit_length() as usize;
    let cpf = header.crc_present;
    let (coding, _ach_bits) = crate::decode_audio_coding_header_at(bytes, header_bits, cpf)?;
    let mut decoder = SubframePcmDecoder::new(coding.n_pchs);
    decoder.decode_core_frame_into(bytes, header)
}

/// Persistent §5.3/§5.4/§5.5 + §C.2.5 Core-stream decoder.
///
/// The §C.2.5 `aPrmCh[ch]` synthesis filter is a **continuous**
/// per-channel object whose 512-tap history (`raX[]`) and output
/// accumulator (`raZ[]`) carry across subframe **and frame**
/// boundaries of a contiguous elementary stream — the decoder does not
/// reset the filter at each frame. [`decode_core_frame`] (a fresh
/// per-call decoder) therefore reconstructs each frame as if it were
/// the first frame of a stream, which produces a filter-warmup
/// transient at every frame boundary instead of only the stream's true
/// start. For multi-frame decode use this type: construct it once for
/// the stream's channel count and feed every frame in order through
/// [`CoreStreamDecoder::decode_frame`], so each channel's inter-frame
/// filter tail carries correctly.
///
/// Validated against a black-box `ffmpeg -c:a dca` reference decode of
/// the bundled 5-frame fixture: carrying the filter state across frames
/// makes our channel-0 PCM **shape-identical** to the reference
/// (Pearson correlation 1.0 over the whole stream), versus 0.73 when
/// the filter is reset per frame. (The two differ only by the
/// implementation-defined output `rScale` constant — see
/// [`DtsFrameHeader::output_r_scale`] and the round-356 report.)
#[derive(Debug, Clone)]
pub struct CoreStreamDecoder {
    decoder: SubframePcmDecoder,
}

impl CoreStreamDecoder {
    /// Construct a stream decoder for `channels` primary audio channels
    /// (the §5.3.2 `nPCHS`). Every channel's §C.2.5 filter starts with a
    /// cleared history; that history then carries across every
    /// [`CoreStreamDecoder::decode_frame`] call.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            decoder: SubframePcmDecoder::new(channels),
        }
    }

    /// The configured channel count (`nPCHS`).
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.decoder.channel_count()
    }

    /// Borrow the persistent per-subframe decoder (e.g. to inspect a
    /// channel's inter-frame §C.2.5 filter tail via
    /// [`SubframePcmDecoder::qmf`]).
    #[must_use]
    pub fn subframe_decoder(&self) -> &SubframePcmDecoder {
        &self.decoder
    }

    /// Decode one whole Core frame to planar PCM, carrying the
    /// per-channel §C.2.5 filter tail into the next call.
    ///
    /// Identical reconstruction to [`decode_core_frame`] except the
    /// filter state is **not** reset: a frame's first output samples see
    /// the previous frame's filter tail, exactly as the §C.2.5
    /// continuous per-channel filter requires for a contiguous stream.
    ///
    /// `bytes` is one frame's bit-stream buffer; `header` its parsed
    /// §5.3.1 header. The frame's §5.3.2 audio-coding-header channel
    /// count must equal this decoder's configured channel count.
    ///
    /// # Errors
    ///
    /// The same errors as [`decode_core_frame`], plus
    /// [`CoreFrameDecodeError::Decode`] wrapping a
    /// [`SubframePcmError::ChannelCountMismatch`] if the frame's
    /// `nPCHS` disagrees with the configured channel count.
    pub fn decode_frame(
        &mut self,
        bytes: &[u8],
        header: &DtsFrameHeader,
    ) -> Result<SubframePcm, CoreFrameDecodeError> {
        self.decoder.decode_core_frame_into(bytes, header)
    }
}

impl SubframePcmDecoder {
    /// Decode one whole Core frame to planar PCM using this persistent
    /// decoder's per-channel §C.2.5 filter state (carried across calls).
    ///
    /// This is the per-frame body shared by [`decode_core_frame`] (which
    /// calls it on a fresh decoder, giving single-frame semantics) and
    /// [`CoreStreamDecoder::decode_frame`] (which calls it on a
    /// stream-lifetime decoder, carrying the inter-frame filter tail).
    ///
    /// # Errors
    ///
    /// See [`decode_core_frame`].
    pub fn decode_core_frame_into(
        &mut self,
        bytes: &[u8],
        header: &DtsFrameHeader,
    ) -> Result<SubframePcm, CoreFrameDecodeError> {
        // §5.3.2 Primary Audio Coding Header begins right after the
        // §5.3.1 frame header. The §5.3.1 CRC-present flag
        // (CPF == `crc_present`) controls the optional 16-bit SICRC
        // trailer of every subframe's §5.4.1 side info.
        let header_bits = header.header_bit_length() as usize;
        let cpf = header.crc_present;
        let (coding, ach_bits) = crate::decode_audio_coding_header_at(bytes, header_bits, cpf)?;

        // Joint-intensity (JOINX > 0) is the only side-info tail still
        // undecodable; DYNF / CPF are handled below.
        let joint_intensity = coding.joinx.iter().any(|&j| j > 0);
        if joint_intensity {
            return Err(CoreFrameDecodeError::UnsupportedSideInfoTail {
                dynamic_range: header.dynamic_range,
                side_info_crc: cpf,
                joint_intensity,
            });
        }

        let channels = coding.n_pchs;
        if channels != self.channel_count() {
            return Err(CoreFrameDecodeError::Decode(
                SubframePcmError::ChannelCountMismatch {
                    expected: self.channel_count(),
                    got: channels,
                },
            ));
        }
        let mut pcm: SubframePcm = vec![Vec::new(); channels];

        // The §5.4.1 side-info walk needs the per-channel
        // ChannelSideInfoParams.
        let params: Vec<_> = coding.channel_params.clone();

        let mut bit = header_bits + ach_bits;
        for _ in 0..coding.n_subframes {
            // §5.4.1 side info (Table 5-28) through the end of the
            // SCALES block.
            let (side, side_bits) = crate::decode_primary_side_info_at(bytes, bit, &params)?;
            bit += side_bits;

            // The Table 5-28 RANGE (DYNF) / SICRC (CPF) tail sits between
            // the SCALES block and the §5.5 region. (JOINX == 0 here, so
            // no JOIN_SHUFF / JOIN_SCALES bits.)
            let (tail, tail_bits): (SideInfoTail, usize) = crate::decode_primary_side_info_tail_at(
                bytes,
                bit,
                &coding.joinx,
                header.dynamic_range,
                cpf,
            )?;
            bit += tail_bits;

            let n_ssc = side.subsubframe_count.n_ssc() as usize;
            let (mut block, audio_bits) = self.decode_subframe(
                bytes,
                bit,
                header,
                &coding,
                &side.channels,
                n_ssc,
                header.aspf,
            )?;

            // §5.4.1: when DYNF != 0, multiply every reconstructed PCM
            // sample of this subframe by the §D.4 RANGE multiplier,
            // applied after QMF synthesis.
            if let Some(idx) = tail.range_index {
                apply_range(&mut block, crate::drc_range(idx));
            }

            for (ch, samples) in block.into_iter().enumerate() {
                pcm[ch].extend(samples);
            }
            bit += audio_bits;
        }

        Ok(pcm)
    }
}

/// Apply the §5.4.1 `RANGE` dynamic-range multiplier (the §D.4
/// [`crate::drc_range`] linear gain) to every reconstructed PCM sample
/// of one subframe, in place, after QMF synthesis. Results are rounded
/// to the nearest integer and saturated to the `i32` range.
fn apply_range(block: &mut SubframePcm, range: f64) {
    if range == 1.0 {
        return;
    }
    for channel in block.iter_mut() {
        for sample in channel.iter_mut() {
            let scaled = (*sample as f64 * range).round();
            *sample = if scaled >= i32::MAX as f64 {
                i32::MAX
            } else if scaled <= i32::MIN as f64 {
                i32::MIN
            } else {
                scaled as i32
            };
        }
    }
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

    /// Encode a clean §5.3.1 header (single channel, byte-aligned, with
    /// `dynamic_range`/`predictor_history`/`aspf` as given) by parsing
    /// the fixture, mutating the flags, and re-encoding. Returns the
    /// encoded header bytes (a body packed separately concatenates
    /// straight onto them; the caller parses the assembled buffer).
    fn encode_clean_header(dynf: bool, cpf: bool) -> Vec<u8> {
        let hdr_bytes: [u8; 16] = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ];
        let mut header = crate::parse_frame_header(&hdr_bytes).unwrap();
        header.dynamic_range = dynf;
        // CPF is the §5.3.1 CRC-Present-Flag (`crc_present`), the flag
        // that gates HCRC / AHCRC / SICRC — NOT `predictor_history`.
        header.crc_present = cpf;
        // When CPF is set the header carries a 16-bit HCRC; supply a
        // value so the BE encoder serialises the field (its value is not
        // verified on decode per §5.3.1).
        header.header_crc = if cpf { Some(0) } else { None };
        header.aspf = false;
        crate::encode_frame_header_be(&header).unwrap()
    }

    /// A one-channel, one-subframe, all-`ABITS==0` (NoBits) Core frame
    /// decodes end to end from raw bytes through `decode_core_frame` to
    /// all-zero PCM of the right length.
    #[test]
    fn decode_core_frame_no_bits_round_trips() {
        let mut bytes = encode_clean_header(false, false);

        // §5.3.2 Audio Coding Header (Table 5-21), one channel:
        //   SUBFS=0 -> 1 subframe; PCHS=0 -> 1 channel;
        //   SUBS=0 -> nSUBS=2; VQSUB=1 -> nVQSUB=2 (== nSUBS, no HF VQ);
        //   JOINX=0; THUFF=0; SHUFF=0; BHUFF=0.
        //   SEL plane: ABITS1 1 bit, ABITS2-5 4×2 bits, ABITS6-10 5×3 bits.
        //   With every SEL=0, every group transmits a 2-bit ADJ -> 10 ADJ.
        let mut body: Vec<(u32, u8)> = vec![
            (0, 4), // SUBFS
            (0, 3), // PCHS
            (0, 5), // SUBS -> nSUBS 2
            (1, 5), // VQSUB -> nVQSUB 2
            (0, 3), // JOINX
            (0, 2), // THUFF
            (0, 3), // SHUFF
            (6, 3), // BHUFF=6 -> Linear5Bit (5-bit ABITS reads)
        ];
        body.push((0, 1)); // SEL ABITS1
        for _ in 1..5 {
            body.push((0, 2));
        }
        for _ in 5..10 {
            body.push((0, 3));
        }
        for _ in 0..10 {
            body.push((0, 2)); // ADJ
        }

        // §5.4.1 side info (Table 5-28), one subframe:
        //   SSC=0 -> nSSC=1; PSC=0; PMODE[0][0..2]=0 (2 bits);
        //   no PVQ (PMODE all 0); ABITS[0][0..2]=0 (2× the BHUFF=6
        //   Linear5Bit code -> 5 bits each, value 0); nSSC==1 so no
        //   TMODE plane; all ABITS 0 so no SCALES factors for the two
        //   primary subbands, and nVQSUB==nSUBS so no HF VQ scales.
        body.push((0, 2)); // SSC
        body.push((0, 3)); // PSC
        body.push((0, 1)); // PMODE[0][0]
        body.push((0, 1)); // PMODE[0][1]
        body.push((0, 5)); // ABITS[0][0] (BHUFF=6 Linear5Bit) = 0
        body.push((0, 5)); // ABITS[0][1] = 0

        // §5.5 Audio Data: nSSC=1, all ABITS 0 -> NoBits -> no audio
        // bits, then the single DSYNC trailer.
        body.push((0xffff, 16));

        let body_bytes = pack_fields(&body);
        bytes.extend_from_slice(&body_bytes);
        // A little trailing slack so the header parser's lookahead is
        // always satisfied.
        bytes.extend_from_slice(&[0u8; 4]);

        let header = crate::parse_frame_header(&bytes).unwrap();
        assert!(!header.dynamic_range);
        assert!(!header.crc_present);
        assert_eq!(header.header_bit_length() % 8, 0);

        let pcm = decode_core_frame(&bytes, &header).unwrap();
        assert_eq!(pcm.len(), 1);
        // One subframe, one subsubframe -> 8 rows -> 256 PCM samples.
        assert_eq!(pcm[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(pcm[0].iter().all(|&s| s == 0));
    }

    /// The §5.3.2 one-channel NoBits ACH body shared by the tail tests:
    /// SUBFS=0/PCHS=0/SUBS=0(nSUBS 2)/VQSUB=1(nVQSUB 2)/JOINX=0, all
    /// codebook selectors 0 except BHUFF=6 (Linear5Bit), the SEL plane,
    /// and the 10 ADJ groups. When `cpf` is set a 16-bit AHCRC trailer
    /// is appended (consumed by `decode_audio_coding_header_at`).
    fn nobits_ach_body(cpf: bool) -> Vec<(u32, u8)> {
        let mut body: Vec<(u32, u8)> = vec![
            (0, 4), // SUBFS
            (0, 3), // PCHS
            (0, 5), // SUBS -> nSUBS 2
            (1, 5), // VQSUB -> nVQSUB 2
            (0, 3), // JOINX
            (0, 2), // THUFF
            (0, 3), // SHUFF
            (6, 3), // BHUFF=6 -> Linear5Bit
        ];
        body.push((0, 1)); // SEL ABITS1
        for _ in 1..5 {
            body.push((0, 2));
        }
        for _ in 5..10 {
            body.push((0, 3));
        }
        for _ in 0..10 {
            body.push((0, 2)); // ADJ
        }
        if cpf {
            body.push((0, 16)); // AHCRC
        }
        body
    }

    /// The §5.4.1 one-subframe NoBits side-info SCALES block (SSC/PSC,
    /// 2 PMODE bits, 2 zero ABITS Linear5Bit reads — no SCALES, no HF
    /// VQ since nVQSUB==nSUBS).
    fn nobits_side_info() -> Vec<(u32, u8)> {
        vec![
            (0, 2), // SSC
            (0, 3), // PSC
            (0, 1), // PMODE[0][0]
            (0, 1), // PMODE[0][1]
            (0, 5), // ABITS[0][0] = 0
            (0, 5), // ABITS[0][1] = 0
        ]
    }

    /// A frame whose header sets `CPF` (a 16-bit `SICRC` side-info tail)
    /// now decodes end to end: the `SICRC` word is consumed for framing
    /// (its CRC test is not applied per §5.4.1) and the §5.5 region lands
    /// at the right cursor, yielding all-zero PCM of the right length.
    #[test]
    fn decode_core_frame_consumes_sicrc_tail() {
        let mut bytes = encode_clean_header(false, true); // DYNF=0, CPF=1
        let mut body = nobits_ach_body(true);
        body.extend(nobits_side_info());
        body.push((0xABCD, 16)); // SICRC (CPF=1) — consumed, not verified
        body.push((0xffff, 16)); // §5.5 DSYNC
        let body_bytes = pack_fields(&body);
        bytes.extend_from_slice(&body_bytes);
        bytes.extend_from_slice(&[0u8; 4]);

        let header = crate::parse_frame_header(&bytes).unwrap();
        assert!(header.crc_present);

        let pcm = decode_core_frame(&bytes, &header).unwrap();
        assert_eq!(pcm.len(), 1);
        assert_eq!(pcm[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(pcm[0].iter().all(|&s| s == 0));
    }

    /// A frame whose header sets `DYNF` carries an 8-bit `RANGE` index in
    /// each subframe's side-info tail; `decode_core_frame` consumes it
    /// and (for a non-unity index) the §D.4 multiplier scales the PCM.
    /// With an all-zero (NoBits) subframe the PCM is zero regardless of
    /// `RANGE`, which proves only the framing/cursor is correct — the
    /// `apply_range` value is covered by `range_unity_is_noop` /
    /// `range_scales_pcm`.
    #[test]
    fn decode_core_frame_consumes_range_tail() {
        let mut bytes = encode_clean_header(true, false); // DYNF=1, CPF=0
        let mut body = nobits_ach_body(false);
        body.extend(nobits_side_info());
        body.push((127, 8)); // RANGE index 127 -> unity (no SICRC, CPF=0)
        body.push((0xffff, 16)); // §5.5 DSYNC
        let body_bytes = pack_fields(&body);
        bytes.extend_from_slice(&body_bytes);
        bytes.extend_from_slice(&[0u8; 4]);

        let header = crate::parse_frame_header(&bytes).unwrap();
        assert!(header.dynamic_range);

        let pcm = decode_core_frame(&bytes, &header).unwrap();
        assert_eq!(pcm.len(), 1);
        assert_eq!(pcm[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(pcm[0].iter().all(|&s| s == 0));
    }

    /// A frame with a `JOINX > 0` channel is still declined — the
    /// joint-intensity `JOIN_SHUFF`/`JOIN_SCALES` tail needs the unstaged
    /// joint-scale table.
    #[test]
    fn decode_core_frame_declines_joint_intensity() {
        let mut bytes = encode_clean_header(false, false);
        // ACH with JOINX[0] = 1 (> 0).
        let mut body: Vec<(u32, u8)> = vec![
            (0, 4), // SUBFS
            (0, 3), // PCHS
            (0, 5), // SUBS
            (1, 5), // VQSUB
            (1, 3), // JOINX = 1 (> 0)
            (0, 2), // THUFF
            (0, 3), // SHUFF
            (6, 3), // BHUFF
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
        let body_bytes = pack_fields(&body);
        bytes.extend_from_slice(&body_bytes);
        bytes.extend_from_slice(&[0u8; 4]);

        let header = crate::parse_frame_header(&bytes).unwrap();
        let err = decode_core_frame(&bytes, &header).unwrap_err();
        assert!(matches!(
            err,
            CoreFrameDecodeError::UnsupportedSideInfoTail {
                joint_intensity: true,
                ..
            }
        ));
    }

    /// `apply_range` with the §D.4 unity index leaves the PCM untouched.
    #[test]
    fn range_unity_is_noop() {
        let mut block: SubframePcm = vec![vec![100, -200, 0, i32::MAX, i32::MIN]];
        apply_range(&mut block, crate::drc_range(127)); // 1.0
        assert_eq!(block[0], vec![100, -200, 0, i32::MAX, i32::MIN]);
    }

    /// `apply_range` scales every sample by the §D.4 multiplier with
    /// round-to-nearest and `i32` saturation.
    #[test]
    fn range_scales_pcm() {
        // Index 47 -> 0.1; index 207 -> 10.0.
        let mut down: SubframePcm = vec![vec![1000, -1000, 5]];
        apply_range(&mut down, crate::drc_range(47)); // 0.1
        assert_eq!(down[0], vec![100, -100, 1]); // 5*0.1=0.5 -> round 1

        let mut up: SubframePcm = vec![vec![10, -10, i32::MAX]];
        apply_range(&mut up, crate::drc_range(207)); // 10.0
        assert_eq!(up[0], vec![100, -100, i32::MAX]); // saturates
    }

    /// Build a complete one-channel all-`ABITS==0` (NoBits) raw-BE Core
    /// frame — the same proven layout `decode_core_frame_no_bits_round_trips`
    /// uses — with a non-unity §D.4 `RANGE` index optionally injected so
    /// the post-QMF PCM is forced non-zero (exercising the `apply_range`
    /// path even though the §5.5 audio data is silent). When `dynf` is
    /// `false` no `RANGE` field is present and the frame decodes to
    /// all-zero PCM.
    fn build_nobits_frame(dynf: bool, range_index: u8) -> Vec<u8> {
        let mut header = crate::parse_frame_header(&[
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03,
            0xef, 0x7f,
        ])
        .unwrap();
        header.dynamic_range = dynf;
        header.crc_present = false;
        header.header_crc = None;
        header.aspf = false;
        let mut bytes = crate::encode_frame_header_be(&header).unwrap();

        // §5.3.2 ACH: one channel, nSUBS=2/nVQSUB=2, BHUFF=6 Linear5Bit,
        // SEL plane all zero (every group transmits a 2-bit ADJ), 10 ADJ.
        let mut body: Vec<(u32, u8)> = vec![
            (0, 4), // SUBFS -> 1 subframe
            (0, 3), // PCHS -> 1 channel
            (0, 5), // SUBS -> nSUBS 2
            (1, 5), // VQSUB -> nVQSUB 2
            (0, 3), // JOINX
            (0, 2), // THUFF
            (0, 3), // SHUFF
            (6, 3), // BHUFF=6 Linear5Bit
        ];
        body.push((0, 1)); // SEL ABITS1
        for _ in 1..5 {
            body.push((0, 2));
        }
        for _ in 5..10 {
            body.push((0, 3));
        }
        for _ in 0..10 {
            body.push((0, 2)); // ADJ
        }

        // §5.4.1 side info: SSC/PSC, 2 PMODE bits, 2 zero ABITS reads.
        body.push((0, 2)); // SSC -> nSSC 1
        body.push((0, 3)); // PSC
        body.push((0, 1)); // PMODE[0][0]
        body.push((0, 1)); // PMODE[0][1]
        body.push((0, 5)); // ABITS[0][0] = 0
        body.push((0, 5)); // ABITS[0][1] = 0

        // Table 5-28 tail: an 8-bit RANGE index when DYNF (CPF=0 so no
        // SICRC), then the §5.5 DSYNC trailer.
        if dynf {
            body.push((range_index as u32, 8));
        }
        body.push((0xffff, 16)); // DSYNC

        bytes.extend_from_slice(&pack_fields(&body));
        bytes.extend_from_slice(&[0u8; 4]);
        bytes
    }

    /// [`CoreStreamDecoder::decode_frame`] reproduces the standalone
    /// [`decode_core_frame`] result frame-for-frame (the per-frame body
    /// is the shared [`SubframePcmDecoder::decode_core_frame_into`]); the
    /// difference is only in the persistent filter state carried between
    /// calls, which an all-zero stream cannot expose, so this pins the
    /// per-frame equivalence.
    #[test]
    fn core_stream_decode_matches_decode_core_frame_per_frame() {
        let f0 = build_nobits_frame(false, 0);
        let f1 = build_nobits_frame(false, 0);
        let h0 = crate::parse_frame_header(&f0).unwrap();
        let h1 = crate::parse_frame_header(&f1).unwrap();

        let mut stream = CoreStreamDecoder::new(1);
        let s0 = stream.decode_frame(&f0, &h0).unwrap();
        let s1 = stream.decode_frame(&f1, &h1).unwrap();
        assert_eq!(stream.channel_count(), 1);

        // Each frame matches the fresh-per-frame decode (silent stream:
        // the carried filter tail is zero, so the two paths agree).
        assert_eq!(s0, decode_core_frame(&f0, &h0).unwrap());
        assert_eq!(s1, decode_core_frame(&f1, &h1).unwrap());
        assert_eq!(s0[0].len(), SAMPLES_PER_SUBSUBFRAME * PCM_PER_SUBBAND_ROW);
        assert!(s0[0].iter().all(|&v| v == 0));
    }

    /// [`CoreStreamDecoder`] reuses one persistent per-channel §C.2.5
    /// filter across frames rather than resetting it — the structural
    /// precondition for inter-frame filter continuity. (The end-to-end
    /// proof that this makes our PCM shape-identical to a black-box
    /// `ffmpeg -c:a dca` reference decode of a real multi-frame stream is
    /// the `decodes_real_fixture_stream_matching_ffmpeg_shape`
    /// integration test; with non-zero §5.5 audio the carried tail
    /// changes the next frame's leading samples, which an all-`ABITS==0`
    /// synthetic frame cannot exercise.)
    #[test]
    fn core_stream_reuses_persistent_filter_across_frames() {
        let f0 = build_nobits_frame(false, 0);
        let f1 = build_nobits_frame(false, 0);
        let h0 = crate::parse_frame_header(&f0).unwrap();
        let h1 = crate::parse_frame_header(&f1).unwrap();
        let mut stream = CoreStreamDecoder::new(1);

        // The same filter object (and its history) must survive a decode:
        // a silent stream leaves the history all-zero, so we assert the
        // decoder neither panics nor reallocates the channel filters.
        let _ = stream.decode_frame(&f0, &h0).unwrap();
        assert_eq!(stream.subframe_decoder().qmf().channel_count(), 1);
        let _ = stream.decode_frame(&f1, &h1).unwrap();
        assert_eq!(stream.subframe_decoder().qmf().channel_count(), 1);
        assert!(stream
            .subframe_decoder()
            .qmf()
            .channels()
            .iter()
            .all(|q| q.x_history().iter().all(|&v| v == 0.0)));
    }

    /// A [`CoreStreamDecoder`] built for the wrong channel count rejects
    /// a frame whose `nPCHS` disagrees, without panicking.
    #[test]
    fn core_stream_channel_count_mismatch_rejected() {
        let frame = build_nobits_frame(false, 0);
        let header = crate::parse_frame_header(&frame).unwrap();
        // The frame is one channel; a 2-channel decoder must decline.
        let mut stream = CoreStreamDecoder::new(2);
        let err = stream.decode_frame(&frame, &header).unwrap_err();
        assert!(matches!(
            err,
            CoreFrameDecodeError::Decode(SubframePcmError::ChannelCountMismatch {
                expected: 2,
                got: 1
            })
        ));
    }
}
