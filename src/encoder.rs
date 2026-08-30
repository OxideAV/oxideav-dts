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
//! * per-band scale factors from the §D.1.2 7-bit RMS table
//!   (`SHUFF = 6`, linear), chosen as the smallest table level whose
//!   quantizer span covers the subframe's band peak (overload-free);
//! * mid-tread quantization against the §D.2.1 step sizes, carried as
//!   §D.6 block codes (`ABITS 1..=7`, the `V…` terminal SEL) or plain
//!   two's-complement fields (`ABITS ≥ 8`, the NFE terminal SEL);
//! * `ABITS` as linear 5-bit fields (`BHUFF = 6`), `TMODE` through
//!   the §D.5.2 `A4` book (`THUFF = 0`);
//! * a greedy rate-distortion bit allocator that spends the frame's
//!   byte budget (from the configured Table 5-7 target rate) one
//!   `ABITS` step at a time on the band with the best
//!   noise-reduction-per-bit, with exact bit accounting;
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

use crate::bitwriter::BitWriter;
use crate::cos_mod::NUM_SUBBAND;
use crate::filter_bank::FilterBankSelection;
use crate::header::{encode_frame_header_be, DtsFrameHeader, FrameType, LfeMode, SyncWordEncoding};
use crate::lfe_analysis::LfeAnalysis;
use crate::lfe_interp::LfeInterpolationSelection;
use crate::lfe_synth::LFE_SCALE_STEP;
use crate::qmf_analysis::QmfAnalysis;
use crate::side_info::{tmode_table, TmodeCodebook, RMS_7BIT};
use crate::step_size::{StepSizeTable, SAMPLES_PER_SUBSUBFRAME};

/// Samples per channel in one encoded frame (16 blocks of 32).
pub const ENCODER_FRAME_SAMPLES: usize = 512;

/// Subsubframes per frame (`SSC + 1`).
const N_SSC: usize = 2;

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

/// Largest quantization index magnitude per `ABITS`, for the SEL this
/// encoder emits: the mid-tread `(levels−1)/2` for the block-code
/// family, and the symmetric two's-complement bound for NFE.
fn qmax(abits: u8) -> i32 {
    match abits {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 => 8,
        7 => 12,
        a if (8..=26).contains(&a) => (1i32 << (a - 4)) - 1,
        _ => 0,
    }
}

/// Audio bits for one band over one frame (16 samples) at `abits`.
fn band_sample_bits(abits: u8) -> usize {
    match abits {
        0 => 0,
        1..=7 => 4 * BLOCK_WORD_BITS[abits as usize] as usize,
        a => 16 * (a as usize - 3),
    }
}

/// Side-information bits a band pays when it first becomes active
/// (`TMODE` 1 bit through the A4 zero code + 7-bit linear scale
/// factor). The 5-bit linear `ABITS` field is paid for every band.
const ACTIVE_BAND_SIDE_BITS: usize = 1 + 7;

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
        let bytes = (bytes & !1).min(16_384);
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
/// construction).
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
            let mut scaled: Vec<f64> = self.buf[ch]
                [start..start + ENCODER_FRAME_SAMPLES + crate::QMF_ANALYSIS_LOOKAHEAD]
                .iter()
                .map(|&v| v * ANALYSIS_INPUT_GAIN)
                .collect();
            debug_assert_eq!(
                scaled.len(),
                ENCODER_FRAME_SAMPLES + crate::QMF_ANALYSIS_LOOKAHEAD
            );
            // analyze() yields exactly rows_per_frame rows for this span.
            let ch_rows = self.qmf.analyze(&scaled);
            debug_assert_eq!(ch_rows.len(), rows_per_frame);
            scaled.clear();
            rows.push(ch_rows);
        }

        // (2) LFE decimation: 8 decimated samples for this frame.
        let lfe_decimated: Option<Vec<f64>> = self.lfe.as_ref().map(|dec| {
            let plane = &self.buf[channels];
            // decimate_count reads zeros outside the slice; feed it the
            // whole retained buffer aligned so output 0 is this frame's
            // first decimated sample.
            let span = &plane[start..];
            dec.decimate_count(span, ENCODER_FRAME_SAMPLES / 64)
                .iter()
                .map(|&v| v * LFE_INPUT_GAIN)
                .collect()
        });

        self.consumed += ENCODER_FRAME_SAMPLES;

        // (3) Per-band statistics + allocation + emission.
        encode_frame_bits(
            &self.config,
            self.frame_bytes,
            &rows,
            lfe_decimated.as_deref(),
        )
    }
}

/// Per-band planning record.
#[derive(Debug, Clone, Copy, Default)]
struct BandPlan {
    /// Chosen `ABITS`.
    abits: u8,
    /// Chosen §D.1.2 scale index (0..=124) — valid when `abits > 0`.
    scale_index: u8,
}

/// Choose the smallest §D.1.2 7-bit RMS level whose quantizer span
/// covers `peak` at `abits`, i.e. `qmax·step·RMS ≥ peak`.
fn scale_index_for(abits: u8, peak: f64, step_table: StepSizeTable) -> u8 {
    let step = step_table
        .step_size(abits)
        .expect("allocator only uses valid ABITS");
    let need = peak / (f64::from(qmax(abits)) * step);
    // RMS_7BIT indices 125..=127 are reserved.
    for (idx, &v) in RMS_7BIT.iter().enumerate().take(125) {
        if f64::from(v) >= need {
            return idx as u8;
        }
    }
    124
}

/// Quantization-noise power estimate (per sample) at `abits` with the
/// overload-free scale rule above.
fn noise_power(abits: u8, peak: f64, step_table: StepSizeTable) -> f64 {
    let idx = scale_index_for(abits, peak, step_table);
    let step = step_table.step_size(abits).expect("valid ABITS");
    let q = step * f64::from(RMS_7BIT[idx as usize]);
    q * q / 12.0
}

/// The greedy allocator + bitstream emission for one frame.
fn encode_frame_bits(
    config: &EncoderConfig,
    frame_bytes: usize,
    rows: &[Vec<[f64; NUM_SUBBAND]>],
    lfe_decimated: Option<&[f64]>,
) -> Vec<u8> {
    let channels = config.channels;
    let step_table = StepSizeTable::for_rate(config.rate_index);

    // --- Statistics -------------------------------------------------
    // Band peak and mean-square over the frame, per channel.
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

    // --- Fixed-cost accounting -------------------------------------
    let header_bits = 104usize; // CPF = 0
    let coding_header_bits = 7 + 45 * channels;
    let side_fixed_bits = 5 /* SSC + PSC */ + NUM_SUBBAND * channels /* PMODE */
        + 5 * NUM_SUBBAND * channels /* linear 5-bit ABITS, every band */;
    let lfe_bits = if lfe_decimated.is_some() {
        8 * ENCODER_FRAME_SAMPLES / 64 + 8
    } else {
        0
    };
    let dsync_bits = 16usize;
    let budget_total = frame_bytes * 8;
    let fixed = header_bits + coding_header_bits + side_fixed_bits + lfe_bits + dsync_bits;
    let mut budget = budget_total.saturating_sub(fixed);

    // --- Greedy allocation -----------------------------------------
    let mut plan = vec![[BandPlan::default(); NUM_SUBBAND]; channels];
    let mut noise = vec![[0.0_f64; NUM_SUBBAND]; channels];
    for ch in 0..channels {
        for n in 0..NUM_SUBBAND {
            noise[ch][n] = power[ch][n];
        }
    }
    // Precompute each band's full noise ladder once: the per-step
    // noise curve is not strictly monotone (the overload-free scale
    // is re-quantized up to the next §D.1.2 level at every ABITS), so
    // the greedy must be able to jump several ABITS at once past a
    // locally-flat step.
    let ladder: Vec<[f64; 27]> = (0..channels)
        .flat_map(|ch| (0..NUM_SUBBAND).map(move |n| (ch, n)).collect::<Vec<_>>())
        .map(|(ch, n)| {
            let mut l = [0.0_f64; 27];
            l[0] = power[ch][n];
            for a in 1..=26u8 {
                l[a as usize] = noise_power(a, peak[ch][n], step_table).min(power[ch][n]);
            }
            l
        })
        .collect();
    loop {
        // Pick the (channel, band, target ABITS) buying the largest
        // noise reduction per bit within the remaining budget.
        let mut best: Option<(usize, usize, u8, usize, f64)> = None;
        for ch in 0..channels {
            for n in 0..NUM_SUBBAND {
                let cur = plan[ch][n].abits;
                if cur >= 26 || peak[ch][n] <= 0.0 {
                    continue;
                }
                let l = &ladder[ch * NUM_SUBBAND + n];
                for next in (cur + 1)..=26 {
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
        plan[ch][n].abits = next;
        plan[ch][n].scale_index = scale_index_for(next, peak[ch][n], step_table);
        noise[ch][n] = ladder[ch * NUM_SUBBAND + n][next as usize];
        budget -= extra;
    }

    // --- Emission ---------------------------------------------------
    let header = frame_header(config, frame_bytes);
    let header_bytes = encode_frame_header_be(&header).expect("encoder header fields are bounded");
    debug_assert_eq!(header_bytes.len() * 8, header_bits);
    let mut w = BitWriter::from_bytes(header_bytes);

    // §5.3.2 primary audio coding header (Table 5-21).
    w.push(0, 4); // SUBFS = 0 -> 1 subframe
    w.push(channels as u32 - 1, 3); // PCHS
    for _ in 0..channels {
        w.push(NUM_SUBBAND as u32 - 2, 5); // SUBS -> nSUBS = 32
    }
    for _ in 0..channels {
        w.push(NUM_SUBBAND as u32 - 1, 5); // VQSUB -> nVQSUB = 32
    }
    for _ in 0..channels {
        w.push(0, 3); // JOINX = 0
    }
    for _ in 0..channels {
        w.push(0, 2); // THUFF = 0 (A4)
    }
    for _ in 0..channels {
        w.push(6, 3); // SHUFF = 6 (7-bit linear)
    }
    for _ in 0..channels {
        w.push(6, 3); // BHUFF = 6 (5-bit linear)
    }
    // SEL planes: terminal (block-code / NFE) selector per family.
    for _ in 0..channels {
        w.push(1, 1); // ABITS 1 -> V3
    }
    for _ in 1..5 {
        for _ in 0..channels {
            w.push(3, 2); // ABITS 2..=5 -> V…
        }
    }
    for _ in 5..10 {
        for _ in 0..channels {
            w.push(7, 3); // ABITS 6..=10 -> V… / NFE
        }
    }
    // Terminal SELs transmit no ADJ fields; CPF = 0 -> no AHCRC.
    debug_assert_eq!(w.bit_len(), header_bits + coding_header_bits);

    // §5.4.1 side information (Table 5-28).
    w.push(N_SSC as u32 - 1, 2); // SSC
    w.push(0, 3); // PSC
    for _ in 0..channels {
        for _ in 0..NUM_SUBBAND {
            w.push(0, 1); // PMODE = 0
        }
    }
    // No PVQ (all PMODE = 0).
    for ch_plan in plan.iter() {
        for band in ch_plan.iter() {
            w.push(u32::from(band.abits), 5);
        }
    }
    // TMODE: two subsubframes -> transmitted for allocated bands.
    let a4_zero = tmode_code(TmodeCodebook::A4, 0);
    for ch_plan in plan.iter() {
        for band in ch_plan.iter() {
            if band.abits > 0 {
                w.push(a4_zero.0, a4_zero.1);
            }
        }
    }
    // SCALES: linear 7-bit indices for allocated bands (no transients).
    for ch_plan in plan.iter() {
        for band in ch_plan.iter() {
            if band.abits > 0 {
                w.push(u32::from(band.scale_index), 7);
            }
        }
    }
    // Tail: JOINX = 0, DYNF = 0, CPF = 0 -> nothing.

    // §5.5 audio data. nVQSUB == nSUBS -> no HFREQ phase. LFE next.
    if let Some(lfe) = lfe_decimated {
        emit_lfe(&mut w, lfe);
    }
    for ssf in 0..N_SSC {
        for (ch, ch_plan) in plan.iter().enumerate() {
            for (n, band) in ch_plan.iter().enumerate() {
                emit_band_subsubframe(&mut w, band, step_table, &rows[ch], n, ssf);
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

/// The §D.5.2 code for `symbol` in `codebook` (encoder side of
/// `QTMODE`).
fn tmode_code(codebook: TmodeCodebook, symbol: u8) -> (u32, u32) {
    let entry = tmode_table(codebook)
        .iter()
        .find(|&&(sym, _, _)| sym == i16::from(symbol))
        .expect("TMODE symbols 0..=3 are complete");
    (u32::from(entry.2), u32::from(entry.1))
}

/// Quantize and emit one band's 8 samples of one subsubframe.
fn emit_band_subsubframe(
    w: &mut BitWriter,
    band: &BandPlan,
    step_table: StepSizeTable,
    rows: &[[f64; NUM_SUBBAND]],
    n: usize,
    ssf: usize,
) {
    if band.abits == 0 {
        return;
    }
    let step = step_table.step_size(band.abits).expect("valid ABITS");
    let recon = step * f64::from(RMS_7BIT[band.scale_index as usize]);
    let q = qmax(band.abits);
    let mut idx = [0i32; SAMPLES_PER_SUBSUBFRAME];
    for (m, slot) in idx.iter_mut().enumerate() {
        let s = rows[ssf * SAMPLES_PER_SUBSUBFRAME + m][n];
        *slot = (s / recon).round().clamp(f64::from(-q), f64::from(q)) as i32;
    }
    if band.abits <= 7 {
        let levels = u32::from(crate::audio_data::QUANT_LEVELS[band.abits as usize]);
        let offset = crate::block_code::block_code_offset(levels);
        let width = BLOCK_WORD_BITS[band.abits as usize];
        for block in idx.chunks_exact(4) {
            let mut code = 0u32;
            for &v in block.iter().rev() {
                code = code * levels + (v + offset) as u32;
            }
            w.push(code, width);
        }
    } else {
        let width = u32::from(band.abits) - 3;
        for &v in &idx {
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
        // Block-code families: (levels − 1) / 2.
        for (a, expect) in [(1, 1), (2, 2), (3, 3), (4, 4), (5, 6), (6, 8), (7, 12)] {
            assert_eq!(qmax(a), expect);
            assert_eq!(
                i64::from(crate::audio_data::QUANT_LEVELS[a as usize]),
                2 * i64::from(qmax(a)) + 1
            );
        }
        // NFE: symmetric within the (ABITS − 3)-bit two's-complement.
        assert_eq!(qmax(8), 15);
        assert_eq!(qmax(9), 31);
        assert_eq!(qmax(26), (1 << 22) - 1);
    }

    #[test]
    fn block_code_emission_matches_decoder() {
        // Every index pattern for ABITS 1 (V3, 3 levels) round-trips
        // through the crate's block-code decoder.
        let levels = 3u32;
        let offset = crate::block_code::block_code_offset(levels);
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
        for abits in 1..=26u8 {
            for &peak in &[0.5, 12.0, 3_000.0, 800_000.0, 7.6e6] {
                let idx = scale_index_for(abits, peak, t);
                let step = t.step_size(abits).unwrap();
                let span = f64::from(qmax(abits)) * step * f64::from(RMS_7BIT[idx as usize]);
                // Either covered, or pinned at the table top.
                assert!(span >= peak || idx == 124, "a={abits} peak={peak}");
            }
        }
    }
}
