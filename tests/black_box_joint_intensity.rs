//! Black-box PCM validation of the **joint-intensity (`JOINX != 0`)
//! reconstruction** against a reference decode produced by the
//! `ffmpeg` binary (used ONLY as an opaque reference decoder — its
//! source is never consulted).
//!
//! The stream fixture (`tests/fixtures/dts_joint_5_frames.bin`) is
//! **spec-built**, not encoder-produced: no reachable black-box
//! encoder emits `JOINX != 0` at any accepted bitrate/samplerate
//! (verified by parsing its output across the whole accepted matrix),
//! so the stream is assembled field-by-field from the staged spec's
//! Table 5-21 / 5-28 / 5-29 layout by the deterministic builder in
//! `tests/common/mod.rs` — five stereo frames whose channel 1 carries
//! `JOINX = 1` and imports sub-bands 16..32 from channel 0 through a
//! §D.3 joint-scale ramp. [`committed_stream_matches_builder`] pins
//! the committed bytes to that builder, so the fixture's provenance is
//! reproducible source, not an opaque binary.
//!
//! The reference PCM was produced once, out of band:
//!
//! ```text
//!   ffmpeg -f dts -i dts_joint_5_frames.bin -f s32le \
//!          -acodec pcm_s32le dts_joint_5_frames_ffmpeg_ref.s32
//! ```
//!
//! (the reference decoder accepted the stream with no warnings) and
//! committed so CI can compare deterministically. As with the other
//! black-box fixtures the comparison is **shape-exact** (Pearson
//! correlation + sign agreement), because the §C.2.5 output `rScale`
//! is implementation-defined (see `tests/black_box_ffmpeg_pcm.rs`).
//!
//! This is the independent cross-check of the whole joint-intensity
//! chain — the §5.4.1 `JOIN_SHUFF`/`JOIN_SCALES` tail parse, the §D.3
//! scale resolution, the §C.2.3 sub-band import, and the §C.2.5
//! effective-`nSUBS` widening: with the pre-round-429 bug (imported
//! sub-bands zero-filled away at the QMF) channel 1's correlation
//! against the reference drops well below 1, since the reference
//! decoder *does* reconstruct the imported high sub-bands.

mod common;

use oxideav_dts::{iter_frames, CoreStreamDecoder};

/// The committed spec-built 5-frame stereo `JOINX = 1` stream.
const FIXTURE: &[u8] = include_bytes!("fixtures/dts_joint_5_frames.bin");

/// `ffmpeg`'s reference decode of [`FIXTURE`] — interleaved s32le,
/// 2 channels, 2560 samples per channel (5 frames × 512).
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_joint_5_frames_ffmpeg_ref.s32");

/// The committed fixture is byte-for-byte the deterministic builder's
/// output: the stream's provenance is `tests/common/mod.rs` (spec
/// tables + LCG), and any builder drift breaks loudly here instead of
/// silently invalidating the reference comparison.
#[test]
fn committed_stream_matches_builder() {
    let rebuilt = common::build_joint_stream(5);
    assert_eq!(
        rebuilt, FIXTURE,
        "the committed joint fixture must be re-derivable from the builder"
    );
}

fn decode_ours() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    let mut out: Vec<Vec<i32>> = vec![Vec::new(), Vec::new()];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every joint fixture frame must decode to PCM");
        for ch in 0..2 {
            out[ch].extend(&pcm[ch]);
        }
    }
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

/// Our joint-intensity PCM is shape-identical to the reference on
/// **both** channels — channel 1 being the jointly-coded one whose
/// upper sixteen sub-bands exist only through the §C.2.3 import.
#[test]
fn joint_decode_matches_reference_shape() {
    let ours = decode_ours();
    let refc = reference();

    for ch in 0..2 {
        assert_eq!(ours[ch].len(), refc[ch].len(), "channel {ch} length");
        assert_eq!(ours[ch].len(), 2560, "5 frames x 512 samples");
        let corr = pearson(&ours[ch], &refc[ch]);
        assert!(
            corr > 0.999,
            "channel {ch}: Pearson correlation vs reference = {corr:.6}, \
             expected > 0.999 (joint-intensity reconstruction must be \
             shape-identical to the reference)"
        );
    }
}

/// Sign agreement above a small noise floor is total on both channels.
#[test]
fn joint_decode_sign_agrees_with_reference() {
    let ours = decode_ours();
    let refc = reference();

    for ch in 0..2 {
        let mut agree = 0usize;
        let mut total = 0usize;
        for i in 0..ours[ch].len() {
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

/// The jointly-coded channel is non-silent real audio (guards against
/// a vacuous correlation pass on near-zero data), and the two channels
/// are genuinely different programmes (channel 1 is not a copy of
/// channel 0 — its own low sub-bands differ).
#[test]
fn joint_fixture_decodes_to_distinct_non_silent_channels() {
    let ours = decode_ours();
    for (ch, plane) in ours.iter().enumerate() {
        let peak = plane.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 1000, "channel {ch} silent (peak {peak})");
    }
    assert_ne!(
        ours[0], ours[1],
        "the two channels carry different content by construction"
    );
}
