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
/// frame-to-frame.
pub fn build_joint_frame(template: &DtsFrameHeader, seed: u32) -> Vec<u8> {
    let mut header = *template;
    header.frame_type = FrameType::Normal;
    header.sample_count_per_block = 32;
    header.crc_present = false;
    header.header_crc = None;
    header.blocks_per_frame = 15; // 16 blocks -> 512 samples/ch
    header.frame_size_bytes = JOINT_FRAME_BYTES as u16;
    header.dynamic_range = false;
    header.time_stamp = false;
    header.aux_data = false;
    header.ext_coding = false;
    header.aspf = false;
    header.lfe = LfeMode::None;
    header.predictor_history = false;
    header.front_sum = false;
    header.surround_sum = false;

    let header_bytes = oxideav_dts::encode_frame_header_be(&header).expect("header encodes");
    assert_eq!(
        header_bytes.len() * 8,
        header.header_bit_length() as usize,
        "CPF=0 header must be byte-aligned at 13 bytes"
    );

    let mut b = BitSink::new();

    // ---- §5.3.2 Audio Coding Header (Table 5-21) ----
    b.push(0, 4); // SUBFS -> 1 subframe
    b.push(1, 3); // PCHS  -> 2 channels
    b.push((JOINT_N_SUBS_CH0 - 2) as u32, 5); // SUBS[0]  -> nSUBS 32
    b.push((JOINT_N_SUBS_CH1 - 2) as u32, 5); // SUBS[1]  -> nSUBS 16
    b.push((JOINT_N_SUBS_CH0 - 1) as u32, 5); // VQSUB[0] -> nVQSUB 32 (no HF-VQ)
    b.push((JOINT_N_SUBS_CH1 - 1) as u32, 5); // VQSUB[1] -> nVQSUB 16
    b.push(0, 3); // JOINX[0] = 0
    b.push(1, 3); // JOINX[1] = 1 (source channel 0)
    b.push(3, 2); // THUFF[0] = D4 (raw 2-bit TMODE)
    b.push(3, 2); // THUFF[1]
    b.push(5, 3); // SHUFF[0] = Linear6Bit
    b.push(5, 3); // SHUFF[1]
    b.push(6, 3); // BHUFF[0] = Linear5Bit
    b.push(6, 3); // BHUFF[1]
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
    // ABITS=11 group has a single (NFE) entry: no SEL bits. No AHCRC
    // (CPF = 0).

    // ---- §5.4.1 Primary Audio Coding Side Information (Table 5-28) ----
    b.push(1, 2); // SSC -> nSSC = 2
    b.push(0, 3); // PSC = 0
    for _ in 0..JOINT_N_SUBS_CH0 {
        b.push(0, 1); // PMODE[0][n] = 0 (no ADPCM)
    }
    for _ in 0..JOINT_N_SUBS_CH1 {
        b.push(0, 1); // PMODE[1][n] = 0
    }
    // No PVQ plane (all PMODE zero). ABITS plane, BHUFF=6 -> Linear5Bit.
    for _ in 0..JOINT_N_SUBS_CH0 {
        b.push(8, 5); // ABITS[0][n] = 8 -> NFE 5-bit samples
    }
    for _ in 0..JOINT_N_SUBS_CH1 {
        b.push(8, 5); // ABITS[1][n] = 8
    }
    // TMODE plane (nSSC > 1), THUFF=3 -> D4 raw 2-bit; no transients.
    for _ in 0..(JOINT_N_SUBS_CH0 + JOINT_N_SUBS_CH1) {
        b.push(0, 2);
    }
    // SCALES plane, SHUFF=5 -> Linear6Bit absolute indexes.
    for n in 0..JOINT_N_SUBS_CH0 {
        b.push(scales_index(n), 6);
    }
    for n in 0..JOINT_N_SUBS_CH1 {
        b.push(scales_index(n), 6);
    }
    // ---- Table 5-28 tail: JOIN_SHUFF / JOIN_SCALES ----
    b.push(5, 3); // JOIN_SHUFF[1] = Linear6Bit
    for &raw in &JOINT_SCALE_RAW {
        b.push(raw, 6); // biased +64 into the §D.3 JScaleTbl
    }
    // DYNF = 0 -> no RANGE; CPF = 0 -> no SICRC.

    // ---- §5.5 Audio Data (Table 5-29) ----
    let mut lcg = Lcg(seed);
    for _ssf in 0..JOINT_N_SSC {
        for n_subs in [JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1] {
            for _n in 0..n_subs {
                for _m in 0..8 {
                    b.push((lcg.nfe_sample() as u32) & 0x1f, 5);
                }
            }
        }
    }
    b.push(0xffff, 16); // DSYNC after the last subsubframe (ASPF = 0)

    let body = b.into_bytes();
    let mut frame = header_bytes;
    frame.extend_from_slice(&body);
    assert!(
        frame.len() <= JOINT_FRAME_BYTES,
        "payload {} exceeds FSIZE {}",
        frame.len(),
        JOINT_FRAME_BYTES
    );
    frame.resize(JOINT_FRAME_BYTES, 0);
    frame
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
