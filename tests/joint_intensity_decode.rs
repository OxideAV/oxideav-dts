//! End-to-end **joint-intensity (`JOINX != 0`) decode** validation on
//! spec-built streams (see `tests/common/mod.rs` for why the streams
//! are synthetic: no reachable black-box encoder emits `JOINX != 0`).
//!
//! Three layers of proof:
//!
//! 1. **The flag is really set**: every frame of the stream parses with
//!    `JOINX == [0, 1]` through the §5.3.2 audio-coding-header walker —
//!    the streams genuinely exercise the joint path, they don't just
//!    claim to.
//! 2. **Analytic reconstruction**: the frame content is known at build
//!    time (deterministic LCG), so the expected per-subband sample
//!    matrices can be recomputed *without parsing* — quantization index
//!    × §D.2 step size × §D.1.1 scale for the coded ranges, then the
//!    §C.2.3 joint import `JOIN_SCALES[n] · src[n]` for sub-bands
//!    16..32 of channel 1 — and pushed through the public §C.2.5
//!    [`MultiChannelQmf`] with the source channel's sub-band count.
//!    `decode_core_frame`'s PCM must equal that reconstruction
//!    **bit-exactly**, which pins the whole §5.4.1 tail parse
//!    (JOIN_SHUFF / JOIN_SCALES cursor math), the §D.3 scale
//!    resolution, the §C.2.3 copy, and the effective-`nSUBS` widening
//!    at the QMF in one assertion.
//! 3. **The import is audible**: channel 1's PCM must differ from a
//!    decode of the same frame with the joint import zeroed (i.e. the
//!    high-band energy really lands in the output — this is the
//!    regression guard for the bug where the QMF was driven with the
//!    destination's own `nSUBS` and silently discarded the import).

mod common;

use common::{
    build_joint_frame, build_joint_stream, scales_index, Lcg, JOINT_FRAME_BYTES, JOINT_N_SSC,
    JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1, JOINT_SAMPLES_PER_FRAME, JOINT_SCALE_RAW,
};
use oxideav_dts::{
    decode_audio_coding_header_at, decode_core_frame, iter_frames, join_scale, parse_frame_header,
    FilterBankSelection, MultiChannelQmf, StepSizeTable, NUM_SUBBAND, RMS_6BIT,
};

const STREAM_FRAMES: usize = 5;

/// Layer 1: the synthetic stream's joint-intensity flag is genuinely
/// set — every frame parses with `JOINX == [0, 1]`, `nSUBS == [32, 16]`.
#[test]
fn synthetic_stream_parses_with_joinx_set() {
    let stream = build_joint_stream(STREAM_FRAMES);
    assert_eq!(stream.len(), STREAM_FRAMES * JOINT_FRAME_BYTES);

    let mut frames = 0usize;
    for fv in iter_frames(&stream) {
        let fv = fv.expect("synthetic frames iterate cleanly");
        let hb = fv.header.header_bit_length() as usize;
        let (coding, _) = decode_audio_coding_header_at(fv.data, hb, fv.header.crc_present)
            .expect("audio coding header parses");
        assert_eq!(coding.n_pchs, 2);
        assert_eq!(
            coding.joinx,
            vec![0, 1],
            "channel 1 must carry JOINX = 1 (joint-intensity, source ch 0)"
        );
        assert_eq!(coding.n_subs(), vec![JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1]);
        assert_eq!(coding.n_vqsub(), vec![JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1]);
        frames += 1;
    }
    assert_eq!(frames, STREAM_FRAMES);
}

/// Recompute the two channels' per-subband sample matrices straight
/// from the builder's known field values (no bit-stream parsing):
/// `sample = AUDIO_index · step(ABITS=8) · RMS_6BIT[scales_index(n)]`,
/// then the §C.2.3 joint import for channel 1's sub-bands 16..32.
fn analytic_matrices(rate_index: u8, seed: u32) -> Vec<Vec<[f64; NUM_SUBBAND]>> {
    let rows = JOINT_N_SSC * 8;
    let mut matrices = vec![vec![[0.0f64; NUM_SUBBAND]; rows]; 2];

    let table = StepSizeTable::for_rate(rate_index);
    let step = table.step_size(8).expect("ABITS=8 step size");

    // §5.5 walk order: subsubframe-major, channel, subband, 8 samples.
    let mut lcg = Lcg(seed);
    for ssf in 0..JOINT_N_SSC {
        for (ch, n_subs) in [JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH1].into_iter().enumerate() {
            for n in 0..n_subs {
                let scale = f64::from(RMS_6BIT[scales_index(n) as usize]);
                for m in 0..8 {
                    let index = lcg.nfe_sample();
                    matrices[ch][ssf * 8 + m][n] = step * scale * f64::from(index);
                }
            }
        }
    }

    // §C.2.3: ch1 imports sub-bands 16..32 from ch0, scaled by the
    // §D.3 factors of the biased Linear6Bit JOIN_SCALES indexes.
    let source = matrices[0].clone();
    for (dst_row, src_row) in matrices[1].iter_mut().zip(&source) {
        for (k, &raw) in JOINT_SCALE_RAW.iter().enumerate() {
            let n = JOINT_N_SUBS_CH1 + k;
            let factor = join_scale(raw as i32 + 64).expect("biased index inside §D.3");
            dst_row[n] = factor * src_row[n];
        }
    }
    matrices
}

/// Layer 2: `decode_core_frame` over the synthetic joint frame is
/// bit-exactly the analytic reconstruction pushed through the public
/// §C.2.5 QMF — with channel 1 synthesized over the **source**
/// channel's 32 sub-bands per the §C.2.5 driving-call note.
#[test]
fn joint_frame_decode_matches_analytic_reconstruction() {
    let seed = 0xC0FF_EE01;
    let template = parse_frame_header(include_bytes!("fixtures/dts_5_frames.bin"))
        .expect("template header parses");
    let frame = build_joint_frame(&template, seed);
    let header = parse_frame_header(&frame).expect("synthetic frame header parses");

    let ours = decode_core_frame(&frame, &header).expect("joint frame decodes to PCM");
    assert_eq!(ours.len(), 2);
    assert_eq!(ours[0].len(), JOINT_SAMPLES_PER_FRAME);
    assert_eq!(ours[1].len(), JOINT_SAMPLES_PER_FRAME);

    // Analytic expectation through the public QMF.
    let matrices = analytic_matrices(header.rate_index, seed);
    let refs: Vec<&[[f64; NUM_SUBBAND]]> = matrices.iter().map(|m| m.as_slice()).collect();
    let filter: FilterBankSelection = header.filter_bank_selection();
    let r_scale = header
        .output_r_scale()
        .expect("template PCMR is not reserved");
    // Per §C.2.5 (PDF p.184): the jointly-coded channel's active count
    // "must be set to that of the source channel".
    let n_subs_eff = [JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH0];
    let mut expect: Vec<Vec<i32>> = vec![Vec::new(); 2];
    MultiChannelQmf::new(2)
        .synthesize_planar(&refs, &n_subs_eff, filter, r_scale, &mut expect)
        .expect("analytic QMF synthesis");

    assert_eq!(
        ours, expect,
        "decode_core_frame must reproduce the analytic joint reconstruction bit-exactly"
    );
    // Guard against a vacuous pass: the frame is not silent.
    let peak = ours[1].iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 1000, "channel 1 peak {peak} — expected real audio");
}

/// Layer 3: the joint import is *audible* — zeroing the imported
/// sub-bands (but keeping everything else identical) changes channel
/// 1's PCM. This is the regression guard for the effective-`nSUBS`
/// QMF bug: with the import zero-filled away the two decodes were
/// identical.
#[test]
fn joint_import_changes_channel1_output() {
    let seed = 0xC0FF_EE01;
    let template = parse_frame_header(include_bytes!("fixtures/dts_5_frames.bin"))
        .expect("template header parses");
    let frame = build_joint_frame(&template, seed);
    let header = parse_frame_header(&frame).expect("synthetic frame header parses");
    let ours = decode_core_frame(&frame, &header).expect("joint frame decodes");

    // Ablated reconstruction: same analytic matrices with the imported
    // columns zeroed, synthesized over the same effective counts.
    let mut matrices = analytic_matrices(header.rate_index, seed);
    for row in matrices[1].iter_mut() {
        for slot in row.iter_mut().take(JOINT_N_SUBS_CH0).skip(JOINT_N_SUBS_CH1) {
            *slot = 0.0;
        }
    }
    let refs: Vec<&[[f64; NUM_SUBBAND]]> = matrices.iter().map(|m| m.as_slice()).collect();
    let mut ablated: Vec<Vec<i32>> = vec![Vec::new(); 2];
    MultiChannelQmf::new(2)
        .synthesize_planar(
            &refs,
            &[JOINT_N_SUBS_CH0, JOINT_N_SUBS_CH0],
            header.filter_bank_selection(),
            header.output_r_scale().unwrap(),
            &mut ablated,
        )
        .expect("ablated QMF synthesis");

    assert_eq!(ours[0], ablated[0], "channel 0 carries no joint import");
    assert_ne!(
        ours[1], ablated[1],
        "zeroing the §C.2.3 import must change channel 1's PCM — the \
         imported high sub-bands must reach the output"
    );

    // Quantify: the imported bands carry real energy (RMS of the
    // difference is well above the rounding floor).
    let diff_energy: f64 = ours[1]
        .iter()
        .zip(&ablated[1])
        .map(|(&a, &b)| {
            let d = f64::from(a) - f64::from(b);
            d * d
        })
        .sum::<f64>()
        / ours[1].len() as f64;
    assert!(
        diff_energy.sqrt() > 100.0,
        "joint-import RMS contribution {:.1} too small — the import \
         must contribute audible energy",
        diff_energy.sqrt()
    );
}

/// The whole 5-frame joint stream decodes through the streaming
/// decoder (inter-frame §C.2.5 filter tail carried), producing full-
/// length non-silent PCM on both channels.
#[test]
fn joint_stream_decodes_end_to_end() {
    let stream = build_joint_stream(STREAM_FRAMES);
    let mut dec = oxideav_dts::CoreStreamDecoder::new(2);
    let mut out: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(&stream) {
        let fv = fv.expect("frames iterate");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every joint frame decodes");
        for ch in 0..2 {
            out[ch].extend(&pcm[ch]);
        }
    }
    for (ch, plane) in out.iter().enumerate() {
        assert_eq!(plane.len(), STREAM_FRAMES * JOINT_SAMPLES_PER_FRAME);
        let peak = plane.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 1000, "channel {ch} silent (peak {peak})");
    }
}
