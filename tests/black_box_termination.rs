//! Black-box validation of the **termination-frame** stream fixture
//! (`tests/fixtures/dts_term_5_frames.bin`) against a reference decode
//! produced by the `ffmpeg` binary (invoked as an opaque black-box
//! reference decoder; its output is treated as opaque reference data).
//!
//! The fixture is **spec-built** (no reachable black-box encoder emits
//! `FTYPE = 0` frames): four plain normal stereo frames (512 samples
//! each) followed by one termination frame (`nSSC = 2`, `PSC = 5` →
//! 416 samples, `SHORT` deficit 11) — the §5.3.1 use case, aligning a
//! sequence end to a video frame boundary. The deterministic builder
//! in `tests/common/mod.rs` re-derives the committed bytes in CI.
//!
//! The reference PCM was produced once, out of band:
//!
//! ```text
//!   ffmpeg -f dts -i dts_term_5_frames.bin -f s32le \
//!          -acodec pcm_s32le dts_term_5_frames_ffmpeg_ref.s32
//! ```
//!
//! **Observed reference bound (2026-07, black-box):** the reference
//! *skips termination frames at the parser level* — it reads all
//! 10 000 bytes, reports zero decode errors, and emits exactly
//! `4 × 512` samples per channel (the normal frames), whether the
//! `FTYPE = 0` frame sits at the end or mid-stream, and whether or
//! not it carries a partial subsubframe. The shape-exact comparison
//! bound is therefore the **normal-frame prefix**: it validates that
//! our stream is reference-legal and framed exactly (the reference
//! resyncs cleanly across the termination frame's 2 000 bytes), while
//! the termination frame's own PCM (416 samples, the §5.4.1 PSC
//! valid-prefix semantics) is validated in-crate by
//! `tests/termination_decode.rs`.

mod common;

use oxideav_dts::{iter_frames, CoreStreamDecoder, FrameType};

/// The committed spec-built stream: 4 normal frames + 1 termination.
const FIXTURE: &[u8] = include_bytes!("fixtures/dts_term_5_frames.bin");

/// The reference decode of [`FIXTURE`] — interleaved s32le, 2
/// channels; 2048 samples per channel (the 4 normal frames only; see
/// the module docs for the observed skip of the termination frame).
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_term_5_frames_ffmpeg_ref.s32");

const NORMAL_FRAMES: usize = 4;
const NORMAL_SAMPLES: usize = 512;

/// The committed fixture is byte-for-byte the deterministic builder's
/// output: its provenance is `tests/common/mod.rs` (spec tables +
/// LCG), and any builder drift breaks loudly here.
#[test]
fn committed_stream_matches_builder() {
    let rebuilt = common::build_termination_stream(5);
    assert_eq!(
        rebuilt, FIXTURE,
        "the committed termination fixture must be re-derivable from the builder"
    );
}

/// Pin the observed black-box bound: the reference emitted exactly the
/// normal-frame prefix (4 × 512 samples per channel), skipping the
/// termination frame without any decode error. If a future reference
/// binary starts decoding termination frames, this assertion fails
/// loudly and the comparison should be widened to cover the 416-sample
/// tail.
#[test]
fn reference_covers_exactly_the_normal_frame_prefix() {
    assert_eq!(
        FFMPEG_REF_S32LE.len(),
        NORMAL_FRAMES * NORMAL_SAMPLES * 2 * 4,
        "reference decode = 4 normal frames x 512 samples x 2ch x 4 bytes"
    );
}

fn decode_ours() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    let mut out: Vec<Vec<i32>> = vec![Vec::new(), Vec::new()];
    let mut frame_types = Vec::new();
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        frame_types.push(fv.header.frame_type);
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every fixture frame must decode to PCM");
        for ch in 0..2 {
            out[ch].extend(&pcm[ch]);
        }
    }
    assert_eq!(
        frame_types,
        vec![
            FrameType::Normal,
            FrameType::Normal,
            FrameType::Normal,
            FrameType::Normal,
            FrameType::Termination
        ]
    );
    out
}

fn reference() -> Vec<Vec<i32>> {
    let mut out: Vec<Vec<i32>> = vec![Vec::new(), Vec::new()];
    for (i, c) in FFMPEG_REF_S32LE.chunks_exact(4).enumerate() {
        out[i % 2].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    out
}

fn pearson(a: &[i32], b: &[i32]) -> f64 {
    let n = a.len();
    let af: Vec<f64> = a.iter().map(|&v| v as f64).collect();
    let bf: Vec<f64> = b.iter().map(|&v| v as f64).collect();
    let ma = af.iter().sum::<f64>() / n as f64;
    let mb = bf.iter().sum::<f64>() / n as f64;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (af[i] - ma, bf[i] - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    num / (da.sqrt() * db.sqrt())
}

/// Our decode of the whole stream is `4·512 + 416` samples per
/// channel, and the normal-frame prefix is shape-identical to the
/// reference on both channels (Pearson > 0.999) — so the stream that
/// carries the termination frame is reference-legal and our framing
/// of it is exact.
#[test]
fn normal_prefix_matches_reference_shape_and_tail_decodes() {
    let ours = decode_ours();
    let refc = reference();

    let prefix = NORMAL_FRAMES * NORMAL_SAMPLES;
    for ch in 0..2 {
        assert_eq!(
            ours[ch].len(),
            prefix + common::TERM_SAMPLES,
            "channel {ch}: 4 x 512 + 416 samples"
        );
        assert_eq!(refc[ch].len(), prefix);
        let corr = pearson(&ours[ch][..prefix], &refc[ch]);
        assert!(
            corr > 0.999,
            "channel {ch}: Pearson correlation vs reference = {corr:.6} over \
             the normal-frame prefix, expected > 0.999"
        );
    }

    // The 416-sample termination tail is live audio, not padding.
    for (ch, plane) in ours.iter().enumerate() {
        let tail = &plane[prefix..];
        let peak = tail.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 1000, "channel {ch} termination tail silent ({peak})");
    }
}

/// Sign agreement above a small noise floor is total on the compared
/// prefix.
#[test]
fn normal_prefix_sign_agrees_with_reference() {
    let ours = decode_ours();
    let refc = reference();
    let prefix = NORMAL_FRAMES * NORMAL_SAMPLES;

    for ch in 0..2 {
        let mut agree = 0usize;
        let mut total = 0usize;
        for i in 0..prefix {
            if ours[ch][i].abs() > 5000 && refc[ch][i].abs() > 5000 {
                total += 1;
                if (ours[ch][i] < 0) == (refc[ch][i] < 0) {
                    agree += 1;
                }
            }
        }
        assert!(
            total > 1000,
            "channel {ch}: too few above-floor samples ({total})"
        );
        assert_eq!(
            agree, total,
            "channel {ch}: {agree}/{total} samples agree in sign with the reference"
        );
    }
}
