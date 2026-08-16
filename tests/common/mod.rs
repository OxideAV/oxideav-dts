//! Shared test-side builder for **synthetic joint-intensity (JOINX)
//! DTS Core frames**, assembled field-by-field from the §5.3 / §5.4 /
//! §5.5 bit-stream layout of ETSI TS 102 114 V1.3.1 (the staged
//! `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`).
//!
//! No available black-box encoder emits `JOINX != 0` (the one
//! reachable DTS encoder was swept across its entire accepted
//! bitrate/samplerate matrix and parses as `JOINX == [0, …]` in every
//! frame), and `docs/audio/dts/` stages no joint-intensity fixture.
//! Real-stream validation of the joint-intensity decode path therefore
//! uses **spec-built** streams: every field below is written straight
//! from the Table 5-21 / Table 5-28 / Table 5-29 pseudocode, and the
//! result is validated two independent ways —
//!
//! 1. end-to-end through this crate's own decode chain (see
//!    `tests/joint_intensity_decode.rs`), and
//! 2. black-box: the committed stream is reference-decoded by an
//!    opaque external decoder binary (out of band, like the other
//!    fixtures) and our PCM is shape-compared against it
//!    (`tests/black_box_joint_intensity.rs`).
//!
//! The builder is deterministic (tiny LCG), so the committed fixture
//! can be re-derived and byte-compared in CI — the fixture's
//! provenance is this source file, not an opaque binary.

// Shared across several integration-test targets; not every target
// uses every helper.
#![allow(dead_code)]

use oxideav_dts::{parse_frame_header, DtsFrameHeader, FrameType, LfeMode};

/// MSB-first bit packer mirroring the spec's `ExtractBits` order.
#[derive(Default)]
pub struct BitSink {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the low `width` bits of `value`, MSB-first.
    pub fn push(&mut self, value: u32, width: u8) {
        for i in (0..width).rev() {
            let bit = ((value >> i) & 1) as u8;
            if self.bit_len % 8 == 0 {
                self.bytes.push(0);
            }
            let byte = self.bytes.last_mut().unwrap();
            *byte |= bit << (7 - (self.bit_len % 8));
            self.bit_len += 1;
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Deterministic LCG so the fixture bytes are reproducible in CI.
pub struct Lcg(pub u32);

impl Lcg {
    pub fn next_u32(&mut self) -> u32 {
        // Numerical-recipes constants; any full-period LCG works here.
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    /// A 5-bit two's-complement NFE sample in `-14..=14` (never the
    /// extreme -16/-15/15 so the ABITS=8 mid-tread range stays clean).
    pub fn nfe_sample(&mut self) -> i32 {
        (self.next_u32() % 29) as i32 - 14
    }
}

/// Layout constants of the synthetic joint-intensity frame.
pub const JOINT_N_SUBS_CH0: usize = 32;
pub const JOINT_N_SUBS_CH1: usize = 16;
pub const JOINT_N_SSC: usize = 2; // SSC=1 -> 2 subsubframes = 512 PCM/ch
pub const JOINT_SAMPLES_PER_FRAME: usize = 512;
/// Fixed byte size of every synthetic frame (`FSIZE`); comfortably
/// above the packed payload so the tail is zero slack, and 4-byte
/// aligned like real streams.
pub const JOINT_FRAME_BYTES: usize = 608;

/// Fixed byte size (`FSIZE`) of the plain (non-joint, both channels
/// full-width) synthetic frames: large enough for every battery
/// shape up to two subframes / `nSSC = 4`, 4-byte aligned.
pub const PLAIN_FRAME_BYTES: usize = 2000;

/// `PSC` of the committed termination fixture's last frame: its
/// second subsubframe carries 5 subband samples per subband.
pub const TERM_PSC: u8 = 5;
/// Subband-sample blocks of the termination frame:
/// `nSSC·8 - (8 - PSC) = 16 - 3 = 13` (`NBLKS` raw 12).
pub const TERM_BLOCKS: usize = 2 * 8 - (8 - TERM_PSC as usize);
/// Decoded PCM samples per channel of the termination frame
/// (`13 · 32`).
pub const TERM_SAMPLES: usize = TERM_BLOCKS * 32;

/// The 16 per-subband `JOIN_SCALES` raw 6-bit indexes for channel 1
/// (JOIN_SHUFF = 5 → Linear6Bit → biased `+64` into the §D.3 table:
/// raw 0 → unity, raw 8 → ≈1.585, …). A varied ramp so the joint
/// import is scale-sensitive, not a plain copy.
pub const JOINT_SCALE_RAW: [u32; JOINT_N_SUBS_CH0 - JOINT_N_SUBS_CH1] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 4, 3, 2, 1];

/// Per-subband 6-bit SCALES indexes (SHUFF = 5 → Linear6Bit → §D.1.1
/// RMS table): a gentle downward spectral ramp, identical for both
/// channels over their own coded ranges.
pub fn scales_index(n: usize) -> u32 {
    // RMS_6BIT[38] ≈ 19055 … RMS_6BIT[30] ≈ 2512; keep well inside the
    // valid 0..=62 range.
    (38 - n / 4) as u32
}

/// Full parameterization of a synthetic stereo Core frame with
/// (optional) joint-intensity coding — the edge-case battery varies
/// these knobs while [`build_joint_frame`] pins the default shape the
/// committed fixture was built from.
pub struct JointFrameSpec {
    /// `SUBFS + 1` — number of §5.4 subframes (each with its own
    /// Table 5-28 side info + tail + §5.5 audio region).
    pub n_subframes: usize,
    /// `SSC + 1` per subframe (all subframes share it here).
    pub n_ssc: usize,
    /// Per-channel `nSUBS` (2 channels).
    pub n_subs: [usize; 2],
    /// Per-channel count of **trailing** §D.10.2 high-frequency-VQ
    /// subbands (`nVQSUB = nSUBS − hf_subbands`). Those subbands'
    /// §5.5 region is one 10-bit `nVQIndex` per subband (phase 1,
    /// before the LFE phase), from [`hf_vq_index`], and their side
    /// info is the single SCALES factor of the Table 5-28 HF tail
    /// loop. Default `[0, 0]` = no HF-VQ subbands (a count, not an
    /// absolute bound, so a spec that overrides `n_subs` keeps the
    /// no-HF default without also updating this field).
    pub hf_subbands: [usize; 2],
    /// Per-channel count of **leading** subbands with `PMODE = 1`
    /// (ADPCM prediction active): each writes a 12-bit `PVQ` index
    /// ([`pvq_index`]) in the §5.4.1 PVQ plane. Must be
    /// `<= nVQSUB[ch]`. Default `[0, 0]` = no prediction.
    pub adpcm_subbands: [usize; 2],
    /// §5.3.1 `HFLAG` (Predictor History Flag Switch): when `true`
    /// the decoder uses the previous frame's §C.2.2 reconstruction
    /// history; when `false` the history is ignored (entry-point
    /// frame).
    pub predictor_history: bool,
    /// When `Some(base)`, the frame's HF-VQ subbands take
    /// **consecutive** §D.10.2 indices `(base + slot) mod 1024` in the
    /// builder's walk order (subframe-major, channel-major,
    /// subband-minor) instead of the [`hf_vq_index`] scatter — the
    /// full-book coverage sweeps drive every §D.10.2 vector through
    /// the real bitstream this way. Default `None`.
    pub hf_index_base: Option<u32>,
    /// The §D.10.1 counterpart of [`Self::hf_index_base`]: when
    /// `Some(base)`, the `PMODE = 1` subbands take consecutive 12-bit
    /// `PVQ` indices `(base + slot) mod 4096` in the §5.4.1 PVQ-plane
    /// walk order instead of the [`pvq_index`] scatter. Default
    /// `None`.
    pub pvq_index_base: Option<u32>,
    /// Per-channel `JOINX` (0 = no joint coding; `k` = source channel
    /// `k - 1`).
    pub joinx: [u8; 2],
    /// The 3-bit `JOIN_SHUFF` selector written for every jointly-coded
    /// channel: 5 → Linear6Bit raw, 6 → Linear7Bit raw, 0 → Huffman
    /// SA129 (§D.5.3 Table A5 symbols), 7 → reserved (error-path).
    pub join_shuff: u8,
    /// The raw `JOIN_SCALES` symbols written per jointly-coded channel
    /// and subframe (linear: absolute index; Huffman: signed symbol).
    pub join_symbols: Vec<i32>,
    /// `Some(code)` sets the frame-header `DYNF` and writes the 8-bit
    /// signed-Q2 `RANGE` code in every subframe tail.
    pub dynf_code: Option<u8>,
    /// Sets the frame-header `CPF`: 16-bit `HCRC` in the header, a
    /// 16-bit `AHCRC` after the audio coding header and a 16-bit
    /// `SICRC` at the end of every subframe tail (all unverified per
    /// the spec's "shall not be applied").
    pub cpf: bool,
    /// Total frame byte size (`FSIZE`); the payload is zero-padded up
    /// to it.
    pub frame_bytes: usize,
    /// Sets the frame-header `ASPF` flag: a `DSYNC` word follows
    /// **every** subsubframe of the §5.5 audio region, not just the
    /// last one.
    pub aspf: bool,
    /// Sets the frame-header `FRONT_SUM` (`SUMF`) flag: the §C.2.4
    /// front L/R sum/difference matrix runs on the reconstructed
    /// sub-band samples after the §C.2.3 joint import.
    pub front_sum: bool,
    /// §5.3.1 `FTYPE`: `FrameType::Termination` writes a termination
    /// frame (required whenever `psc > 0`).
    pub frame_type: FrameType,
    /// §5.4.1 `PSC` written in the **last** subframe's side info
    /// (`0` = no partial subsubframe; earlier subframes always write
    /// `PSC = 0`). When `psc > 0` the last subframe's last
    /// subsubframe carries `psc` samples per active subband instead
    /// of 8, and the header's NBLKS shrinks by `8 - psc` blocks.
    pub psc: u8,
    /// §5.3.1 `SHORT` (Deficit Sample Count) raw wire value for a
    /// termination frame, `0..=30` (the header stores `SHORT + 1`).
    /// Ignored for normal frames (which write the `31` normal-frame
    /// marker, i.e. 32 samples per block).
    pub short_raw: u8,
    /// Adds an LFE channel (`LFF = 1`, 128× interpolation): each
    /// subframe's §5.5 region starts with `2·LFF·nSSC` 8-bit
    /// decimated LFE samples plus the 8-bit `LFEscaleIndex` (Table
    /// 5-29 — the count has no `PSC` term).
    pub lfe: bool,
    /// LCG seed for the §5.5 audio content.
    pub seed: u32,
}

impl JointFrameSpec {
    /// The per-channel `nVQSUB` this spec resolves to
    /// (`nSUBS − hf_subbands`).
    pub fn n_vqsub(&self) -> [usize; 2] {
        [
            self.n_subs[0] - self.hf_subbands[0],
            self.n_subs[1] - self.hf_subbands[1],
        ]
    }

    /// The committed-fixture shape: one subframe, `nSSC = 2`,
    /// `nSUBS = [32, 16]`, `JOINX = [0, 1]`, Linear6Bit joint scales
    /// from [`JOINT_SCALE_RAW`], no DYNF, no CPF.
    pub fn default_joint(seed: u32) -> Self {
        Self {
            n_subframes: 1,
            n_ssc: JOINT_N_SSC,
            n_subs: [JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1],
            hf_subbands: [0, 0],
            adpcm_subbands: [0, 0],
            predictor_history: false,
            hf_index_base: None,
            pvq_index_base: None,
            joinx: [0, 1],
            join_shuff: 5,
            join_symbols: JOINT_SCALE_RAW.iter().map(|&r| r as i32).collect(),
            dynf_code: None,
            cpf: false,
            frame_bytes: JOINT_FRAME_BYTES,
            aspf: false,
            front_sum: false,
            frame_type: FrameType::Normal,
            psc: 0,
            short_raw: 0,
            lfe: false,
            seed,
        }
    }

    /// A plain (non-joint) stereo normal frame: both channels carry
    /// their own 32 sub-bands, `JOINX = [0, 0]`, one subframe,
    /// `nSSC = 2` (16 blocks = 512 PCM samples per channel). The
    /// `FSIZE` is roomier than the joint default's because both
    /// channels carry full-width §5.5 payloads (and the batteries
    /// scale it to `nSSC = 4` / two-subframe shapes).
    pub fn default_plain(seed: u32) -> Self {
        Self {
            n_subs: [32, 32],
            joinx: [0, 0],
            join_symbols: Vec::new(),
            frame_bytes: PLAIN_FRAME_BYTES,
            ..Self::default_joint(seed)
        }
    }

    /// The committed termination-frame shape: a plain stereo frame
    /// with `FTYPE = 0`, one subframe, `nSSC = 2` whose second
    /// subsubframe is **partial** (`PSC = 5` -> 13 blocks = 416 PCM
    /// samples per channel), and a `SHORT` deficit of 11 pad samples
    /// (`short_raw = 10`).
    pub fn default_termination(seed: u32) -> Self {
        Self {
            frame_type: FrameType::Termination,
            psc: TERM_PSC,
            short_raw: 10,
            ..Self::default_plain(seed)
        }
    }
}

/// §D.5.3 Table A5 (SA129 difference symbols) — the encode direction
/// of the 5-level Huffman book, for writing Huffman-coded
/// `JOIN_SCALES` symbols: `symbol → (code, code_len)`.
fn a5_encode(symbol: i32) -> (u32, u8) {
    match symbol {
        0 => (0, 1),
        1 => (2, 2),
        -1 => (6, 3),
        2 => (14, 4),
        -2 => (15, 4),
        other => panic!("A5 has no codeword for symbol {other}"),
    }
}

/// Build one synthetic stereo joint-intensity Core frame.
///
/// * channel 0: 32 active subbands (`SUBS=30`), all ABITS=8 (NFE,
///   5-bit samples), no ADPCM, no HF-VQ;
/// * channel 1: 16 active subbands, same coding, plus `JOINX[1] = 1`:
///   subbands 16..32 are imported from channel 0 and scaled by the
///   §D.3 factors selected by [`JOINT_SCALE_RAW`];
/// * one subframe, `nSSC = 2`, `DSYNC` after the last subsubframe;
/// * `DYNF = 0`, `CPF = 0`, no LFE, normal frame, NBLKS raw = 15
///   (16 blocks = 512 PCM samples per channel).
///
/// `template` supplies the §5.3.1 header fields we do not vary
/// (AMODE/SFREQ/RATE/PCMR/FILTS…); `seed` varies the audio content
/// frame-to-frame. Byte-identical to the committed fixture's frames
/// (asserted in `tests/black_box_joint_intensity.rs`).
pub fn build_joint_frame(template: &DtsFrameHeader, seed: u32) -> Vec<u8> {
    build_frame_from_spec(template, &JointFrameSpec::default_joint(seed))
}

/// Build one synthetic stereo Core frame per `spec` — the
/// field-by-field §5.3.2 / §5.4.1 / §5.5 writer behind
/// [`build_joint_frame`], with every joint-intensity knob exposed.
pub fn build_frame_from_spec(template: &DtsFrameHeader, spec: &JointFrameSpec) -> Vec<u8> {
    assert!(
        spec.psc == 0 || spec.frame_type == FrameType::Termination,
        "PSC > 0 exists only in a termination frame (§5.4.1, PDF p.30)"
    );
    assert!(spec.psc < 8 && spec.short_raw <= 30);
    let n_vqsub = spec.n_vqsub();
    for (ch, &nv) in n_vqsub.iter().enumerate() {
        assert!(
            (1..=spec.n_subs[ch]).contains(&nv),
            "nVQSUB must be 1..=nSUBS"
        );
        assert!(
            spec.adpcm_subbands[ch] <= nv,
            "PMODE subbands must be audio-coded (n < nVQSUB)"
        );
    }

    // NBLKS + 1 = total subband-sample rows across all subframes; a
    // partial last subsubframe (last subframe only) shrinks it by
    // `8 - psc` blocks.
    let total_blocks = spec.n_subframes * spec.n_ssc * 8
        - if spec.psc > 0 {
            8 - spec.psc as usize
        } else {
            0
        };
    assert!(
        (6..=128).contains(&total_blocks),
        "NBLKS raw must be 5..=127"
    );

    let mut header = *template;
    header.frame_type = spec.frame_type;
    header.sample_count_per_block = match spec.frame_type {
        FrameType::Normal => 32,
        // SHORT (deficit) raw 0..=30 is stored as +1 by the parser's
        // convention; the frame pads `short_raw + 1` PCM samples.
        FrameType::Termination => spec.short_raw + 1,
    };
    header.crc_present = spec.cpf;
    header.header_crc = if spec.cpf { Some(0) } else { None };
    header.blocks_per_frame = (total_blocks - 1) as u8;
    header.frame_size_bytes = spec.frame_bytes as u16;
    header.dynamic_range = spec.dynf_code.is_some();
    header.time_stamp = false;
    header.aux_data = false;
    header.ext_coding = false;
    header.aspf = spec.aspf;
    header.lfe = if spec.lfe {
        LfeMode::Mode1 // LFF = 1 -> 128x interpolation
    } else {
        LfeMode::None
    };
    header.predictor_history = spec.predictor_history;
    header.front_sum = spec.front_sum;
    header.surround_sum = false;

    let header_bytes = oxideav_dts::encode_frame_header_be(&header).expect("header encodes");
    assert_eq!(
        header_bytes.len() * 8,
        header.header_bit_length() as usize,
        "the §5.3.1 header must be byte-aligned (13 bytes CPF=0, 15 CPF=1)"
    );

    let mut b = BitSink::new();

    // ---- §5.3.2 Audio Coding Header (Table 5-21) ----
    b.push((spec.n_subframes - 1) as u32, 4); // SUBFS
    b.push(1, 3); // PCHS -> 2 channels
    for ch in 0..2 {
        b.push((spec.n_subs[ch] - 2) as u32, 5); // SUBS[ch]
    }
    for &nv in &n_vqsub {
        // VQSUB[ch]: nVQSUB = VQSUB + 1; == nSUBS means no HF-VQ.
        b.push((nv - 1) as u32, 5);
    }
    for ch in 0..2 {
        b.push(spec.joinx[ch] as u32, 3); // JOINX[ch]
    }
    for _ch in 0..2 {
        b.push(3, 2); // THUFF = D4 (raw 2-bit TMODE)
    }
    for _ch in 0..2 {
        b.push(5, 3); // SHUFF = Linear6Bit
    }
    for _ch in 0..2 {
        b.push(6, 3); // BHUFF = Linear5Bit
    }
    // SEL plane (ABITS-major, channel-minor), every group at its
    // terminal entry so ABITS=8 resolves to NFE and no ADJ fields
    // follow (Table 5-21's ADJ is Huffman-SEL-gated).
    for _ch in 0..2 {
        b.push(1, 1); // ABITS=1 group (terminal V3)
    }
    for _n in 1..5 {
        for _ch in 0..2 {
            b.push(3, 2); // ABITS 2..=5 groups (terminal V…)
        }
    }
    for _n in 5..10 {
        for _ch in 0..2 {
            b.push(7, 3); // ABITS 6..=10 groups (terminal V…/NFE)
        }
    }
    // ABITS=11 group has a single (NFE) entry: no SEL bits.
    if spec.cpf {
        b.push(0, 16); // AHCRC — consumed, never verified (§5.3.2)
    }

    let mut lcg = Lcg(spec.seed);
    for subframe in 0..spec.n_subframes {
        // The partial subsubframe (PSC > 0) sits in the last subframe.
        let sf_psc = if subframe == spec.n_subframes - 1 {
            spec.psc as usize
        } else {
            0
        };
        // ---- §5.4.1 Primary Audio Coding Side Information ----
        b.push((spec.n_ssc - 1) as u32, 2); // SSC
        b.push(sf_psc as u32, 3); // PSC
        for ch in 0..2 {
            for n in 0..spec.n_subs[ch] {
                // PMODE plane covers all active subbands; prediction
                // is active on the leading `adpcm_subbands` ones.
                b.push(u32::from(n < spec.adpcm_subbands[ch]), 1);
            }
        }
        // PVQ plane: one 12-bit §D.10.1 index per PMODE-set subband.
        for ch in 0..2 {
            for n in 0..spec.adpcm_subbands[ch] {
                b.push(spec_pvq_index(spec, subframe, ch, n), 12);
            }
        }
        // ABITS plane over the audio-coded subbands only (`n <
        // nVQSUB`, Table 5-28 "Not for VQ encoded subbands"),
        // BHUFF=6 -> Linear5Bit.
        for &nv in &n_vqsub {
            for _ in 0..nv {
                b.push(8, 5); // ABITS[ch][n] = 8 -> NFE 5-bit samples
            }
        }
        // TMODE plane, transmitted only when nSSC > 1 (Table 5-28),
        // audio-coded subbands only.
        if spec.n_ssc > 1 {
            for &nv in &n_vqsub {
                for _ in 0..nv {
                    b.push(0, 2); // THUFF=3 -> D4 raw 2-bit; no transient
                }
            }
        }
        // SCALES plane, SHUFF=5 -> Linear6Bit absolute indexes: one
        // factor per bit-allocated audio-coded subband, then the
        // Table 5-28 "High frequency VQ subbands" tail (one factor
        // each) — a single 0..nSUBS ramp here since every coded
        // subband has ABITS > 0.
        for ch in 0..2 {
            for n in 0..spec.n_subs[ch] {
                b.push(scales_index(n), 6);
            }
        }
        // ---- Table 5-28 tail ----
        // 1. All JOIN_SHUFF selectors first (channel-major).
        for ch in 0..2 {
            if spec.joinx[ch] > 0 {
                b.push(spec.join_shuff as u32, 3);
            }
        }
        // 2. Then per jointly-coded channel, its JOIN_SCALES symbols.
        for ch in 0..2 {
            if spec.joinx[ch] > 0 {
                for &sym in &spec.join_symbols {
                    match spec.join_shuff {
                        5 => b.push(sym as u32, 6),
                        6 => b.push(sym as u32, 7),
                        0 => {
                            let (code, len) = a5_encode(sym);
                            b.push(code, len);
                        }
                        other => panic!("builder does not encode JOIN_SHUFF {other}"),
                    }
                }
            }
        }
        // 3. RANGE (DYNF) then 4. SICRC (CPF).
        if let Some(code) = spec.dynf_code {
            b.push(code as u32, 8);
        }
        if spec.cpf {
            b.push(0xDEAD, 16); // SICRC — consumed, never verified
        }

        // ---- §5.5 phase 1 (Table 5-29): one 10-bit `nVQIndex` per
        // high-frequency-VQ subband, ahead of the LFE phase.
        for (ch, &nv) in n_vqsub.iter().enumerate() {
            for n in nv..spec.n_subs[ch] {
                b.push(spec_hf_vq_index(spec, subframe, ch, n), 10);
            }
        }

        // ---- §5.5 LFE phase (Table 5-29): 2·LFF·nSSC 8-bit samples
        // + 8-bit LFEscaleIndex, before the audio-data arrays. The
        // count has no PSC term (whole subsubframes).
        if spec.lfe {
            for _ in 0..2 * spec.n_ssc {
                // Small signed 8-bit decimated samples.
                b.push((lcg.next_u32() % 201).wrapping_sub(100) & 0xff, 8);
            }
            b.push(60, 8); // LFEscaleIndex (well inside the 7-bit RMS table)
        }

        // ---- §5.5 Audio Data (Table 5-29) ----
        for ssf in 0..spec.n_ssc {
            // §5.4.1 PSC: the last subsubframe of a termination-frame
            // subframe is partial — `sf_psc` samples per subband.
            let count = if sf_psc > 0 && ssf == spec.n_ssc - 1 {
                sf_psc
            } else {
                8
            };
            for &nv in &n_vqsub {
                // Audio-coded subbands only — the HF-VQ subbands'
                // whole §5.5 payload is their phase-1 index.
                for _n in 0..nv {
                    for _m in 0..count {
                        b.push((lcg.nfe_sample() as u32) & 0x1f, 5);
                    }
                }
            }
            // DSYNC after every subsubframe when ASPF, else only the last
            // (a DSYNC always occurs after a partial subsubframe).
            if spec.aspf || ssf == spec.n_ssc - 1 {
                b.push(0xffff, 16);
            }
        }
    }

    let body = b.into_bytes();
    let mut frame = header_bytes;
    frame.extend_from_slice(&body);
    assert!(
        frame.len() <= spec.frame_bytes,
        "payload {} exceeds FSIZE {}",
        frame.len(),
        spec.frame_bytes
    );
    frame.resize(spec.frame_bytes, 0);
    frame
}

/// The deterministic 10-bit §D.10.2 `nVQIndex` the builder writes for
/// high-frequency-VQ subband `n` of channel `ch` in subframe
/// `subframe` — a pure function so analytic reconstructions recompute
/// it without parsing.
pub fn hf_vq_index(subframe: usize, ch: usize, n: usize) -> u32 {
    ((subframe * 131 + ch * 61 + n * 7 + 3) % 1024) as u32
}

/// The deterministic 12-bit §D.10.1 `PVQ` index the builder writes for
/// a `PMODE = 1` subband — pure-function counterpart of
/// [`hf_vq_index`].
pub fn pvq_index(subframe: usize, ch: usize, n: usize) -> u32 {
    ((subframe * 257 + ch * 89 + n * 11 + 5) % 4096) as u32
}

/// The §D.10.2 `nVQIndex` the builder writes under `spec` for HF-VQ
/// subband `n` (`n ∈ [nVQSUB[ch], nSUBS[ch])`) of channel `ch` in
/// subframe `subframe`: the [`hf_vq_index`] scatter by default, or —
/// when [`JointFrameSpec::hf_index_base`] pins a sweep base —
/// `(base + slot) mod 1024` with `slot` the subband's position in the
/// frame's phase-1 walk order (subframes outer, channels next, HF
/// subbands inner), so a run of frames with stepped bases covers the
/// whole 1024-vector book with consecutive indices.
pub fn spec_hf_vq_index(spec: &JointFrameSpec, subframe: usize, ch: usize, n: usize) -> u32 {
    let Some(base) = spec.hf_index_base else {
        return hf_vq_index(subframe, ch, n);
    };
    let per_subframe: usize = spec.hf_subbands.iter().sum();
    let slot = subframe * per_subframe
        + spec.hf_subbands[..ch].iter().sum::<usize>()
        + (n - spec.n_vqsub()[ch]);
    ((base as usize + slot) % 1024) as u32
}

/// The §D.10.1 `PVQ` index the builder writes under `spec` for
/// `PMODE = 1` subband `n` of channel `ch` in subframe `subframe`:
/// the [`pvq_index`] scatter by default, or — when
/// [`JointFrameSpec::pvq_index_base`] pins a sweep base —
/// `(base + slot) mod 4096` with `slot` the subband's position in the
/// §5.4.1 PVQ-plane walk order (subframes outer, channels next,
/// leading `PMODE` subbands inner).
pub fn spec_pvq_index(spec: &JointFrameSpec, subframe: usize, ch: usize, n: usize) -> u32 {
    let Some(base) = spec.pvq_index_base else {
        return pvq_index(subframe, ch, n);
    };
    let per_subframe: usize = spec.adpcm_subbands.iter().sum();
    let slot = subframe * per_subframe + spec.adpcm_subbands[..ch].iter().sum::<usize>() + n;
    ((base as usize + slot) % 4096) as u32
}

/// Element `m` of §D.10.2 vector `v` of the **synthetic** test book —
/// the two-int8-÷ 2⁴ §D.10.2 entry decoding applied to a deterministic
/// int8 ramp spanning ±24 raw (±1.5 scaled).
pub fn synthetic_hf_element(v: usize, m: usize) -> f64 {
    f64::from(synthetic_hf_int8(v, m)) / 16.0
}

fn synthetic_hf_int8(v: usize, m: usize) -> i8 {
    (((v * 31 + m * 17 + 7) % 49) as i32 - 24) as i8
}

/// The synthetic §D.10.2 `HFreqVQ` book (1024 × 32), built through the
/// packed-entry constructor so the spec's 16-bit two-element packing
/// (element `2k` = entry `k`'s low byte) is exercised end-to-end.
pub fn synthetic_hf_book() -> oxideav_dts::HfVqCodebook {
    let entries: Vec<[u16; 16]> = (0..1024)
        .map(|v| {
            let mut packed = [0u16; 16];
            for (k, e) in packed.iter_mut().enumerate() {
                let lo = synthetic_hf_int8(v, 2 * k) as u8;
                let hi = synthetic_hf_int8(v, 2 * k + 1) as u8;
                *e = (u16::from(hi) << 8) | u16::from(lo);
            }
            packed
        })
        .collect();
    oxideav_dts::HfVqCodebook::from_packed_entries(&entries).expect("1024 vectors")
}

/// Coefficient `j` of §D.10.1 vector `i` of the **synthetic** test
/// book: stored integers in `[-1024, 1024]` (÷ 2¹³ -> |coeff| ≤ 0.125,
/// keeping the 4-tap predictor comfortably stable).
pub fn synthetic_adpcm_coeff(i: usize, j: usize) -> f64 {
    f64::from(synthetic_adpcm_entry(i, j)) / 8192.0
}

fn synthetic_adpcm_entry(i: usize, j: usize) -> i32 {
    ((i * 97 + j * 13 + 1) % 2049) as i32 - 1024
}

/// The synthetic §D.10.1 `ADPCMCoeffVQ` book (4096 × 4), built through
/// the stored-integer constructor so the ÷ 2¹³ scaling is exercised.
pub fn synthetic_adpcm_book() -> oxideav_dts::AdpcmVqCodebook {
    let entries: Vec<[i32; 4]> = (0..4096)
        .map(|i| [0, 1, 2, 3].map(|j| synthetic_adpcm_entry(i, j)))
        .collect();
    oxideav_dts::AdpcmVqCodebook::from_entries(&entries).expect("4096 vectors")
}

/// Both synthetic books as a [`oxideav_dts::VqCodebooks`] pair.
pub fn synthetic_vq_codebooks() -> oxideav_dts::VqCodebooks {
    oxideav_dts::VqCodebooks::none()
        .with_hfreq(synthetic_hf_book())
        .with_adpcm(synthetic_adpcm_book())
}

/// Build the multi-frame synthetic joint-intensity elementary stream:
/// `n_frames` frames whose §5.3.1 constant fields come from the first
/// frame of the committed real stereo fixture (so AMODE/SFREQ/RATE/
/// PCMR are values a real encoder emits), with per-frame varying
/// audio content.
pub fn build_joint_stream(n_frames: usize) -> Vec<u8> {
    // Template: the bundled real 48 kHz stereo fixture's first header.
    let template_bytes = include_bytes!("../fixtures/dts_5_frames.bin");
    let template = parse_frame_header(template_bytes).expect("fixture header parses");

    let mut stream = Vec::with_capacity(n_frames * JOINT_FRAME_BYTES);
    for k in 0..n_frames {
        stream.extend_from_slice(&build_joint_frame(&template, 0x1234_5678 ^ (k as u32) << 8));
    }
    stream
}

/// Build the synthetic **§D.10-bearing** elementary stream committed
/// as `tests/fixtures/dts_d10_5_frames.bin`: two plain normal frames,
/// then an HF-VQ frame (`nVQSUB < nSUBS` on both channels), an ADPCM
/// frame (`PMODE = 1` on leading subbands, `HFLAG = 0`), and a
/// combined HF-VQ + ADPCM frame with `HFLAG = 1` (history carried
/// over the previous frame). 512 samples per channel per frame.
pub fn build_d10_stream() -> Vec<u8> {
    let template = template_header();
    let specs = [
        JointFrameSpec::default_plain(0xD10_5EED),
        JointFrameSpec::default_plain(0xD10_5EED ^ 0x100),
        JointFrameSpec {
            hf_subbands: [8, 4],
            ..JointFrameSpec::default_plain(0xD10_5EED ^ 0x200)
        },
        JointFrameSpec {
            adpcm_subbands: [4, 2],
            ..JointFrameSpec::default_plain(0xD10_5EED ^ 0x300)
        },
        JointFrameSpec {
            hf_subbands: [8, 0],
            adpcm_subbands: [2, 2],
            predictor_history: true,
            ..JointFrameSpec::default_plain(0xD10_5EED ^ 0x400)
        },
    ];
    let mut stream = Vec::new();
    for spec in &specs {
        stream.extend_from_slice(&build_frame_from_spec(&template, spec));
    }
    stream
}

/// Build the synthetic **§D.10 interaction-stress** elementary stream
/// committed as `tests/fixtures/dts_d10_stress_6_frames.bin`: six
/// normal stereo frames exercising the §D.10 VQ/ADPCM sub-paths across
/// widths and the §C.2.2 cross-frame `HFLAG` history chain, in one
/// continuous stream (so the §C.2.5 filter tail carries across the
/// VQ-bearing frames). Every frame is reference-decodable and shares
/// the plain frame size, so the black-box comparison sees all six:
///
/// 1. plain (books-independent anchor + filter warm-up);
/// 2. **wide** HF-VQ (`hf_subbands = [12, 8]` — 20 VQ subbands);
/// 3. HF-VQ + ADPCM together (`[4, 4]` + `[6, 4]`, `HFLAG = 0`);
/// 4. the same HF-VQ + ADPCM shape with `HFLAG = 1` — §C.2.2
///    prediction primed by frame 3's reconstruction history;
/// 5. **ADPCM-heavy** (`adpcm_subbands = [8, 6]`, `HFLAG = 0`);
/// 6. HF-VQ + ADPCM with `HFLAG = 1` continuing frame 5's history.
pub fn build_d10_stress_stream() -> Vec<u8> {
    let template = template_header();
    let specs = [
        JointFrameSpec::default_plain(0xD10_57E0),
        JointFrameSpec {
            hf_subbands: [12, 8],
            ..JointFrameSpec::default_plain(0xD10_57E1)
        },
        JointFrameSpec {
            hf_subbands: [4, 4],
            adpcm_subbands: [6, 4],
            ..JointFrameSpec::default_plain(0xD10_57E2)
        },
        JointFrameSpec {
            hf_subbands: [4, 4],
            adpcm_subbands: [6, 4],
            predictor_history: true,
            ..JointFrameSpec::default_plain(0xD10_57E3)
        },
        JointFrameSpec {
            adpcm_subbands: [8, 6],
            ..JointFrameSpec::default_plain(0xD10_57E4)
        },
        JointFrameSpec {
            hf_subbands: [4, 2],
            adpcm_subbands: [8, 6],
            predictor_history: true,
            ..JointFrameSpec::default_plain(0xD10_57E5)
        },
    ];
    let mut stream = Vec::new();
    for spec in &specs {
        stream.extend_from_slice(&build_frame_from_spec(&template, spec));
    }
    stream
}

/// The §5.3.1 header template shared by every synthetic frame: the
/// first frame of the committed real stereo fixture (so AMODE/SFREQ/
/// RATE/PCMR are values a real encoder emits).
pub fn template_header() -> DtsFrameHeader {
    let template_bytes = include_bytes!("../fixtures/dts_5_frames.bin");
    parse_frame_header(template_bytes).expect("fixture header parses")
}

/// Build the synthetic **termination-ended** elementary stream:
/// `n_frames - 1` plain normal stereo frames (512 samples each)
/// followed by one termination frame ([`JointFrameSpec::
/// default_termination`]: `FTYPE = 0`, `nSSC = 2`, `PSC = 5` -> 416
/// samples, `SHORT` deficit 11) — the spec's own use case, "to
/// accurately align the end of an audio sequence with a video frame
/// end point" (§5.3.1, PDF p.18).
pub fn build_termination_stream(n_frames: usize) -> Vec<u8> {
    assert!(n_frames >= 1);
    let template = template_header();
    let mut stream = Vec::with_capacity(n_frames * JOINT_FRAME_BYTES);
    for k in 0..n_frames - 1 {
        let spec = JointFrameSpec::default_plain(0x5EED_0000 ^ (k as u32) << 8);
        stream.extend_from_slice(&build_frame_from_spec(&template, &spec));
    }
    let spec = JointFrameSpec::default_termination(0x5EED_0000 ^ ((n_frames - 1) as u32) << 8);
    stream.extend_from_slice(&build_frame_from_spec(&template, &spec));
    stream
}
