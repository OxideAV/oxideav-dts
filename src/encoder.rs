//! DTS Coherent Acoustics — **Core encoder** (§5.3 frame header,
//! §5.3.2 primary audio coding header, §5.4.1 side information, §5.5
//! primary audio data), driven by the crate's own §C.2.5-inverse
//! analysis bank ([`crate::QmfAnalysis`]) and §C.2.6-adjoint LFE
//! decimator ([`crate::LfeAnalysis`]).
//!
//! ETSI TS 102 114 is a decoder specification: it fixes the bitstream
//! and the reconstruction, and every encoder decision here is chosen
//! so the *decoder-side* pseudocode reproduces the input. The encoder
//! emits **normal frames** of 512 samples per channel (`NBLKS+1 = 16`
//! blocks, one subframe, two subsubframes), with:
//!
//! * per-band scale factors from the §D.1.2 7-bit RMS table (`SHUFF =
//!   6`; the difference-coded `SHUFF 0..=4` path exists behind
//!   [`HuffmanScales`] but is off by default — see there), chosen as
//!   the smallest table level whose quantizer span covers the
//!   subframe's band peak (overload-free);
//! * mid-tread quantization against the §D.2.1 step sizes, carried
//!   per `(channel, ABITS)` family through whichever Table 5-26 `SEL`
//!   is cheapest for the frame — a §D.5 Huffman book (with the
//!   Table 5-27 `ADJ` field at unity), a §D.6 block code
//!   (`ABITS 1..=7`) or the plain two's-complement NFE form;
//! * `ABITS` through the cheapest of the §D.5.6 12-level books or the
//!   linear 4-/5-bit fields (`BHUFF`), `TMODE` through the §D.5.2
//!   `A4` book (`THUFF = 0`);
//! * a greedy rate-distortion bit allocator that spends the frame's
//!   byte budget (from the configured Table 5-7 target rate) one
//!   `ABITS` step at a time on the band with the best
//!   noise-reduction-per-bit, in two passes: the first against the
//!   worst-case (block-code / NFE / linear) field widths, the second
//!   re-spending the bits the entropy selection saved;
//! * the LFE channel (`LFF = 2`, 64× decimation) as §5.5 8-bit
//!   samples plus a 7-bit-RMS scale index.
//!
//! The §5.4.1 fields the decoder reads but this encoder does not yet
//! exercise (`PMODE`/ADPCM, high-frequency VQ, joint intensity,
//! `DYNF`) are emitted in their inactive form.
//!
//! # Levels
//!
//! Input samples are normalized (`±1.0` = full scale). The decoder's
//! output plane is `pcm · 2³¹` (S32 full scale, matching the
//! black-box reference since the round-453 output calibration), so
//! the encoder feeds the analysis bank `pcm · 2¹⁵·⁵` — the value `v`
//! for which `int(rScale · √2 · synth(v))` with the 16-bit `PCMR`
//! `rScale = 2¹⁵` lands on `pcm · 2³¹`. A full-scale sine then peaks
//! near the top of the §D.1.2 scale table, the same headroom
//! convention the black-box reference encoder's streams exhibit.

use crate::audio_data::{CODEBOOK_GROUP_SIZE, QUANT_LEVELS};
use crate::audio_huff::{table_for, AudioHuffCodebook};
use crate::bitwriter::BitWriter;
use crate::block_code::block_code_offset;
use crate::cos_mod::NUM_SUBBAND;
use crate::filter_bank::FilterBankSelection;
use crate::header::{encode_frame_header_be, DtsFrameHeader, FrameType, LfeMode, SyncWordEncoding};
use crate::lfe_analysis::LfeAnalysis;
use crate::lfe_interp::LfeInterpolationSelection;
use crate::lfe_synth::LFE_SCALE_STEP;
use crate::qmf_analysis::QmfAnalysis;
use crate::side_info::{
    abits_table, scales_table, tmode_table, AbitsCodebook, ScalesCodebook, TmodeCodebook, RMS_6BIT,
    RMS_7BIT,
};
use crate::step_size::{StepSizeTable, SAMPLES_PER_SUBSUBFRAME};
use crate::unpack14::FourteenBitByteOrder;

/// Samples per channel in one encoded frame (16 blocks of 32).
pub const ENCODER_FRAME_SAMPLES: usize = 512;

/// Subsubframes per frame (`SSC + 1`).
const N_SSC: usize = 2;

/// Subband samples per band per frame.
const SAMPLES_PER_BAND: usize = N_SSC * SAMPLES_PER_SUBSUBFRAME;

/// PCM lookahead (beyond the frame being encoded) the encoder buffers
/// before it can emit that frame: the QMF analysis window plus the
/// LFE decimator's equalizer span.
pub const ENCODER_LOOKAHEAD: usize = 1280;

/// The §5.3.1 Table 5-7 `RATE` codes and their targeted bit rates in
/// bits/s (`0b11101` "open" and the invalid codes excluded).
const RATE_TABLE: [(u8, u32); 25] = [
    (0, 32_000),
    (1, 56_000),
    (2, 64_000),
    (3, 96_000),
    (4, 112_000),
    (5, 128_000),
    (6, 192_000),
    (7, 224_000),
    (8, 256_000),
    (9, 320_000),
    (10, 384_000),
    (11, 448_000),
    (12, 512_000),
    (13, 576_000),
    (14, 640_000),
    (15, 768_000),
    (16, 960_000),
    (17, 1_024_000),
    (18, 1_152_000),
    (19, 1_280_000),
    (20, 1_344_000),
    (21, 1_408_000),
    (22, 1_411_200),
    (23, 1_472_000),
    (24, 1_536_000),
];

/// Analysis-domain gain: normalized PCM → the §C.2.5 `raZ` domain
/// (see the module docs — `2^15.5`).
const ANALYSIS_INPUT_GAIN: f64 = 46_340.950_011_841_57;

/// LFE decimated-domain gain: normalized PCM → the §5.5 LFE dequant
/// domain (`2²³`, see the module docs).
const LFE_INPUT_GAIN: f64 = 8_388_608.0;

/// §D.6 `V…` block-code word widths per `ABITS` family (V3..V25),
/// matching the decoder's `block_code_word_bits`.
const BLOCK_WORD_BITS: [u32; 8] = [0, 7, 10, 12, 13, 15, 17, 19];

/// Highest `ABITS` the allocator uses (the last valid §D.2 row).
const MAX_ABITS: u8 = 26;

/// Largest quantization index magnitude per `ABITS`: the mid-tread
/// `(levels−1)/2` for the block-code / Huffman families, and the
/// symmetric two's-complement bound for NFE (which is also inside
/// every §D.5 book of the same family).
fn qmax(abits: u8) -> i32 {
    match abits {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 => 8,
        7 => 12,
        a if (8..=MAX_ABITS).contains(&a) => (1i32 << (a - 4)) - 1,
        _ => 0,
    }
}

/// Worst-case audio bits for one band over one frame at `abits`
/// (block code / NFE — the terminal `SEL` the allocator plans with).
fn band_sample_bits(abits: u8) -> usize {
    match abits {
        0 => 0,
        1..=7 => 4 * BLOCK_WORD_BITS[abits as usize] as usize,
        a => SAMPLES_PER_BAND * (a as usize - 3),
    }
}

/// Worst-case side-information bits a band pays when it first becomes
/// active (`TMODE` 1 bit through the A4 zero code + 7-bit linear scale
/// factor). The 5-bit linear `ABITS` field is planned for every band.
const ACTIVE_BAND_SIDE_BITS: usize = 1 + 7;

/// `(code, length)` of `symbol` in a `(symbol, length, code)` book.
fn huff_code(table: &[(i16, u8, u16)], symbol: i16) -> Option<(u32, u32)> {
    table
        .iter()
        .find(|&&(sym, _, _)| sym == symbol)
        .map(|&(_, len, code)| (u32::from(code), u32::from(len)))
}

/// Encoder configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Core sample rate in Hz (must be a Table 5-5 rate).
    pub sample_rate: u32,
    /// Primary channel count (1..=5; Table 5-4 layouts A, L R,
    /// C L R, L R SL SR, C L R SL SR).
    pub channels: usize,
    /// Whether an LFE channel is carried (64× decimation, `LFF = 2`).
    pub lfe: bool,
    /// Table 5-7 `RATE` code for the targeted bit rate.
    pub rate_index: u8,
    /// §D.8 prototype selection (`FILTS`); the analysis bank and the
    /// header flag always agree.
    pub filter: FilterBankSelection,
    /// Policy for the §D.5.12 difference-coded 6-bit scale factors
    /// (`SHUFF 0..=4`) versus the 7-bit linear field.
    pub huffman_scales: HuffmanScales,
    /// On-wire word format of the emitted frames (raw 16-bit
    /// big-/little-endian, or the 14-bits-per-word containers). The
    /// 14-bit forms round the frame down to a multiple of 14 bytes so
    /// every frame packs into whole 28-bit container pairs and the
    /// stream stays sync-aligned (§6.1.3.1 / §3.3 of the staged
    /// extracts).
    pub sync_word_encoding: SyncWordEncoding,
}

/// Policy for the difference-coded scale factors (`SHUFF 0..=4`).
///
/// **Default [`Never`](Self::Never).** The spec names the five
/// scale-factor books `SA129..SE129` (Table 5-24) but prints no table
/// under those names; this crate codes them through the §D.5.12
/// A129..E129 audio books, which is structurally consistent (a ±64
/// difference alphabet over the 64-entry §D.1.1 grid, accumulated
/// from zero) yet **rejected by the black-box reference decoder**, so
/// such streams round-trip only through this crate's own decoder.
/// The other policies exist for experimentation and for the day the
/// real books are staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuffmanScales {
    /// Never (always `SHUFF = 6`, 7-bit linear) — interoperable.
    Never,
    /// Whenever cheaper for the frame (6-bit §D.1.1 grid).
    WhenCheaper,
    /// Whenever cheaper, but only for rate codes below 512 kbit/s.
    LowRatesOnly,
}

/// Errors from encoder configuration / input plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodeError {
    /// Sample rate is not one of the Table 5-5 core rates.
    UnsupportedSampleRate {
        /// The rejected rate in Hz.
        sample_rate: u32,
    },
    /// Primary channel count outside 1..=5.
    UnsupportedChannelCount {
        /// The rejected primary-channel count.
        channels: usize,
    },
    /// No Table 5-7 rate code at or above the requested bit rate.
    BitRateTooHigh {
        /// The requested rate in bit/s.
        bits_per_second: u32,
    },
    /// Requested rate leaves less than the minimum legal frame
    /// (`FSIZE + 1 ≥ 96` bytes).
    BitRateTooLow {
        /// The targeted rate in bit/s.
        bits_per_second: u32,
    },
    /// `rate_index` is not a fixed Table 5-7 code.
    InvalidRateIndex {
        /// The rejected 5-bit RATE code.
        rate_index: u8,
    },
    /// Pushed plane count does not match the configuration.
    ChannelCountMismatch {
        /// Planes the configuration requires (primaries + LFE).
        expected: usize,
        /// Planes the caller pushed.
        got: usize,
    },
    /// Pushed planes have differing lengths.
    PlaneLengthMismatch,
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedSampleRate { sample_rate } => {
                write!(f, "unsupported core sample rate {sample_rate} Hz")
            }
            Self::UnsupportedChannelCount { channels } => {
                write!(f, "unsupported primary channel count {channels}")
            }
            Self::BitRateTooHigh { bits_per_second } => {
                write!(f, "no Table 5-7 rate code covers {bits_per_second} bit/s")
            }
            Self::BitRateTooLow { bits_per_second } => {
                write!(f, "{bits_per_second} bit/s is below the minimum frame size")
            }
            Self::InvalidRateIndex { rate_index } => {
                write!(f, "RATE code {rate_index} is not a fixed Table 5-7 rate")
            }
            Self::ChannelCountMismatch { expected, got } => {
                write!(f, "expected {expected} input planes, got {got}")
            }
            Self::PlaneLengthMismatch => write!(f, "input planes differ in length"),
        }
    }
}

impl std::error::Error for EncodeError {}

impl EncoderConfig {
    /// Configuration for `channels` primary channels at `sample_rate`,
    /// defaulting to 768 kbit/s, no LFE, and the Perfect
    /// Reconstruction prototype.
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, EncodeError> {
        if sfreq_code(sample_rate).is_none() {
            return Err(EncodeError::UnsupportedSampleRate { sample_rate });
        }
        if !(1..=5).contains(&channels) {
            return Err(EncodeError::UnsupportedChannelCount { channels });
        }
        Ok(Self {
            sample_rate,
            channels,
            lfe: false,
            rate_index: 15,
            filter: FilterBankSelection::PerfectReconstruction,
            huffman_scales: HuffmanScales::Never,
            sync_word_encoding: SyncWordEncoding::RawBigEndian,
        })
    }

    /// Enable/disable the LFE channel (64× decimation).
    #[must_use]
    pub fn with_lfe(mut self, lfe: bool) -> Self {
        self.lfe = lfe;
        self
    }

    /// Select the targeted bit rate as the smallest Table 5-7 code at
    /// or above `bits_per_second`.
    pub fn with_bit_rate(mut self, bits_per_second: u32) -> Result<Self, EncodeError> {
        let code = RATE_TABLE
            .iter()
            .find(|&&(_, bps)| bps >= bits_per_second)
            .map(|&(code, _)| code)
            .ok_or(EncodeError::BitRateTooHigh { bits_per_second })?;
        self.rate_index = code;
        // Reject rates whose frame would be under the legal minimum.
        self.frame_bytes()?;
        Ok(self)
    }

    /// Select the §D.8 prototype (`FILTS`).
    #[must_use]
    pub fn with_filter(mut self, filter: FilterBankSelection) -> Self {
        self.filter = filter;
        self
    }

    /// Set the difference-coded scale-factor policy.
    #[must_use]
    pub fn with_huffman_scales(mut self, policy: HuffmanScales) -> Self {
        self.huffman_scales = policy;
        self
    }

    /// Set the on-wire word format (raw BE/LE or 14-bit BE/LE).
    #[must_use]
    pub fn with_sync_word_encoding(mut self, encoding: SyncWordEncoding) -> Self {
        self.sync_word_encoding = encoding;
        self
    }

    /// The targeted bit rate for [`Self::rate_index`], in bit/s.
    pub fn bit_rate_bps(&self) -> Result<u32, EncodeError> {
        RATE_TABLE
            .iter()
            .find(|&&(code, _)| code == self.rate_index)
            .map(|&(_, bps)| bps)
            .ok_or(EncodeError::InvalidRateIndex {
                rate_index: self.rate_index,
            })
    }

    /// Bytes of one encoded frame (`FSIZE + 1`): the targeted rate
    /// spread over [`ENCODER_FRAME_SAMPLES`], rounded down to a whole
    /// 16-bit word.
    pub fn frame_bytes(&self) -> Result<usize, EncodeError> {
        let bps = self.bit_rate_bps()?;
        let bytes = (u64::from(bps) * ENCODER_FRAME_SAMPLES as u64
            / u64::from(self.sample_rate)
            / 8) as usize;
        // Whole 16-bit words; the 14-bit containers additionally need
        // whole 28-bit pairs per frame (8·bytes ≡ 0 mod 14 ⇔ bytes ≡ 0
        // mod 7, so multiples of 14 keep both).
        let bytes = if self.sync_word_encoding.is_14bit_packed() {
            bytes / 14 * 14
        } else {
            bytes & !1
        }
        .min(16_384);
        if bytes < 96 {
            return Err(EncodeError::BitRateTooLow {
                bits_per_second: bps,
            });
        }
        Ok(bytes)
    }

    /// Total input planes (`channels`, plus one trailing LFE plane).
    #[must_use]
    pub fn plane_count(&self) -> usize {
        self.channels + usize::from(self.lfe)
    }

    /// Table 5-4 `AMODE` code for [`Self::channels`].
    #[must_use]
    pub fn amode(&self) -> u8 {
        match self.channels {
            1 => 0, // A
            2 => 2, // L + R
            3 => 5, // C + L + R
            4 => 8, // L + R + SL + SR
            _ => 9, // C + L + R + SL + SR
        }
    }

    /// Whether this frame may use the difference-coded scale factors.
    fn huffman_scales_allowed(&self) -> bool {
        match self.huffman_scales {
            HuffmanScales::Never => false,
            HuffmanScales::WhenCheaper => true,
            HuffmanScales::LowRatesOnly => self.rate_index < 12,
        }
    }
}

/// Table 5-5 `SFREQ` code for a core sample rate.
fn sfreq_code(sample_rate: u32) -> Option<u8> {
    Some(match sample_rate {
        8_000 => 0b0001,
        16_000 => 0b0010,
        32_000 => 0b0011,
        11_025 => 0b0110,
        22_050 => 0b0111,
        44_100 => 0b1000,
        12_000 => 0b1011,
        24_000 => 0b1100,
        48_000 => 0b1101,
        _ => return None,
    })
}

/// Streaming DTS Core encoder.
///
/// Push normalized (±1.0 full-scale) planar PCM with
/// [`Self::push`]; whole frames come back as soon as the analysis
/// lookahead is covered, and [`Self::flush`] zero-pads to drain the
/// tail. The decode chain is zero-delay against this encoder: frame
/// `k` reconstructs input samples `512k .. 512k+512` sample-for-
/// sample (both the QMF pair and the LFE pair are delay-free by
/// construction); the decoder's first 512 output samples are its
/// filter priming against zero history, as for any DTS stream start.
#[derive(Debug, Clone)]
pub struct CoreEncoder {
    config: EncoderConfig,
    frame_bytes: usize,
    qmf: QmfAnalysis,
    lfe: Option<LfeAnalysis>,
    /// Per-plane input buffers (primaries, then LFE), in normalized
    /// PCM, holding everything not yet consumed by an emitted frame.
    buf: Vec<Vec<f64>>,
    /// Samples of `buf` already emitted as frames (the next frame
    /// starts here). Buffers are compacted as frames leave.
    consumed: usize,
}

impl CoreEncoder {
    /// Build an encoder for `config`.
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let frame_bytes = config.frame_bytes()?;
        Ok(Self {
            config,
            frame_bytes,
            qmf: QmfAnalysis::new(config.filter),
            lfe: config
                .lfe
                .then(|| LfeAnalysis::new(LfeInterpolationSelection::Decimation64)),
            buf: vec![Vec::new(); config.plane_count()],
            consumed: 0,
        })
    }

    /// The configuration in use.
    #[must_use]
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Bytes of every emitted frame (`FSIZE + 1`).
    #[must_use]
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Feed planar PCM (primaries in Table 5-4 order, then the LFE
    /// plane when configured) and collect every frame that becomes
    /// encodable. Planes must have equal lengths per call; call
    /// lengths are unconstrained.
    pub fn push(&mut self, planes: &[&[f64]]) -> Result<Vec<Vec<u8>>, EncodeError> {
        if planes.len() != self.buf.len() {
            return Err(EncodeError::ChannelCountMismatch {
                expected: self.buf.len(),
                got: planes.len(),
            });
        }
        if planes.windows(2).any(|pair| pair[0].len() != pair[1].len()) {
            return Err(EncodeError::PlaneLengthMismatch);
        }
        for (buf, plane) in self.buf.iter_mut().zip(planes) {
            buf.extend_from_slice(plane);
        }
        let mut frames = Vec::new();
        while self.buf[0].len() - self.consumed >= ENCODER_FRAME_SAMPLES + ENCODER_LOOKAHEAD {
            frames.push(self.encode_next_frame());
            self.compact();
        }
        Ok(frames)
    }

    /// Zero-pad the pending tail and emit the remaining frames (the
    /// last frame is the one containing the final real sample; the
    /// encoder state then holds no pending audio).
    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        let pending = self.buf[0].len() - self.consumed;
        let mut frames = Vec::new();
        if pending == 0 {
            return frames;
        }
        let whole = pending.div_ceil(ENCODER_FRAME_SAMPLES) * ENCODER_FRAME_SAMPLES;
        let target = self.consumed + whole + ENCODER_LOOKAHEAD;
        for buf in self.buf.iter_mut() {
            buf.resize(target, 0.0);
        }
        while self.buf[0].len() - self.consumed >= ENCODER_FRAME_SAMPLES + ENCODER_LOOKAHEAD {
            frames.push(self.encode_next_frame());
            self.compact();
        }
        for buf in self.buf.iter_mut() {
            buf.clear();
        }
        self.consumed = 0;
        frames
    }

    /// Drop fully-consumed history the analysis windows can no longer
    /// reach.
    fn compact(&mut self) {
        // The QMF window for the next frame starts exactly at
        // `consumed`; nothing before it is read again. The LFE
        // equalizer reaches LFE_EQ_HALF decimated samples (× 64)
        // back.
        let keep_back = crate::lfe_analysis::LFE_EQ_HALF * 64;
        if self.consumed > keep_back + ENCODER_FRAME_SAMPLES * 4 {
            let cut = self.consumed - keep_back;
            for buf in self.buf.iter_mut() {
                buf.drain(..cut);
            }
            self.consumed -= cut;
        }
    }

    /// Encode the frame starting at `self.consumed` (lookahead is
    /// guaranteed by the callers).
    fn encode_next_frame(&mut self) -> Vec<u8> {
        let start = self.consumed;
        let channels = self.config.channels;
        let rows_per_frame = ENCODER_FRAME_SAMPLES / NUM_SUBBAND;

        // (1) Analysis: 16 subband rows per primary channel.
        let mut rows: Vec<Vec<[f64; NUM_SUBBAND]>> = Vec::with_capacity(channels);
        for ch in 0..channels {
            let scaled: Vec<f64> = self.buf[ch]
                [start..start + ENCODER_FRAME_SAMPLES + crate::QMF_ANALYSIS_LOOKAHEAD]
                .iter()
                .map(|&v| v * ANALYSIS_INPUT_GAIN)
                .collect();
            let ch_rows = self.qmf.analyze(&scaled);
            debug_assert_eq!(ch_rows.len(), rows_per_frame);
            rows.push(ch_rows);
        }

        // (2) LFE decimation: 8 decimated samples for this frame.
        let lfe_decimated: Option<Vec<f64>> = self.lfe.as_ref().map(|dec| {
            let plane = &self.buf[channels];
            let span = &plane[start..];
            dec.decimate_count(span, ENCODER_FRAME_SAMPLES / 64)
                .iter()
                .map(|&v| v * LFE_INPUT_GAIN)
                .collect()
        });

        self.consumed += ENCODER_FRAME_SAMPLES;

        // (3) Statistics + allocation + entropy selection + emission.
        let frame = encode_frame_bits(
            &self.config,
            self.frame_bytes,
            &rows,
            lfe_decimated.as_deref(),
        );
        to_wire(frame, self.config.sync_word_encoding)
    }
}

// ---------------------------------------------------------------
// Per-frame planning
// ---------------------------------------------------------------

/// Per-band planning record.
#[derive(Debug, Clone, Copy)]
struct BandPlan {
    /// Chosen `ABITS`.
    abits: u8,
    /// Chosen §D.1 scale index (in the table the channel's `SHUFF`
    /// implies) — valid when `abits > 0`.
    scale_index: u8,
    /// Quantization indices for the frame — valid when `abits > 0`.
    idx: [i32; SAMPLES_PER_BAND],
}

impl Default for BandPlan {
    fn default() -> Self {
        Self {
            abits: 0,
            scale_index: 0,
            idx: [0; SAMPLES_PER_BAND],
        }
    }
}

/// Per-channel entropy selection.
#[derive(Debug, Clone)]
struct ChannelCoding {
    /// `BHUFF` selector.
    bhuff: u8,
    /// `SHUFF` selector.
    shuff: u8,
    /// `SEL[ch][ABITS-1]` for `ABITS 1..=10`.
    sel: [u8; 10],
}

/// Smallest §D.1 level in `table` (a valid-prefix slice) whose
/// quantizer span covers `peak` at `abits`, i.e. `qmax·step·RMS ≥
/// peak`; the top valid level when nothing covers it.
fn scale_index_in(abits: u8, peak: f64, step_table: StepSizeTable, table: &[u32]) -> u8 {
    let step = step_table
        .step_size(abits)
        .expect("allocator only uses valid ABITS");
    let need = peak / (f64::from(qmax(abits)) * step);
    for (idx, &v) in table.iter().enumerate() {
        if f64::from(v) >= need {
            return idx as u8;
        }
    }
    (table.len() - 1) as u8
}

/// Valid prefix of a §D.1 table (the reserved entries excluded:
/// §D.1.1 index 63, §D.1.2 indices 125..=127).
fn valid_rms(table: &[u32]) -> &[u32] {
    if table.len() == 128 {
        &table[..125]
    } else {
        &table[..63]
    }
}

/// Quantization-noise power estimate (per sample) at `abits` with the
/// overload-free scale rule above, on the 7-bit grid.
fn noise_power(abits: u8, peak: f64, step_table: StepSizeTable) -> f64 {
    let idx = scale_index_in(abits, peak, step_table, valid_rms(&RMS_7BIT));
    let step = step_table.step_size(abits).expect("valid ABITS");
    let q = step * f64::from(RMS_7BIT[idx as usize]);
    q * q / 12.0
}

/// One allocator step (for rollback): `(channel, band, previous
/// ABITS, previous noise)`.
type AllocStep = (usize, usize, u8, f64);

/// Greedy allocation continuing from `plan`, spending at most
/// `budget` worst-case bits; returns the steps taken, newest last.
fn allocate(
    plan: &mut [[BandPlan; NUM_SUBBAND]],
    noise: &mut [[f64; NUM_SUBBAND]],
    ladder: &[[f64; 27]],
    peak: &[[f64; NUM_SUBBAND]],
    mut budget: usize,
) -> Vec<AllocStep> {
    let channels = plan.len();
    let mut steps = Vec::new();
    loop {
        // Pick the (channel, band, target ABITS) buying the largest
        // noise reduction per bit within the remaining budget.
        let mut best: Option<(usize, usize, u8, usize, f64)> = None;
        for ch in 0..channels {
            for n in 0..NUM_SUBBAND {
                let cur = plan[ch][n].abits;
                if cur >= MAX_ABITS || peak[ch][n] <= 0.0 {
                    continue;
                }
                let l = &ladder[ch * NUM_SUBBAND + n];
                for next in (cur + 1)..=MAX_ABITS {
                    let extra = band_sample_bits(next) - band_sample_bits(cur)
                        + if cur == 0 { ACTIVE_BAND_SIDE_BITS } else { 0 };
                    if extra > budget {
                        break;
                    }
                    let gain = noise[ch][n] - l[next as usize];
                    if gain <= 0.0 {
                        continue;
                    }
                    let score = gain / extra as f64;
                    if best.map_or(true, |(.., s)| score > s) {
                        best = Some((ch, n, next, extra, score));
                    }
                }
            }
        }
        let Some((ch, n, next, extra, _)) = best else {
            break;
        };
        steps.push((ch, n, plan[ch][n].abits, noise[ch][n]));
        plan[ch][n].abits = next;
        noise[ch][n] = ladder[ch * NUM_SUBBAND + n][next as usize];
        budget -= extra;
    }
    steps
}

/// Quantize every active band of one channel against its chosen
/// scale grid (`table` = the valid prefix of the §D.1 table its
/// `SHUFF` implies).
fn quantize_channel(
    plan: &mut [BandPlan; NUM_SUBBAND],
    rows: &[[f64; NUM_SUBBAND]],
    peak: &[f64; NUM_SUBBAND],
    step_table: StepSizeTable,
    table: &[u32],
) {
    for (n, band) in plan.iter_mut().enumerate() {
        if band.abits == 0 {
            continue;
        }
        band.scale_index = scale_index_in(band.abits, peak[n], step_table, table);
        let step = step_table.step_size(band.abits).expect("valid ABITS");
        let recon = step * f64::from(table[band.scale_index as usize]);
        let q = qmax(band.abits);
        for (m, slot) in band.idx.iter_mut().enumerate() {
            *slot = (rows[m][n] / recon)
                .round()
                .clamp(f64::from(-q), f64::from(q)) as i32;
        }
    }
}

/// Audio bits of one band under a given `SEL` for its family.
fn band_audio_bits(band: &BandPlan, sel: u8) -> usize {
    let a = band.abits;
    if a == 0 {
        return 0;
    }
    if let Some(book) = AudioHuffCodebook::from_abits_sel(a, sel) {
        let table = table_for(book);
        band.idx
            .iter()
            .map(|&v| huff_code(table, v as i16).map_or(usize::MAX / 64, |(_, l)| l as usize))
            .sum()
    } else {
        band_sample_bits(a)
    }
}

/// Choose, per `(channel, ABITS 1..=10)` family, the cheapest `SEL`
/// (a §D.5 Huffman book with its 2-bit `ADJ`, or the terminal
/// block-code / NFE form). Returns the selection and the audio bits
/// of the whole channel (including the `ABITS > 10` NFE bands).
fn choose_sel(plan: &[BandPlan; NUM_SUBBAND]) -> ([u8; 10], usize) {
    let mut sel = [0u8; 10];
    let mut total = 0usize;
    for a in 1..=10u8 {
        let group = CODEBOOK_GROUP_SIZE[a as usize];
        let terminal = group - 1;
        let bands: Vec<&BandPlan> = plan.iter().filter(|b| b.abits == a).collect();
        let mut best = (
            terminal,
            bands
                .iter()
                .map(|b| band_audio_bits(b, terminal))
                .sum::<usize>(),
        );
        if !bands.is_empty() {
            for s in 0..terminal {
                // Huffman: sample codes + the 2-bit ADJ field.
                let bits = 2 + bands.iter().map(|b| band_audio_bits(b, s)).sum::<usize>();
                if bits < best.1 {
                    best = (s, bits);
                }
            }
        }
        sel[a as usize - 1] = best.0;
        total += best.1;
    }
    // ABITS 11..=26: NFE only.
    total += plan
        .iter()
        .filter(|b| b.abits > 10)
        .map(|b| band_sample_bits(b.abits))
        .sum::<usize>();
    (sel, total)
}

/// Choose the cheapest `BHUFF` for a channel's `ABITS` vector and
/// return `(bhuff, bits)`.
fn choose_bhuff(plan: &[BandPlan; NUM_SUBBAND], n_vqsub: usize) -> (u8, usize) {
    let abits = &plan[..n_vqsub];
    let mut best = (6u8, 5 * n_vqsub);
    if abits.iter().all(|b| b.abits <= 15) && 4 * n_vqsub < best.1 {
        best = (5, 4 * n_vqsub);
    }
    if abits.iter().all(|b| (1..=12).contains(&b.abits)) {
        for (code, cb) in [
            (0u8, AbitsCodebook::A12),
            (1, AbitsCodebook::B12),
            (2, AbitsCodebook::C12),
            (3, AbitsCodebook::D12),
            (4, AbitsCodebook::E12),
        ] {
            let table = abits_table(cb).expect("Huffman ABITS book");
            let bits: usize = abits
                .iter()
                .map(|b| {
                    huff_code(table, i16::from(b.abits))
                        .map_or(usize::MAX / 64, |(_, l)| l as usize)
                })
                .sum();
            if bits < best.1 {
                best = (code, bits);
            }
        }
    }
    best
}

/// Bits of a channel's SCALES field under a Huffman `SHUFF` book,
/// given the 6-bit indices of its active bands in band order.
fn scales_huffman_bits(indices: &[u8], cb: ScalesCodebook) -> Option<usize> {
    let table = scales_table(cb)?;
    let mut sum = 0i32;
    let mut bits = 0usize;
    for &idx in indices {
        let diff = i32::from(idx) - sum;
        let (_, len) = huff_code(table, diff as i16)?;
        bits += len as usize;
        sum = i32::from(idx);
    }
    Some(bits)
}

/// The greedy allocator + entropy selection + bitstream emission for
/// one frame.
fn encode_frame_bits(
    config: &EncoderConfig,
    frame_bytes: usize,
    rows: &[Vec<[f64; NUM_SUBBAND]>],
    lfe_decimated: Option<&[f64]>,
) -> Vec<u8> {
    let channels = config.channels;
    let step_table = StepSizeTable::for_rate(config.rate_index);
    let n_vqsub = NUM_SUBBAND;

    // --- Statistics -------------------------------------------------
    let mut peak = vec![[0.0_f64; NUM_SUBBAND]; channels];
    let mut power = vec![[0.0_f64; NUM_SUBBAND]; channels];
    for ch in 0..channels {
        for row in &rows[ch] {
            for (n, &v) in row.iter().enumerate() {
                peak[ch][n] = peak[ch][n].max(v.abs());
                power[ch][n] += v * v;
            }
        }
        for p in power[ch].iter_mut() {
            *p /= rows[ch].len() as f64;
        }
    }

    // --- Fixed-cost accounting (worst case) --------------------------
    let header_bits = 104usize; // CPF = 0
                                // Coding header: 4 + 3 + per channel (5+5+3+2+3+3) + SEL planes
                                // (1 + 4·2 + 5·3 = 24) + ADJ (2 per Huffman family, ≤ 10).
    let coding_header_bits = 7 + (21 + 24 + 20) * channels;
    let abits_worst = 5 * n_vqsub * channels; // linear 5-bit ABITS
    let side_fixed_bits = 5 /* SSC + PSC */ + NUM_SUBBAND * channels /* PMODE */ + abits_worst;
    let lfe_bits = if lfe_decimated.is_some() {
        8 * ENCODER_FRAME_SAMPLES / 64 + 8
    } else {
        0
    };
    let dsync_bits = 16usize;
    let budget_total = frame_bytes * 8;
    let fixed = header_bits + coding_header_bits + side_fixed_bits + lfe_bits + dsync_bits;
    let budget = budget_total.saturating_sub(fixed);

    // --- Pass 1: allocation on worst-case widths ---------------------
    let ladder: Vec<[f64; 27]> = (0..channels)
        .flat_map(|ch| (0..NUM_SUBBAND).map(move |n| (ch, n)).collect::<Vec<_>>())
        .map(|(ch, n)| {
            let mut l = [0.0_f64; 27];
            l[0] = power[ch][n];
            for a in 1..=MAX_ABITS {
                l[a as usize] = noise_power(a, peak[ch][n], step_table).min(power[ch][n]);
            }
            l
        })
        .collect();
    let mut plan = vec![[BandPlan::default(); NUM_SUBBAND]; channels];
    let mut noise: Vec<[f64; NUM_SUBBAND]> = power.clone();
    let _ = allocate(&mut plan, &mut noise, &ladder, &peak, budget);

    // --- Entropy selection (exact costs) -----------------------------
    // Quantize every channel on its chosen scale grid and pick the
    // cheapest SHUFF / SEL / BHUFF books; returns the per-channel
    // selections and the exact bits the frame uses (the worst-case
    // fixed part with the real side-info costs swapped in).
    let select_all = |plan: &mut Vec<[BandPlan; NUM_SUBBAND]>| -> (Vec<ChannelCoding>, usize) {
        let mut coding = Vec::with_capacity(channels);
        let mut actual = 0usize;
        for ch in 0..channels {
            let mut shuff = 6u8;
            let mut scale_bits = 7 * plan[ch].iter().filter(|b| b.abits > 0).count();
            if config.huffman_scales_allowed() {
                let idx6: Vec<u8> = plan[ch]
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.abits > 0)
                    .map(|(n, b)| {
                        scale_index_in(b.abits, peak[ch][n], step_table, valid_rms(&RMS_6BIT))
                    })
                    .collect();
                for (code, cb) in [
                    (0u8, ScalesCodebook::Sa129),
                    (1, ScalesCodebook::Sb129),
                    (2, ScalesCodebook::Sc129),
                    (3, ScalesCodebook::Sd129),
                    (4, ScalesCodebook::Se129),
                ] {
                    if let Some(bits) = scales_huffman_bits(&idx6, cb) {
                        if bits < scale_bits {
                            shuff = code;
                            scale_bits = bits;
                        }
                    }
                }
            }
            let table: &[u32] = if shuff == 6 {
                valid_rms(&RMS_7BIT)
            } else {
                valid_rms(&RMS_6BIT)
            };
            quantize_channel(&mut plan[ch], &rows[ch], &peak[ch], step_table, table);
            let (sel, audio_bits) = choose_sel(&plan[ch]);
            let (bhuff, abits_bits) = choose_bhuff(&plan[ch], n_vqsub);
            let tmode_bits = plan[ch].iter().filter(|b| b.abits > 0).count();
            actual += audio_bits + abits_bits + scale_bits + tmode_bits;
            coding.push(ChannelCoding { bhuff, shuff, sel });
        }
        (coding, fixed - abits_worst + actual)
    };

    let (mut coding, mut used) = select_all(&mut plan);
    debug_assert!(
        used <= budget_total,
        "pass-1 plan overflow: {used} > {budget_total}"
    );

    // --- Pass 2: re-spend the entropy saving --------------------------
    // The second pass adds worst-case-priced steps up to the frame's
    // remaining bits, then re-selects the books. A step can still
    // invalidate a cheaper earlier choice (a band pushed past ABITS 12
    // forfeits the Huffman BHUFF, a new band can flip a whole SEL
    // family back to block codes), so the newest steps are rolled
    // back until the exact total fits.
    let slack = budget_total.saturating_sub(used);
    if slack > 0 {
        let mut steps = allocate(&mut plan, &mut noise, &ladder, &peak, slack);
        if !steps.is_empty() {
            loop {
                let (c, u) = select_all(&mut plan);
                coding = c;
                used = u;
                if used <= budget_total {
                    break;
                }
                let (ch, n, prev_abits, prev_noise) = steps
                    .pop()
                    .expect("pass-1 plan fitted, so rolling back all steps fits");
                plan[ch][n].abits = prev_abits;
                noise[ch][n] = prev_noise;
            }
        }
    }
    debug_assert!(used <= budget_total);

    // --- Emission ---------------------------------------------------
    let header = frame_header(config, frame_bytes);
    let header_bytes = encode_frame_header_be(&header).expect("encoder header fields are bounded");
    let mut w = BitWriter::from_bytes(header_bytes);

    // §5.3.2 primary audio coding header (Table 5-21).
    w.push(0, 4); // SUBFS = 0 -> 1 subframe
    w.push(channels as u32 - 1, 3); // PCHS
    for _ in 0..channels {
        w.push(NUM_SUBBAND as u32 - 2, 5); // SUBS -> nSUBS = 32
    }
    for _ in 0..channels {
        w.push(n_vqsub as u32 - 1, 5); // VQSUB
    }
    for _ in 0..channels {
        w.push(0, 3); // JOINX = 0
    }
    for _ in 0..channels {
        w.push(0, 2); // THUFF = 0 (A4)
    }
    for c in &coding {
        w.push(u32::from(c.shuff), 3);
    }
    for c in &coding {
        w.push(u32::from(c.bhuff), 3);
    }
    // SEL planes.
    for c in &coding {
        w.push(u32::from(c.sel[0]), 1);
    }
    for n in 1..5 {
        for c in &coding {
            w.push(u32::from(c.sel[n]), 2);
        }
    }
    for n in 5..10 {
        for c in &coding {
            w.push(u32::from(c.sel[n]), 3);
        }
    }
    // ADJ: transmitted for every Huffman SEL (index 0 = ×1.0).
    for c in &coding {
        if c.sel[0] == 0 {
            w.push(0, 2);
        }
    }
    for n in 1..5 {
        for c in &coding {
            if c.sel[n] < 3 {
                w.push(0, 2);
            }
        }
    }
    for n in 5..10 {
        for c in &coding {
            if c.sel[n] < 7 {
                w.push(0, 2);
            }
        }
    }
    // CPF = 0 -> no AHCRC.

    // §5.4.1 side information (Table 5-28).
    w.push(N_SSC as u32 - 1, 2); // SSC
    w.push(0, 3); // PSC
    for _ in 0..channels {
        for _ in 0..NUM_SUBBAND {
            w.push(0, 1); // PMODE = 0
        }
    }
    // ABITS.
    for (ch, c) in coding.iter().enumerate() {
        let book = AbitsCodebook::from_bhuff(c.bhuff).expect("chosen from valid codes");
        for band in plan[ch].iter().take(n_vqsub) {
            match abits_table(book) {
                Some(table) => {
                    let (code, len) =
                        huff_code(table, i16::from(band.abits)).expect("cost-checked");
                    w.push(code, len);
                }
                None => w.push(u32::from(band.abits), if c.bhuff == 5 { 4 } else { 5 }),
            }
        }
    }
    // TMODE (two subsubframes): transmitted for allocated bands.
    let a4_zero = huff_code(tmode_table(TmodeCodebook::A4), 0).expect("A4 has symbol 0");
    for ch_plan in plan.iter() {
        for band in ch_plan.iter().take(n_vqsub) {
            if band.abits > 0 {
                w.push(a4_zero.0, a4_zero.1);
            }
        }
    }
    // SCALES.
    for (ch, c) in coding.iter().enumerate() {
        let cb = ScalesCodebook::from_shuff(c.shuff).expect("chosen from valid codes");
        let mut sum = 0i32;
        for band in plan[ch].iter().take(n_vqsub) {
            if band.abits == 0 {
                continue;
            }
            match scales_table(cb) {
                Some(table) => {
                    let diff = i32::from(band.scale_index) - sum;
                    let (code, len) = huff_code(table, diff as i16).expect("cost-checked");
                    w.push(code, len);
                    sum = i32::from(band.scale_index);
                }
                None => w.push(u32::from(band.scale_index), 7),
            }
        }
    }
    // Tail: JOINX = 0, DYNF = 0, CPF = 0 -> nothing.

    // §5.5 audio data. nVQSUB == nSUBS -> no HFREQ phase. LFE next.
    if let Some(lfe) = lfe_decimated {
        emit_lfe(&mut w, lfe);
    }
    for ssf in 0..N_SSC {
        for (ch, c) in coding.iter().enumerate() {
            for band in plan[ch].iter().take(n_vqsub) {
                emit_band_subsubframe(&mut w, band, c, ssf);
            }
        }
    }
    // DSYNC at end of the (single) subframe.
    w.push(0xFFFF, 16);

    debug_assert!(
        w.bit_len() <= budget_total,
        "frame overflow: {} bits > {budget_total}",
        w.bit_len()
    );
    w.pad_to_bytes(frame_bytes);
    w.into_bytes()
}

/// Emit one band's 8 quantization indices of one subsubframe under
/// the channel's `SEL` for that family.
fn emit_band_subsubframe(w: &mut BitWriter, band: &BandPlan, coding: &ChannelCoding, ssf: usize) {
    let a = band.abits;
    if a == 0 {
        return;
    }
    let idx = &band.idx[ssf * SAMPLES_PER_SUBSUBFRAME..(ssf + 1) * SAMPLES_PER_SUBSUBFRAME];
    let sel = if a <= 10 {
        coding.sel[a as usize - 1]
    } else {
        0
    };
    if let Some(book) = AudioHuffCodebook::from_abits_sel(a, sel) {
        let table = table_for(book);
        for &v in idx {
            let (code, len) = huff_code(table, v as i16).expect("cost-checked");
            w.push(code, len);
        }
    } else if a <= 7 {
        let levels = u32::from(QUANT_LEVELS[a as usize]);
        let offset = block_code_offset(levels);
        let width = BLOCK_WORD_BITS[a as usize];
        for block in idx.chunks_exact(4) {
            let mut code = 0u32;
            for &v in block.iter().rev() {
                code = code * levels + (v + offset) as u32;
            }
            w.push(code, width);
        }
    } else {
        let width = u32::from(a) - 3;
        for &v in idx {
            w.push_signed(v, width);
        }
    }
}

/// Emit the §5.5 LFE phase: 8-bit two's-complement decimated samples
/// plus the 7-bit-RMS scale index (8 bits on the wire).
fn emit_lfe(w: &mut BitWriter, decimated: &[f64]) {
    let peak = decimated.iter().fold(0.0_f64, |a, &v| a.max(v.abs()));
    let need = peak / (127.0 * LFE_SCALE_STEP);
    let mut scale_index = 124usize;
    for (idx, &v) in RMS_7BIT.iter().enumerate().take(125) {
        if f64::from(v) >= need {
            scale_index = idx;
            break;
        }
    }
    let recon = f64::from(RMS_7BIT[scale_index]) * LFE_SCALE_STEP;
    for &v in decimated {
        let q = (v / recon).round().clamp(-127.0, 127.0) as i32;
        w.push_signed(q, 8);
    }
    w.push(scale_index as u32, 8);
}

/// Convert one raw-BE frame to the configured on-wire word format.
fn to_wire(mut frame: Vec<u8>, encoding: SyncWordEncoding) -> Vec<u8> {
    match encoding {
        SyncWordEncoding::RawBigEndian => frame,
        SyncWordEncoding::RawLittleEndian => {
            for pair in frame.chunks_exact_mut(2) {
                pair.swap(0, 1);
            }
            frame
        }
        SyncWordEncoding::FourteenBitBigEndian => {
            crate::unpack14::pack_16bit_to_14bit(&frame, FourteenBitByteOrder::BigEndian).0
        }
        SyncWordEncoding::FourteenBitLittleEndian => {
            crate::unpack14::pack_16bit_to_14bit(&frame, FourteenBitByteOrder::LittleEndian).0
        }
    }
}

/// Build the §5.3.1 header for one normal frame.
fn frame_header(config: &EncoderConfig, frame_bytes: usize) -> DtsFrameHeader {
    DtsFrameHeader {
        sync_word_encoding: SyncWordEncoding::RawBigEndian,
        frame_type: FrameType::Normal,
        sample_count_per_block: 32,
        crc_present: false,
        blocks_per_frame: (ENCODER_FRAME_SAMPLES / 32 - 1) as u8,
        frame_size_bytes: frame_bytes as u16,
        amode: config.amode(),
        sfreq_index: sfreq_code(config.sample_rate).expect("validated at construction"),
        rate_index: config.rate_index,
        downmix: false,
        dynamic_range: false,
        time_stamp: false,
        aux_data: false,
        hdcd: false,
        ext_descr: 0,
        ext_coding: false,
        aspf: false,
        lfe: if config.lfe {
            LfeMode::Mode2
        } else {
            LfeMode::None
        },
        predictor_history: true,
        header_crc: None,
        multirate_inter: config.filter == FilterBankSelection::PerfectReconstruction,
        version: 7,
        copy_history: 0,
        source_pcm_resolution_index: 0,
        front_sum: false,
        surround_sum: false,
        dialog_normalization: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_maps_rates_and_layouts() {
        let c = EncoderConfig::new(48_000, 2).unwrap();
        assert_eq!(c.rate_index, 15);
        assert_eq!(c.bit_rate_bps().unwrap(), 768_000);
        assert_eq!(c.frame_bytes().unwrap(), 1024);
        assert_eq!(c.amode(), 2);
        let c = EncoderConfig::new(48_000, 5)
            .unwrap()
            .with_bit_rate(700_000)
            .unwrap();
        assert_eq!(c.rate_index, 15); // 768k is the smallest ≥ 700k
        assert_eq!(c.amode(), 9);
        assert!(matches!(
            EncoderConfig::new(47_000, 2),
            Err(EncodeError::UnsupportedSampleRate { .. })
        ));
        assert!(matches!(
            EncoderConfig::new(48_000, 6),
            Err(EncodeError::UnsupportedChannelCount { .. })
        ));
        assert!(matches!(
            EncoderConfig::new(48_000, 2)
                .unwrap()
                .with_bit_rate(2_000_000),
            Err(EncodeError::BitRateTooHigh { .. })
        ));
        assert!(matches!(
            EncoderConfig::new(48_000, 1).unwrap().with_bit_rate(1),
            Err(EncodeError::BitRateTooLow { .. })
        ));
    }

    #[test]
    fn qmax_matches_level_tables() {
        for (a, expect) in [(1, 1), (2, 2), (3, 3), (4, 4), (5, 6), (6, 8), (7, 12)] {
            assert_eq!(qmax(a), expect);
            assert_eq!(
                i64::from(QUANT_LEVELS[a as usize]),
                2 * i64::from(qmax(a)) + 1
            );
        }
        assert_eq!(qmax(8), 15);
        assert_eq!(qmax(9), 31);
        assert_eq!(qmax(26), (1 << 22) - 1);
    }

    #[test]
    fn every_huffman_family_covers_the_quantizer_range() {
        // Every §D.5 audio book the SEL search may pick must have a
        // code for every index the quantizer can emit at that ABITS.
        for a in 1..=10u8 {
            let q = qmax(a);
            for s in 0..CODEBOOK_GROUP_SIZE[a as usize] - 1 {
                let book = AudioHuffCodebook::from_abits_sel(a, s).expect("book exists");
                let table = table_for(book);
                for v in -q..=q {
                    assert!(
                        huff_code(table, v as i16).is_some(),
                        "ABITS {a} SEL {s}: no code for {v}"
                    );
                }
            }
        }
    }

    #[test]
    fn block_code_emission_matches_decoder() {
        let levels = 3u32;
        let offset = block_code_offset(levels);
        for pattern in 0..81u32 {
            let mut idx = [0i32; 4];
            let mut p = pattern;
            for slot in idx.iter_mut() {
                *slot = (p % 3) as i32 - offset;
                p /= 3;
            }
            let mut code = 0u32;
            for &v in idx.iter().rev() {
                code = code * levels + (v + offset) as u32;
            }
            let mut out = [0i32; 4];
            crate::block_code::decode_block_code(code, levels, &mut out).unwrap();
            assert_eq!(out, idx);
        }
    }

    #[test]
    fn scale_index_covers_peak() {
        let t = StepSizeTable::Lossy;
        for abits in 1..=MAX_ABITS {
            for &peak in &[0.5, 12.0, 3_000.0, 800_000.0, 7.6e6] {
                for table in [&RMS_7BIT[..], &RMS_6BIT[..]] {
                    let valid = valid_rms(table);
                    let idx = scale_index_in(abits, peak, t, valid);
                    let step = t.step_size(abits).unwrap();
                    let span = f64::from(qmax(abits)) * step * f64::from(table[idx as usize]);
                    assert!(
                        span >= peak || usize::from(idx) == valid.len() - 1,
                        "a={abits} peak={peak}"
                    );
                }
            }
        }
    }
}
