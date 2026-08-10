//! Black-box validation of the **§D.10 interaction-stress** stream
//! (`tests/fixtures/dts_d10_stress_6_frames.bin`) against a reference
//! decode produced by the `ffmpeg` binary (invoked as an opaque
//! black-box reference decoder; its output is treated as opaque
//! reference data).
//!
//! Where `tests/black_box_d10.rs` pins the built-in §D.10 books on a
//! single HF-VQ / single ADPCM / one combined frame, this fixture
//! stresses the sub-paths *in a continuous six-frame stream*: a wide
//! HF-VQ frame (20 VQ subbands), HF-VQ + ADPCM together, the §C.2.2
//! cross-frame `HFLAG` history chain primed twice, and an
//! ADPCM-heavy frame — all carrying the persistent §C.2.5 filter
//! tail. Every frame is decoded with the **default** decoder (the
//! built-in books, no `set_vq_codebooks` call), and every frame is
//! shape-identical to the reference (Pearson ≈ 1.0) and matches it
//! above 90 dB SNR after the single implementation-defined output
//! `rScale` constant (√2 for this fixture's `PCMR`, the same ratio the
//! plain fixtures show).
//!
//! The reference PCM was produced once, out of band:
//!
//! ```text
//!   ffmpeg -f dts -i dts_d10_stress_6_frames.bin -f s32le \
//!          -acodec pcm_s32le dts_d10_stress_6_frames_ffmpeg_ref.s32
//! ```

mod common;

use common::build_d10_stress_stream;
use oxideav_dts::{iter_frames, CoreStreamDecoder};

const FIXTURE: &[u8] = include_bytes!("fixtures/dts_d10_stress_6_frames.bin");
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_d10_stress_6_frames_ffmpeg_ref.s32");

const FRAMES: usize = 6;
const SAMPLES_PER_FRAME: usize = 512;

/// The committed fixture is byte-for-byte the deterministic builder's
/// output — provenance is `tests/common/mod.rs`, not an opaque binary.
#[test]
fn committed_fixture_matches_builder() {
    assert_eq!(
        FIXTURE,
        build_d10_stress_stream().as_slice(),
        "fixture must re-derive from the deterministic builder"
    );
}

fn reference() -> Vec<Vec<i32>> {
    assert_eq!(FFMPEG_REF_S32LE.len() % 8, 0);
    let mut ch0 = Vec::new();
    let mut ch1 = Vec::new();
    for pair in FFMPEG_REF_S32LE.chunks_exact(8) {
        ch0.push(i32::from_le_bytes(pair[0..4].try_into().unwrap()));
        ch1.push(i32::from_le_bytes(pair[4..8].try_into().unwrap()));
    }
    vec![ch0, ch1]
}

/// Decode the whole stream with the **default** decoder — the
/// round-439 built-in books, no `set_vq_codebooks` anywhere.
fn ours_default() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every stress frame decodes by default");
        for (ch, samples) in block.into_iter().enumerate() {
            pcm[ch].extend(samples);
        }
    }
    pcm
}

fn pearson(a: &[i32], b: &[i32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let n = a.len() as f64;
    let ma = a.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
    let mb = b.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b) {
        let xa = f64::from(x) - ma;
        let yb = f64::from(y) - mb;
        num += xa * yb;
        da += xa * xa;
        db += yb * yb;
    }
    num / (da.sqrt() * db.sqrt())
}

fn gain_and_snr(ours: &[i32], reference: &[i32]) -> (f64, f64) {
    let x: Vec<f64> = ours.iter().map(|&v| f64::from(v)).collect();
    let y: Vec<f64> = reference.iter().map(|&v| f64::from(v)).collect();
    let g = x.iter().zip(&y).map(|(a, b)| a * b).sum::<f64>()
        / x.iter().map(|a| a * a).sum::<f64>().max(1e-30);
    let err: f64 = x.iter().zip(&y).map(|(a, b)| (g * a - b).powi(2)).sum();
    let sig: f64 = y.iter().map(|b| b * b).sum();
    (g, 10.0 * (sig / err.max(1e-30)).log10())
}

/// Exact framing: the reference reads the whole stream and emits
/// exactly `6 × 512` samples per channel; the default decoder emits
/// the same.
#[test]
fn framing_is_exact() {
    let reference = reference();
    assert_eq!(reference[0].len(), FRAMES * SAMPLES_PER_FRAME);
    assert_eq!(reference[1].len(), FRAMES * SAMPLES_PER_FRAME);
    let ours = ours_default();
    assert_eq!(ours[0].len(), FRAMES * SAMPLES_PER_FRAME);
    assert_eq!(ours[1].len(), FRAMES * SAMPLES_PER_FRAME);
}

/// Every frame — wide HF-VQ, HF-VQ + ADPCM, both `HFLAG` gates, the
/// ADPCM-heavy frame — is shape-identical to the reference.
#[test]
fn every_stress_frame_shape_identical() {
    let reference = reference();
    let ours = ours_default();
    for frame in 0..FRAMES {
        let a = frame * SAMPLES_PER_FRAME;
        let b = a + SAMPLES_PER_FRAME;
        for ch in 0..2 {
            let r = pearson(&ours[ch][a..b], &reference[ch][a..b]);
            assert!(
                r > 0.9999,
                "frame {frame} ch {ch}: Pearson {r} — expected shape-identical"
            );
        }
    }
}

/// After the single known constant (the √2 output `rScale` ratio),
/// every frame reconstructs above 90 dB SNR — the VQ/ADPCM frames as
/// much as the plain anchor.
#[test]
fn every_stress_frame_matches_at_90db_after_constant_gain() {
    let reference = reference();
    let ours = ours_default();
    let sqrt2 = 2f64.sqrt();
    for frame in 0..FRAMES {
        let a = frame * SAMPLES_PER_FRAME;
        let b = a + SAMPLES_PER_FRAME;
        for ch in 0..2 {
            let (g, snr) = gain_and_snr(&ours[ch][a..b], &reference[ch][a..b]);
            assert!(
                (g / sqrt2 - 1.0).abs() < 1e-3,
                "frame {frame} ch {ch}: gain {g} — expected the √2 rScale ratio"
            );
            assert!(snr > 90.0, "frame {frame} ch {ch}: SNR {snr} dB");
        }
    }
}

/// The frames really engage the §D.10 books: frames 2-6 carry VQ /
/// ADPCM energy (the whole stream is non-silent well above the noise
/// floor).
#[test]
fn stress_frames_carry_vq_energy() {
    let reference = reference();
    let peak = reference[0]
        .iter()
        .chain(&reference[1])
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap();
    assert!(
        peak > 1000,
        "stress stream must decode to real audio, peak {peak}"
    );
}
