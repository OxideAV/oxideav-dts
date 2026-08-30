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
use crate::inverse_adpcm::NUM_ADPCM_COEFF;
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

/// `(code, length)` of `symbol` in a `(symbol, length, code)` book.
fn huff_code(table: &[(i16, u8, u16)], symbol: i16) -> Option<(u32, u32)> {
    table
        .iter()
        .find(|&&(sym, _, _)| sym == symbol)
        .map(|&(_, len, code)| (u32::from(code), u32::from(len)))
}

/// A §5.7.1 dynamic-downmix specification: the Table 5-32 output
/// group and the `out × in` coefficient matrix (out-major, inputs in
/// the frame's channel order — Table 5-4 primaries then the LFE
/// channel when present). Coefficients are snapped to the §D.11
/// table on emission (`0.0` codes as "no contribution").
#[derive(Debug, Clone, PartialEq)]
pub struct DownmixSpec {
    /// Table 5-32 downmix group.
    pub downmix_type: crate::DownmixType,
    /// `output_channel_count × input_channel_count` gains, out-major.
    pub coefficients: Vec<f64>,
}

/// Encoder configuration.
#[derive(Debug, Clone, PartialEq)]
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
    /// Embedded dynamic-range coefficient (`DYNF = 1`, §5.4.1 `RANGE`):
    /// a constant gain in dB the decoder applies after reconstruction,
    /// coded as the 8-bit signed Q2 value of §D.4 (±31.75 dB in
    /// 0.25 dB steps). `None` emits `DYNF = 0`.
    pub dynamic_range_db: Option<f64>,
    /// Joint intensity coding (`JOINX`, §5.3.2 / §C.2.3): when set,
    /// the second channel of each (L, R) / (SL, SR) pair carries no
    /// subbands from this index up and the decoder copies them from
    /// the first channel of the pair, scaled per band by `JOIN_SCALES`.
    /// With the interoperable linear `JOIN_SHUFF` the §D.3 factor is
    /// bounded below by unity (see [`CoreEncoder`] docs), so this is
    /// an opt-in tool; `None` disables it.
    pub joint_intensity_start: Option<usize>,
    /// Auxiliary-data chunk (`AUXF = 1`, §5.7.1) carrying dynamic
    /// downmix coefficients under a Table 5-32 type, protected by
    /// `nAUXCRC16`. `None` emits `AUXF = 0`.
    pub downmix: Option<DownmixSpec>,
    /// Enable §5.4.1 `PMODE` ADPCM prediction (§C.2.2, §D.10.1 book)
    /// on bands where a 4th-order predictor over the reconstructed
    /// history removes ≥ 3 dB of energy. Default on.
    pub adpcm: bool,
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
    /// The downmix coefficient matrix does not match the layout.
    DownmixShapeMismatch {
        /// `output_channel_count × input planes` expected.
        expected: usize,
        /// Coefficients supplied.
        got: usize,
    },
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
            Self::DownmixShapeMismatch { expected, got } => {
                write!(f, "downmix matrix needs {expected} coefficients, got {got}")
            }
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
            dynamic_range_db: None,
            joint_intensity_start: None,
            downmix: None,
            adpcm: true,
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

    /// Set the embedded dynamic-range gain (`None` = `DYNF = 0`).
    #[must_use]
    pub fn with_dynamic_range_db(mut self, db: Option<f64>) -> Self {
        self.dynamic_range_db = db;
        self
    }

    /// Enable joint intensity coding from subband `start` (2..=31) up,
    /// or disable it with `None`.
    #[must_use]
    pub fn with_joint_intensity_start(mut self, start: Option<usize>) -> Self {
        self.joint_intensity_start = start.map(|k| k.clamp(2, NUM_SUBBAND - 1));
        self
    }

    /// Attach a §5.7.1 dynamic-downmix auxiliary chunk to every frame.
    #[must_use]
    pub fn with_downmix(mut self, downmix: Option<DownmixSpec>) -> Self {
        self.downmix = downmix;
        self
    }

    /// Enable/disable ADPCM prediction.
    #[must_use]
    pub fn with_adpcm(mut self, adpcm: bool) -> Self {
        self.adpcm = adpcm;
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
    /// Per channel, per band: the last four *reconstructed* subband
    /// samples of the previous frame — the decoder's §C.2.2 ADPCM
    /// history (kept for every band, as the decoder does).
    history: Vec<[[f64; NUM_ADPCM_COEFF]; NUM_SUBBAND]>,
}

impl CoreEncoder {
    /// Build an encoder for `config`.
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let frame_bytes = config.frame_bytes()?;
        if let Some(d) = &config.downmix {
            let expect = d.downmix_type.output_channel_count() * config.plane_count();
            if d.coefficients.len() != expect || expect == 0 {
                return Err(EncodeError::DownmixShapeMismatch {
                    expected: expect,
                    got: d.coefficients.len(),
                });
            }
        }
        Ok(Self {
            qmf: QmfAnalysis::new(config.filter),
            lfe: config
                .lfe
                .then(|| LfeAnalysis::new(LfeInterpolationSelection::Decimation64)),
            buf: vec![Vec::new(); config.plane_count()],
            consumed: 0,
            history: vec![[[0.0; NUM_ADPCM_COEFF]; NUM_SUBBAND]; config.channels],
            frame_bytes,
            config,
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
            &mut self.history,
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
    /// Chosen `ABITS` (0 = not quantized; VQ bands are also 0).
    abits: u8,
    /// `TMODE`: 0 = one scale factor, 1 = a transient — the second
    /// subsubframe uses the second scale factor.
    tmode: u8,
    /// Chosen §D.1 scale indices (in the table the channel's `SHUFF`
    /// implies) for the two subsubframe halves; `[1]` is only
    /// transmitted when `tmode > 0`. For a VQ band `[0]` is the
    /// §5.5 `HFREQ` scale.
    scale_index: [u8; 2],
    /// Quantization indices for the frame — valid when `abits > 0`.
    idx: [i32; SAMPLES_PER_BAND],
    /// §D.10.2 vector index for a high-frequency-VQ band.
    vq_index: u16,
    /// §D.10.1 predictor (`PVQ` index + its coefficients) when the
    /// band is ADPCM-coded (`PMODE = 1`).
    pvq: Option<(u16, [f64; NUM_ADPCM_COEFF])>,
    /// Open-loop residual peak of the predictor (drives the scale
    /// choice of a predicted band).
    resid_peak: f64,
    /// Reconstructed samples (what the decoder will hold), for the
    /// next frame's history.
    recon: [f64; SAMPLES_PER_BAND],
}

impl Default for BandPlan {
    fn default() -> Self {
        Self {
            abits: 0,
            tmode: 0,
            scale_index: [0; 2],
            idx: [0; SAMPLES_PER_BAND],
            vq_index: 0,
            pvq: None,
            resid_peak: 0.0,
            recon: [0.0; SAMPLES_PER_BAND],
        }
    }
}

/// Per-channel structure + entropy selection.
#[derive(Debug, Clone)]
struct ChannelCoding {
    /// `nSUBS`: active subbands (2..=32).
    n_subs: usize,
    /// `nVQSUB`: first high-frequency-VQ subband (1..=`n_subs`).
    n_vqsub: usize,
    /// `BHUFF` selector.
    bhuff: u8,
    /// `SHUFF` selector.
    shuff: u8,
    /// `THUFF` selector.
    thuff: u8,
    /// `SEL[ch][ABITS-1]` for `ABITS 1..=10`.
    sel: [u8; 10],
    /// `JOINX` (0 = none, else source channel + 1).
    joinx: u8,
    /// Raw 7-bit `JOIN_SCALES` values (linear `JOIN_SHUFF = 6`; the
    /// decoder adds 64 before the §D.3 lookup), one per joint band.
    join_scales: Vec<u8>,
}

/// Band peaks below this (in the §C.2.5 domain, ≈ −120 dBFS) are
/// treated as silent for `nSUBS` trimming.
const SILENCE_FLOOR: f64 = 8.0;

/// Peak ratio between the two subsubframe halves above which a band
/// is flagged `TMODE = 1` (two scale factors): 12 dB.
const TRANSIENT_RATIO: f64 = 4.0;

/// Smallest §D.1 level in `table` (a valid-prefix slice) whose
/// quantizer span covers `peak` at `abits`, i.e. `qmax·step·RMS ≥
/// peak`; the top valid level when nothing covers it.
fn scale_index_in(abits: u8, peak: f64, step_table: StepSizeTable, table: &[u32]) -> u8 {
    let step = step_table
        .step_size(abits)
        .expect("allocator only uses valid ABITS");
    rms_index_covering(peak / (f64::from(qmax(abits)) * step), table)
}

/// Smallest level of `table` at or above `need` (top level if none).
fn rms_index_covering(need: f64, table: &[u32]) -> u8 {
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

/// Quantization-noise power estimate (per sample) at `abits` for one
/// scale-factor span covering `peak`, on the 7-bit grid.
fn noise_power(abits: u8, peak: f64, step_table: StepSizeTable) -> f64 {
    let idx = scale_index_in(abits, peak, step_table, valid_rms(&RMS_7BIT));
    let step = step_table.step_size(abits).expect("valid ABITS");
    let q = step * f64::from(RMS_7BIT[idx as usize]);
    q * q / 12.0
}

/// Side-information bits a band pays when it first becomes active:
/// `TMODE` (1 bit for the A4 zero code, 2 for a transient) plus one
/// or two 7-bit scale factors.
fn activation_side_bits(tmode: u8) -> usize {
    if tmode > 0 {
        2 + 14
    } else {
        1 + 7
    }
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
                        + if cur == 0 {
                            activation_side_bits(plan[ch][n].tmode)
                                + if plan[ch][n].pvq.is_some() { 12 } else { 0 }
                        } else {
                            0
                        };
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

/// Margin applied to a predictor's open-loop residual peak when
/// choosing the closed-loop scale factor.
const RESIDUAL_MARGIN: f64 = 1.25;

/// Minimum open-loop prediction gain (energy ratio) for a band to be
/// ADPCM-coded: 3 dB.
const MIN_PREDICTION_GAIN: f64 = 2.0;

/// Find a §D.10.1 predictor for one band's 16 samples given the
/// decoder-side history: a 4th-order least-squares fit over the
/// reconstructed past, quantized to the book by a coarse
/// coefficient-distance pre-selection and an exact residual-energy
/// pick. Returns `(index, coefficients, residual_peak)` when the book
/// vector removes at least [`MIN_PREDICTION_GAIN`] of the energy.
#[allow(clippy::needless_range_loop)] // index-heavy normal-equation arithmetic
fn lpc_candidate(
    x: &[f64; SAMPLES_PER_BAND],
    hist: &[f64; NUM_ADPCM_COEFF],
    book: &crate::AdpcmVqCodebook,
) -> Option<(u16, [f64; NUM_ADPCM_COEFF], f64)> {
    // Past sample `m - k - 1` for k in 0..4 (history for m < k+1).
    let past = |m: usize, k: usize| -> f64 {
        if m > k {
            x[m - k - 1]
        } else {
            hist[NUM_ADPCM_COEFF + m - k - 1]
        }
    };
    let mut r0 = 0.0_f64;
    let mut r = [0.0_f64; NUM_ADPCM_COEFF];
    let mut rr = [[0.0_f64; NUM_ADPCM_COEFF]; NUM_ADPCM_COEFF];
    for m in 0..SAMPLES_PER_BAND {
        r0 += x[m] * x[m];
        for i in 0..NUM_ADPCM_COEFF {
            let pi = past(m, i);
            r[i] += x[m] * pi;
            for j in 0..NUM_ADPCM_COEFF {
                rr[i][j] += pi * past(m, j);
            }
        }
    }
    if r0 <= 0.0 {
        return None;
    }
    // Residual energy of coefficient vector c: r0 − 2c·r + cᵀRc.
    let energy = |c: &[f64; NUM_ADPCM_COEFF]| -> f64 {
        let mut e = r0;
        for i in 0..NUM_ADPCM_COEFF {
            e -= 2.0 * c[i] * r[i];
            for j in 0..NUM_ADPCM_COEFF {
                e += c[i] * rr[i][j] * c[j];
            }
        }
        e
    };
    // Unquantized least-squares solution (regularized).
    let mut a = rr;
    let mut b = r;
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += 1e-9 * r0 + 1e-300;
    }
    for col in 0..NUM_ADPCM_COEFF {
        let pivot = (col..NUM_ADPCM_COEFF)
            .max_by(|&p, &q| a[p][col].abs().partial_cmp(&a[q][col].abs()).unwrap())
            .unwrap();
        a.swap(col, pivot);
        b.swap(col, pivot);
        let d = a[col][col];
        if d.abs() < 1e-300 {
            return None;
        }
        for j in 0..NUM_ADPCM_COEFF {
            a[col][j] /= d;
        }
        b[col] /= d;
        for row in 0..NUM_ADPCM_COEFF {
            if row != col {
                let f = a[row][col];
                if f != 0.0 {
                    for j in 0..NUM_ADPCM_COEFF {
                        a[row][j] -= f * a[col][j];
                    }
                    b[row] -= f * b[col];
                }
            }
        }
    }
    if energy(&b) > r0 / MIN_PREDICTION_GAIN {
        return None;
    }
    // Coarse: the 24 book vectors nearest to the unquantized solution.
    let mut coarse: Vec<(f64, u16)> = (0..crate::ADPCM_VQ_BOOK_SIZE as u16)
        .map(|i| {
            let c = book.coefficients(i);
            let d: f64 = c.iter().zip(b.iter()).map(|(p, q)| (p - q) * (p - q)).sum();
            (d, i)
        })
        .collect();
    coarse.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    let mut best: Option<(f64, u16)> = None;
    for &(_, i) in coarse.iter().take(24) {
        let e = energy(book.coefficients(i));
        if best.map_or(true, |(be, _)| e < be) {
            best = Some((e, i));
        }
    }
    let (e, index) = best?;
    if e > r0 / MIN_PREDICTION_GAIN {
        return None;
    }
    let coeffs = *book.coefficients(index);
    let mut peak = 0.0_f64;
    for m in 0..SAMPLES_PER_BAND {
        let mut pred = 0.0;
        for (k, c) in coeffs.iter().enumerate() {
            pred += c * past(m, k);
        }
        peak = peak.max((x[m] - pred).abs());
    }
    Some((index, coeffs, peak))
}

/// Quantize every active band of one channel against its chosen
/// scale grid (`table` = the valid prefix of the §D.1 table its
/// `SHUFF` implies), honouring the per-half scale of transient bands.
#[allow(clippy::needless_range_loop)] // closed-loop recursion indexes the reconstruction it writes
fn quantize_channel(
    plan: &mut [BandPlan; NUM_SUBBAND],
    rows: &[[f64; NUM_SUBBAND]],
    peak: &[[f64; NUM_SUBBAND]; 2],
    history: &[[f64; NUM_ADPCM_COEFF]; NUM_SUBBAND],
    step_table: StepSizeTable,
    table: &[u32],
) {
    for (n, band) in plan.iter_mut().enumerate() {
        if band.abits == 0 {
            band.pvq = None;
            continue;
        }
        let step = step_table.step_size(band.abits).expect("valid ABITS");
        let q = qmax(band.abits);
        if let Some((_, coeffs)) = band.pvq {
            // Closed-loop ADPCM: predict from the *reconstructed* past
            // exactly as the decoder's §C.2.2 loop will, quantize the
            // residual, reconstruct.
            let sc = scale_index_in(
                band.abits,
                band.resid_peak * RESIDUAL_MARGIN,
                step_table,
                table,
            );
            band.scale_index = [sc, sc];
            let recon_step = step * f64::from(table[sc as usize]);
            for m in 0..SAMPLES_PER_BAND {
                let mut pred = 0.0_f64;
                for (k, c) in coeffs.iter().enumerate() {
                    let past = if m > k {
                        band.recon[m - k - 1]
                    } else {
                        history[n][NUM_ADPCM_COEFF + m - k - 1]
                    };
                    pred += c * past;
                }
                let e = rows[m][n] - pred;
                let idx = (e / recon_step).round().clamp(f64::from(-q), f64::from(q)) as i32;
                band.idx[m] = idx;
                // Mirror the decoder's accumulation order.
                let mut acc = recon_step * f64::from(idx);
                for (k, c) in coeffs.iter().enumerate() {
                    let past = if m > k {
                        band.recon[m - k - 1]
                    } else {
                        history[n][NUM_ADPCM_COEFF + m - k - 1]
                    };
                    acc += c * past;
                }
                band.recon[m] = acc;
            }
            continue;
        }
        if band.tmode > 0 {
            band.scale_index[0] = scale_index_in(band.abits, peak[0][n], step_table, table);
            band.scale_index[1] = scale_index_in(band.abits, peak[1][n], step_table, table);
        } else {
            let p = peak[0][n].max(peak[1][n]);
            band.scale_index[0] = scale_index_in(band.abits, p, step_table, table);
            band.scale_index[1] = band.scale_index[0];
        }
        for m in 0..SAMPLES_PER_BAND {
            let half = if band.tmode > 0 && m >= SAMPLES_PER_SUBSUBFRAME {
                1
            } else {
                0
            };
            let recon_step = step * f64::from(table[band.scale_index[half] as usize]);
            let idx = (rows[m][n] / recon_step)
                .round()
                .clamp(f64::from(-q), f64::from(q)) as i32;
            band.idx[m] = idx;
            band.recon[m] = recon_step * f64::from(idx);
        }
    }
}

/// Pick the §D.10.2 vector + §D.1 scale for a high-frequency-VQ band
/// from its 16 subframe samples: the book vector with the largest
/// normalized correlation, then the table level nearest its
/// least-squares gain. Returns `(vq_index, scale_index)`.
fn choose_hf_vq(
    rows: &[[f64; NUM_SUBBAND]],
    n: usize,
    book: &crate::HfVqCodebook,
    table: &[u32],
) -> (u16, u8) {
    let x: Vec<f64> = rows.iter().map(|r| r[n]).collect();
    let xx: f64 = x.iter().map(|v| v * v).sum();
    if xx <= 0.0 {
        return (0, 0);
    }
    let mut best = (0u16, 0.0_f64, 0.0_f64);
    for index in 0..crate::HFREQ_VQ_BOOK_SIZE as u16 {
        let e = book.vector(index);
        let (mut xe, mut ee) = (0.0_f64, 0.0_f64);
        for (a, b) in x.iter().zip(e.iter()) {
            xe += a * b;
            ee += b * b;
        }
        if ee <= 0.0 || xe <= 0.0 {
            continue;
        }
        let score = xe * xe / ee;
        if score > best.1 {
            best = (index, score, xe / ee);
        }
    }
    let gain = best.2;
    // Nearest table level in the log domain.
    let mut scale = 0usize;
    let mut err = f64::INFINITY;
    for (idx, &v) in table.iter().enumerate() {
        let d = (f64::from(v).ln() - gain.ln()).abs();
        if d < err {
            err = d;
            scale = idx;
        }
    }
    (best.0, scale as u8)
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
fn choose_sel(plan: &[BandPlan]) -> ([u8; 10], usize) {
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

/// Choose the cheapest `BHUFF` for a channel's `ABITS` vector
/// (`plan[..n_vqsub]`) and return `(bhuff, bits)`.
fn choose_bhuff(abits: &[BandPlan]) -> (u8, usize) {
    let n = abits.len();
    let mut best = (6u8, 5 * n);
    if abits.iter().all(|b| b.abits <= 15) && 4 * n < best.1 {
        best = (5, 4 * n);
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

/// Choose the cheapest `THUFF` for the transmitted `TMODE` symbols of
/// a channel (`plan[..n_vqsub]`, bands with `ABITS > 0`); returns
/// `(thuff, bits)`.
fn choose_thuff(abits: &[BandPlan]) -> (u8, usize) {
    let mut best = (0u8, usize::MAX);
    for (code, cb) in [
        (0u8, TmodeCodebook::A4),
        (1, TmodeCodebook::B4),
        (2, TmodeCodebook::C4),
        (3, TmodeCodebook::D4),
    ] {
        let table = tmode_table(cb);
        let bits: usize = abits
            .iter()
            .filter(|b| b.abits > 0)
            .map(|b| huff_code(table, i16::from(b.tmode)).expect("TMODE 0..=3").1 as usize)
            .sum();
        if bits < best.1 {
            best = (code, bits);
        }
    }
    best
}

/// Scale-factor indices a channel transmits, in §5.4.1 order: for
/// every quantized band its first (and, on a transient, second)
/// index, then one per high-frequency-VQ band.
fn scale_sequence(plan: &[BandPlan], n_vqsub: usize, n_subs: usize) -> Vec<u8> {
    let mut seq = Vec::new();
    for band in &plan[..n_vqsub] {
        if band.abits > 0 {
            seq.push(band.scale_index[0]);
            if band.tmode > 0 {
                seq.push(band.scale_index[1]);
            }
        }
    }
    for band in &plan[n_vqsub..n_subs] {
        seq.push(band.scale_index[0]);
    }
    seq
}

/// Bits of a channel's SCALES field under a Huffman `SHUFF` book,
/// given the 6-bit indices in transmission order.
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

/// The greedy allocator + structure/entropy selection + bitstream
/// emission for one frame.
fn encode_frame_bits(
    config: &EncoderConfig,
    frame_bytes: usize,
    rows: &[Vec<[f64; NUM_SUBBAND]>],
    lfe_decimated: Option<&[f64]>,
    history: &mut [[[f64; NUM_ADPCM_COEFF]; NUM_SUBBAND]],
) -> Vec<u8> {
    let channels = config.channels;
    let step_table = StepSizeTable::for_rate(config.rate_index);
    let hf_book = crate::HfVqCodebook::builtin();
    let adpcm_book = crate::AdpcmVqCodebook::builtin();

    // --- Statistics -------------------------------------------------
    // Band peak per subsubframe half, and mean-square over the frame.
    let mut peak = vec![[[0.0_f64; NUM_SUBBAND]; 2]; channels];
    let mut power = vec![[0.0_f64; NUM_SUBBAND]; channels];
    for ch in 0..channels {
        for (m, row) in rows[ch].iter().enumerate() {
            let half = m / SAMPLES_PER_SUBSUBFRAME;
            for (n, &v) in row.iter().enumerate() {
                peak[ch][half][n] = peak[ch][half][n].max(v.abs());
                power[ch][n] += v * v;
            }
        }
        for p in power[ch].iter_mut() {
            *p /= rows[ch].len() as f64;
        }
    }
    let band_peak: Vec<[f64; NUM_SUBBAND]> = (0..channels)
        .map(|ch| {
            let mut p = [0.0_f64; NUM_SUBBAND];
            for n in 0..NUM_SUBBAND {
                p[n] = peak[ch][0][n].max(peak[ch][1][n]);
            }
            p
        })
        .collect();

    // --- Transients + ADPCM candidates --------------------------------
    let mut plan = vec![[BandPlan::default(); NUM_SUBBAND]; channels];
    for ch in 0..channels {
        for n in 0..NUM_SUBBAND {
            let (a, b) = (peak[ch][0][n], peak[ch][1][n]);
            let (lo, hi) = (a.min(b), a.max(b));
            if lo > 0.0 && hi > TRANSIENT_RATIO * lo {
                plan[ch][n].tmode = 1;
            }
            if config.adpcm && band_peak[ch][n] > SILENCE_FLOOR {
                let mut x = [0.0_f64; SAMPLES_PER_BAND];
                for (m, row) in rows[ch].iter().enumerate() {
                    x[m] = row[n];
                }
                if let Some((index, coeffs, resid_peak)) =
                    lpc_candidate(&x, &history[ch][n], &adpcm_book)
                {
                    plan[ch][n].pvq = Some((index, coeffs));
                    plan[ch][n].resid_peak = resid_peak;
                    // A predicted band uses one scale over the whole
                    // subframe.
                    plan[ch][n].tmode = 0;
                }
            }
        }
    }

    // --- Fixed-cost accounting (worst case) --------------------------
    let header_bits = 104usize; // CPF = 0
                                // Coding header: 4 + 3 + per channel (5+5+3+2+3+3) + SEL planes
                                // (1 + 4·2 + 5·3 = 24) + ADJ (2 per Huffman family, ≤ 10).
    let coding_header_bits = 7 + (21 + 24 + 20) * channels;
    let lfe_bits = if lfe_decimated.is_some() {
        8 * ENCODER_FRAME_SAMPLES / 64 + 8
    } else {
        0
    };
    let dsync_bits = 16usize;
    let budget_total = frame_bytes * 8;
    // Optional information: the aux chunk (AUXCT + byte/DWORD padding
    // + the chunk) and the per-subframe RANGE coefficient.
    let aux_chunk: Option<Vec<u8>> = config
        .downmix
        .as_ref()
        .map(|d| build_aux_chunk(d, config.plane_count()));
    let aux_bits = aux_chunk.as_ref().map_or(0, |c| 6 + 7 + 24 + 8 * c.len());
    let range_bits = if config.dynamic_range_db.is_some() {
        8
    } else {
        0
    };
    // Worst-case side info: SSC/PSC, one PMODE bit and a 5-bit ABITS
    // field for all 32 bands of every channel.
    let side_worst = 5 + (1 + 5) * NUM_SUBBAND * channels;
    let fixed = header_bits
        + coding_header_bits
        + side_worst
        + lfe_bits
        + dsync_bits
        + aux_bits
        + range_bits;
    let budget = budget_total.saturating_sub(fixed);

    // --- Pass 1: allocation on worst-case widths ---------------------
    let ladder: Vec<[f64; 27]> = (0..channels)
        .flat_map(|ch| (0..NUM_SUBBAND).map(move |n| (ch, n)).collect::<Vec<_>>())
        .map(|(ch, n)| {
            let mut l = [0.0_f64; 27];
            l[0] = power[ch][n];
            for a in 1..=MAX_ABITS {
                let q = if plan[ch][n].pvq.is_some() {
                    noise_power(a, plan[ch][n].resid_peak * RESIDUAL_MARGIN, step_table)
                } else if plan[ch][n].tmode > 0 {
                    0.5 * (noise_power(a, peak[ch][0][n], step_table)
                        + noise_power(a, peak[ch][1][n], step_table))
                } else {
                    noise_power(a, band_peak[ch][n], step_table)
                };
                l[a as usize] = q.min(power[ch][n]);
            }
            l
        })
        .collect();
    let mut noise: Vec<[f64; NUM_SUBBAND]> = power.clone();
    // Joint channels are not allocated above the joint start band.
    let joint_start = config.joint_intensity_start.unwrap_or(NUM_SUBBAND);
    let mut alloc_peak = band_peak.clone();
    if config.joint_intensity_start.is_some() {
        for (ch, _) in joint_pairs(channels) {
            for p in alloc_peak[ch][joint_start..].iter_mut() {
                *p = 0.0;
            }
        }
    }
    let band_peak = alloc_peak;
    let _ = allocate(&mut plan, &mut noise, &ladder, &band_peak, budget);

    // --- Structure + entropy selection (exact costs) -----------------
    // Per channel: nVQSUB just above the last quantized band, nSUBS
    // just above the last non-silent band (VQ in between, subject to
    // `vq_cap[ch]` — lowered when the frame cannot afford every VQ
    // band), quantize on the chosen scale grid, pick the cheapest
    // SHUFF / SEL / BHUFF / THUFF, and return the exact bits used.
    let mut vq_cap = vec![NUM_SUBBAND; channels];
    // Joint intensity: (joint channel, source channel) pairs and the
    // start band; joint channels carry nothing from there up.
    let joint: Vec<Option<usize>> = {
        let mut j = vec![None; channels];
        if config.joint_intensity_start.is_some() {
            for (ch, src) in joint_pairs(channels) {
                j[ch] = Some(src);
            }
        }
        j
    };
    let select_all = |plan: &mut Vec<[BandPlan; NUM_SUBBAND]>,
                      vq_cap: &[usize]|
     -> (Vec<ChannelCoding>, usize) {
        let mut coding: Vec<ChannelCoding> = Vec::with_capacity(channels);
        let mut used =
            header_bits + coding_header_bits + lfe_bits + dsync_bits + 5 + aux_bits + range_bits;
        for ch in 0..channels {
            let n_vqsub = plan[ch]
                .iter()
                .rposition(|b| b.abits > 0)
                .map_or(1, |n| n + 1);
            let last_live = band_peak[ch]
                .iter()
                .rposition(|&p| p > SILENCE_FLOOR)
                .map_or(0, |n| n + 1);
            let cap = if joint[ch].is_some() {
                vq_cap[ch].min(joint_start)
            } else {
                vq_cap[ch]
            };
            let n_subs = n_vqsub.max(last_live).min(cap).max(n_vqsub).max(2);
            // VQ bands: vector + gain on the 7-bit grid first; the
            // SHUFF choice below may move them to the 6-bit grid.
            // Scale grid + SHUFF.
            let mut shuff = 6u8;
            let table7 = valid_rms(&RMS_7BIT);
            let mut best_bits = usize::MAX;
            let mut chosen_table: &[u32] = table7;
            let candidates: Vec<(u8, &[u32])> = if config.huffman_scales_allowed() {
                vec![(6, table7), (0, valid_rms(&RMS_6BIT))]
            } else {
                vec![(6, table7)]
            };
            for (code, table) in candidates {
                quantize_channel(
                    &mut plan[ch],
                    &rows[ch],
                    &peak[ch],
                    &history[ch],
                    step_table,
                    table,
                );
                for (n, band) in plan[ch].iter_mut().enumerate().take(n_subs).skip(n_vqsub) {
                    let (vq, sc) = choose_hf_vq(&rows[ch], n, &hf_book, table);
                    band.vq_index = vq;
                    band.scale_index = [sc, sc];
                }
                let seq = scale_sequence(&plan[ch], n_vqsub, n_subs);
                if code == 6 {
                    let bits = 7 * seq.len();
                    if bits < best_bits {
                        best_bits = bits;
                        shuff = 6;
                        chosen_table = table;
                    }
                } else {
                    for (hcode, cb) in [
                        (0u8, ScalesCodebook::Sa129),
                        (1, ScalesCodebook::Sb129),
                        (2, ScalesCodebook::Sc129),
                        (3, ScalesCodebook::Sd129),
                        (4, ScalesCodebook::Se129),
                    ] {
                        if let Some(bits) = scales_huffman_bits(&seq, cb) {
                            if bits < best_bits {
                                best_bits = bits;
                                shuff = hcode;
                                chosen_table = table;
                            }
                        }
                    }
                }
            }
            // Re-quantize on the winning grid (the last candidate
            // tried may not be the winner).
            quantize_channel(
                &mut plan[ch],
                &rows[ch],
                &peak[ch],
                &history[ch],
                step_table,
                chosen_table,
            );
            for (n, band) in plan[ch].iter_mut().enumerate().take(n_subs).skip(n_vqsub) {
                let (vq, sc) = choose_hf_vq(&rows[ch], n, &hf_book, chosen_table);
                band.vq_index = vq;
                band.scale_index = [sc, sc];
            }
            let scale_bits = best_bits;
            let (sel, audio_bits) = choose_sel(&plan[ch][..n_vqsub]);
            let (bhuff, abits_bits) = choose_bhuff(&plan[ch][..n_vqsub]);
            let (thuff, tmode_bits) = choose_thuff(&plan[ch][..n_vqsub]);
            // PMODE bit per active band + 12-bit PVQ per predicted band.
            let pmode_bits = n_subs
                + 12 * plan[ch][..n_vqsub]
                    .iter()
                    .filter(|b| b.abits > 0 && b.pvq.is_some())
                    .count();
            let vq_bits = 10 * (n_subs - n_vqsub);
            // Joint bands: JOIN_SHUFF (3) + one 7-bit factor per band
            // of the source above this channel's nSUBS.
            let (joinx, join_scales, join_bits) = match joint[ch] {
                Some(src) if n_subs < coding[src].n_subs => {
                    let scales: Vec<u8> = (n_subs..coding[src].n_subs)
                        .map(|n| join_scale_code(power[ch][n], power[src][n]))
                        .collect();
                    let bits = 3 + 7 * scales.len();
                    (src as u8 + 1, scales, bits)
                }
                _ => (0, Vec::new(), 0),
            };
            used += pmode_bits
                + abits_bits
                + tmode_bits
                + scale_bits
                + vq_bits
                + audio_bits
                + join_bits;
            coding.push(ChannelCoding {
                n_subs,
                n_vqsub,
                bhuff,
                shuff,
                thuff,
                sel,
                joinx,
                join_scales,
            });
        }
        (coding, used)
    };

    let (mut coding, mut used) = select_all(&mut plan, &vq_cap);
    // The VQ bands are extra over the worst-case plan; shed them from
    // the top until the frame fits (the quantized plan itself always
    // fits, so this terminates).
    while used > budget_total {
        let Some(ch) = (0..channels)
            .filter(|&ch| coding[ch].n_subs > coding[ch].n_vqsub)
            .max_by_key(|&ch| coding[ch].n_subs)
        else {
            break;
        };
        vq_cap[ch] = coding[ch].n_subs - 1;
        let (c, u) = select_all(&mut plan, &vq_cap);
        coding = c;
        used = u;
    }
    debug_assert!(
        used <= budget_total,
        "pass-1 plan overflow: {used} > {budget_total}"
    );

    // --- Pass 2: re-spend the entropy saving --------------------------
    // Add worst-case-priced steps up to the remaining bits, re-select,
    // and roll the newest steps back until the exact total fits (a
    // step can invalidate a cheaper earlier choice: a band pushed past
    // ABITS 12 forfeits the Huffman BHUFF, a new band can flip a whole
    // SEL family back to block codes, a newly quantized top band moves
    // nVQSUB).
    let slack = budget_total.saturating_sub(used);
    if slack > 0 {
        let mut steps = allocate(&mut plan, &mut noise, &ladder, &band_peak, slack);
        if !steps.is_empty() {
            loop {
                let (c, u) = select_all(&mut plan, &vq_cap);
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
    for c in &coding {
        w.push(c.n_subs as u32 - 2, 5); // SUBS
    }
    for c in &coding {
        w.push(c.n_vqsub as u32 - 1, 5); // VQSUB
    }
    for c in &coding {
        w.push(u32::from(c.joinx), 3); // JOINX
    }
    for c in &coding {
        w.push(u32::from(c.thuff), 2);
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
    for (ch, c) in coding.iter().enumerate() {
        for band in &plan[ch][..c.n_subs] {
            w.push(u32::from(band.pvq.is_some()), 1); // PMODE
        }
    }
    // PVQ: the §D.10.1 index of every predicted band.
    for (ch, c) in coding.iter().enumerate() {
        for band in &plan[ch][..c.n_subs] {
            if let Some((index, _)) = band.pvq {
                w.push(u32::from(index), crate::ADPCM_VQ_INDEX_BITS);
            }
        }
    }
    // ABITS.
    for (ch, c) in coding.iter().enumerate() {
        let book = AbitsCodebook::from_bhuff(c.bhuff).expect("chosen from valid codes");
        for band in &plan[ch][..c.n_vqsub] {
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
    // TMODE (two subsubframes): transmitted for quantized bands.
    for (ch, c) in coding.iter().enumerate() {
        let table = tmode_table(TmodeCodebook::from_thuff(c.thuff));
        for band in &plan[ch][..c.n_vqsub] {
            if band.abits > 0 {
                let (code, len) = huff_code(table, i16::from(band.tmode)).expect("TMODE 0..=3");
                w.push(code, len);
            }
        }
    }
    // SCALES: quantized bands (one or two), then the VQ bands.
    for (ch, c) in coding.iter().enumerate() {
        let cb = ScalesCodebook::from_shuff(c.shuff).expect("chosen from valid codes");
        let seq = scale_sequence(&plan[ch], c.n_vqsub, c.n_subs);
        let mut sum = 0i32;
        for idx in seq {
            match scales_table(cb) {
                Some(table) => {
                    let diff = i32::from(idx) - sum;
                    let (code, len) = huff_code(table, diff as i16).expect("cost-checked");
                    w.push(code, len);
                    sum = i32::from(idx);
                }
                None => w.push(u32::from(idx), 7),
            }
        }
    }
    // Tail: JOIN_SHUFF (all joint channels), JOIN_SCALES, RANGE.
    for c in &coding {
        if c.joinx > 0 {
            w.push(6, 3); // JOIN_SHUFF = 6 (7-bit linear)
        }
    }
    for c in &coding {
        for &raw in &c.join_scales {
            w.push(u32::from(raw), 7);
        }
    }
    if let Some(db) = config.dynamic_range_db {
        w.push(u32::from(range_code(db)), 8);
    }
    // CPF = 0 -> no SICRC.

    // §5.5 audio data: HFREQ VQ indices, LFE, subsubframes.
    for (ch, c) in coding.iter().enumerate() {
        for band in &plan[ch][c.n_vqsub..c.n_subs] {
            w.push(u32::from(band.vq_index), crate::HFREQ_VQ_INDEX_BITS);
        }
    }
    if let Some(lfe) = lfe_decimated {
        emit_lfe(&mut w, lfe);
    }
    for ssf in 0..N_SSC {
        for (ch, c) in coding.iter().enumerate() {
            for band in &plan[ch][..c.n_vqsub] {
                emit_band_subsubframe(&mut w, band, c, ssf);
            }
        }
    }
    // DSYNC at end of the (single) subframe.
    w.push(0xFFFF, 16);

    // §5.6 optional information: AUXCT, then the DWORD-aligned §5.7.1
    // chunk (the zero padding up to the boundary counts as AUXD).
    if let Some(chunk) = &aux_chunk {
        let after_count = w.bit_len() + 6;
        let aligned = after_count.div_ceil(32) * 32;
        let pad_bytes = (aligned - after_count) / 8;
        w.push((pad_bytes + chunk.len()) as u32, 6);
        for _ in 0..(aligned - after_count) {
            w.push(0, 1);
        }
        for &b in chunk {
            w.push(u32::from(b), 8);
        }
    }

    debug_assert!(
        w.bit_len() <= budget_total,
        "frame overflow: {} bits > {budget_total}",
        w.bit_len()
    );
    w.pad_to_bytes(frame_bytes);

    // --- Decoder-side history for the next frame -------------------
    // The decoder keeps the last four reconstructed samples of every
    // band (quantized: the dequantized / predicted values; VQ: the
    // scaled book vector; silent: zero).
    for (ch, c) in coding.iter().enumerate() {
        let table: &[u32] = if c.shuff == 6 { &RMS_7BIT } else { &RMS_6BIT };
        for (n, band) in plan[ch].iter().enumerate() {
            let mut h = [0.0_f64; NUM_ADPCM_COEFF];
            if n < c.n_vqsub && band.abits > 0 {
                h.copy_from_slice(&band.recon[SAMPLES_PER_BAND - NUM_ADPCM_COEFF..]);
            } else if n >= c.n_vqsub && n < c.n_subs {
                let scale = f64::from(table[band.scale_index[0] as usize]);
                let v = hf_book.vector(band.vq_index);
                for (k, slot) in h.iter_mut().enumerate() {
                    *slot = scale * v[SAMPLES_PER_BAND - NUM_ADPCM_COEFF + k];
                }
            }
            history[ch][n] = h;
        }
    }
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

/// Joint-intensity `(joint, source)` channel pairs for a Table 5-4
/// layout: `(R, L)` and `(SR, SL)` where present.
fn joint_pairs(channels: usize) -> Vec<(usize, usize)> {
    match channels {
        2 => vec![(1, 0)],         // L R
        3 => vec![(2, 1)],         // C L R
        4 => vec![(1, 0), (3, 2)], // L R SL SR
        5 => vec![(2, 1), (4, 3)], // C L R SL SR
        _ => Vec::new(),
    }
}

/// Raw 7-bit linear `JOIN_SCALES` value for a joint band: the §D.3
/// index nearest `sqrt(E_joint / E_source)` minus the decoder's +64
/// bias, bounded to the indices the linear selector can reach
/// (64..=128, i.e. factors ≥ 1.0).
fn join_scale_code(e_joint: f64, e_source: f64) -> u8 {
    if e_source <= 0.0 {
        return 0;
    }
    let ratio = (e_joint / e_source).sqrt();
    let mut best = (64usize, f64::INFINITY);
    for idx in 64..crate::JOIN_SCALE_LEN {
        let d = (crate::JOIN_SCALE_FACTOR[idx].ln() - ratio.max(1e-12).ln()).abs();
        if d < best.1 {
            best = (idx, d);
        }
    }
    (best.0 - 64) as u8
}

/// §5.4.1 `RANGE` code for a gain in dB: 8-bit signed Q2 (§D.4).
fn range_code(db: f64) -> u8 {
    ((db * 4.0).round().clamp(-128.0, 127.0) as i8) as u8
}

/// §D.11 downmix code for a gain: sign bit + (table index + 1); `0`
/// for a zero coefficient.
fn dmix_code(gain: f64) -> u16 {
    let mag = gain.abs();
    if mag < f64::from(crate::DMIX_TABLE[0]) / 65_536.0 {
        return 0;
    }
    let mut best = (0usize, f64::INFINITY);
    for (idx, &v) in crate::DMIX_TABLE.iter().enumerate() {
        let d = (f64::from(v) / 32_768.0 - mag).abs();
        if d < best.1 {
            best = (idx, d);
        }
    }
    let sign = if gain >= 0.0 { 0x100 } else { 0 };
    sign | (best.0 as u16 + 1)
}

/// Build the §5.7.1 auxiliary-data chunk (DWORD-aligned sync, no time
/// stamp, dynamic downmix codes, byte-aligned `nAUXCRC16` over the
/// bytes between the sync and the CRC).
fn build_aux_chunk(spec: &DownmixSpec, input_channels: usize) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.push(crate::AUX_SYNC_WORD, 32);
    w.push(0, 1); // bAUXTimeStampFlag
    w.push(1, 1); // bAUXDynamCoeffFlag
    w.push(u32::from(spec.downmix_type.code()), 3);
    let n_out = spec.downmix_type.output_channel_count();
    for out in 0..n_out {
        for inp in 0..input_channels {
            w.push(
                u32::from(dmix_code(spec.coefficients[out * input_channels + inp])),
                9,
            );
        }
    }
    let aligned = w.bit_len().div_ceil(8) * 8;
    for _ in w.bit_len()..aligned {
        w.push(0, 1);
    }
    let mut bytes = w.into_bytes();
    let crc = crate::dts_crc16(&bytes[4..]);
    bytes.extend_from_slice(&crc.to_be_bytes());
    bytes
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
        dynamic_range: config.dynamic_range_db.is_some(),
        time_stamp: false,
        aux_data: config.downmix.is_some(),
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
