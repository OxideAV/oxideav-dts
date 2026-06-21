//! Black-box PCM validation of the full DTS Core reconstruction chain
//! against a reference decode produced by the `ffmpeg` binary.
//!
//! Clean-room note: `ffmpeg` is used here ONLY as an opaque reference
//! decoder — its source is never consulted. The reference PCM
//! (`tests/fixtures/dts_5_frames_ffmpeg_ref.s32`) was produced once,
//! out of band, by running the `ffmpeg` binary as a black box over the
//! same `dts_5_frames.bin` Core fixture:
//!
//! ```text
//!   ffmpeg -f dts -i dts_5_frames.bin -f s32le -acodec pcm_s32le \
//!          dts_5_frames_ffmpeg_ref.s32
//! ```
//!
//! and committed as a fixture so CI (which has no `ffmpeg`) can run the
//! comparison deterministically. This test decodes the same fixture
//! through [`oxideav_dts::CoreStreamDecoder`] and confirms our PCM is
//! **shape-identical** to the reference — i.e. the §5.3/§5.4/§5.5 +
//! §C.2.5 reconstruction chain reproduces the reference waveform up to
//! the single implementation-defined output gain.
//!
//! ## Why a shape (scale-invariant) comparison, not bit-exact
//!
//! The §C.2.5 output `rScale` (`naCh[i] = int(rScale·raZ[i])`) is, per
//! `docs/audio/dts/dts-qmf-driver.md` §2, **not a normative numeric
//! constant** — TS 102 114 §C.2.5 is explicitly one informative
//! implementation among many, and the constant is implementation-
//! internal. Our [`oxideav_dts::DtsFrameHeader::output_r_scale`] uses
//! the spec-derived `2^(PCMR_bits−1)`; the reference decoder's internal
//! normalization differs by a constant positive factor (empirically
//! ≈ √2 over this fixture). A scale-invariant comparison (Pearson
//! correlation + sign agreement) therefore validates the entire
//! reconstruction chain — the part the spec pins down — without baking
//! in a magic constant reverse-engineered from the reference, which the
//! clean-room wall forbids.
//!
//! Measured on this fixture: correlation 1.000000 and 100 % sign
//! agreement on both channels (versus 0.73 correlation when the §C.2.5
//! filter is reset per frame instead of carried across the stream — the
//! round-356 [`oxideav_dts::CoreStreamDecoder`] continuity fix).

use oxideav_dts::{iter_frames, CoreStreamDecoder};

/// The 5-frame raw-16-bit DTS Core fixture (real 48 kHz stereo
/// `ffmpeg -c:a dca` output).
const FIXTURE: &[u8] = include_bytes!("fixtures/dts_5_frames.bin");

/// `ffmpeg`'s reference decode of [`FIXTURE`] — interleaved s32le, 2
/// channels, 2560 samples per channel (16 blocks × 32 samples ×
/// 5 frames).
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_5_frames_ffmpeg_ref.s32");

/// Decode the whole fixture through the streaming Core decoder into
/// planar per-channel PCM.
fn decode_ours() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    let mut out: Vec<Vec<i32>> = vec![Vec::new(), Vec::new()];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames must iterate cleanly");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every common-Core fixture frame must decode to PCM");
        for ch in 0..2 {
            out[ch].extend(&pcm[ch]);
        }
    }
    out
}

/// Deinterleave the committed `ffmpeg` reference into planar channels.
fn reference() -> Vec<Vec<i32>> {
    let mut out: Vec<Vec<i32>> = vec![Vec::new(), Vec::new()];
    for (i, c) in FFMPEG_REF_S32LE.chunks_exact(4).enumerate() {
        out[i % 2].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    out
}

/// Pearson correlation of two equal-length sample vectors.
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

/// Our full-chain Core PCM is shape-identical to the `ffmpeg` reference:
/// Pearson correlation ≈ 1.0 on both channels. This is the end-to-end
/// proof that the §5.3/§5.4/§5.5 + §C.2.5 reconstruction reproduces the
/// reference waveform (up to the implementation-defined output gain).
#[test]
fn decodes_real_fixture_stream_matching_ffmpeg_shape() {
    let ours = decode_ours();
    let refc = reference();

    for ch in 0..2 {
        assert_eq!(
            ours[ch].len(),
            refc[ch].len(),
            "channel {ch}: our sample count must match the ffmpeg reference"
        );
        assert_eq!(ours[ch].len(), 2560, "5 frames × 16 blocks × 32 samples");

        let corr = pearson(&ours[ch], &refc[ch]);
        assert!(
            corr > 0.999,
            "channel {ch}: Pearson correlation vs ffmpeg = {corr:.6}, \
             expected > 0.999 (the full reconstruction chain must be \
             shape-identical to the reference)"
        );
    }
}

/// Sign agreement with the reference is total above a small noise floor:
/// every non-trivial sample has the same sign in our output and the
/// reference. (Near-zero samples around zero-crossings are excluded —
/// the sign of a sub-noise-floor value is not meaningful.)
#[test]
fn fixture_pcm_sign_agrees_with_ffmpeg_reference() {
    let ours = decode_ours();
    let refc = reference();

    for ch in 0..2 {
        let mut agree = 0usize;
        let mut total = 0usize;
        for i in 0..ours[ch].len() {
            // Floor chosen well above quantization noise but below the
            // signal's RMS so the bulk of the waveform is tested.
            if ours[ch][i].abs() > 5000 && refc[ch][i].abs() > 5000 {
                total += 1;
                if (ours[ch][i] < 0) == (refc[ch][i] < 0) {
                    agree += 1;
                }
            }
        }
        assert!(
            total > 1000,
            "channel {ch}: too few above-floor samples ({total}) to be a \
             meaningful sign-agreement test"
        );
        assert_eq!(
            agree, total,
            "channel {ch}: {agree}/{total} samples agree in sign with the \
             ffmpeg reference; the reconstruction must match the reference \
             waveform sign-for-sign above the noise floor"
        );
    }
}

/// The fixture decodes to real, non-silent audio of the expected shape
/// (a guard against a vacuous all-zero pass of the correlation test).
#[test]
fn fixture_decodes_to_non_silent_stereo_audio() {
    let ours = decode_ours();
    assert_eq!(ours.len(), 2, "stereo");
    for (ch, plane) in ours.iter().enumerate() {
        assert_eq!(plane.len(), 2560);
        let peak = plane.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(
            peak > 1000,
            "channel {ch}: decoded audio peaked at {peak}, expected a \
             non-silent signal"
        );
    }
    // Both channels carry the same source (ffmpeg duplicated a mono sine
    // to stereo), so they must be identical.
    assert_eq!(ours[0], ours[1], "the fixture's two channels are identical");
}
