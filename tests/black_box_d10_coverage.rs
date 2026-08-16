//! Black-box **book-coverage** validation of the §D.10 built-in code
//! books: the committed 12-frame stream
//! (`tests/fixtures/dts_d10_coverage_12_frames.bin`, spec-built by
//! [`common::build_d10_coverage_stream`]) sweeps **160 distinct
//! §D.10.2 vectors and 320 distinct §D.10.1 vectors** — roughly ten
//! times the index sample of the earlier §D.10 fixtures — and our
//! default decode is compared numerically against a reference decode
//! produced by the `ffmpeg` binary (invoked as an opaque black-box
//! reference decoder), produced once, out of band:
//!
//! ```text
//!   ffmpeg -f dts -i dts_d10_coverage_12_frames.bin -f s32le \
//!          -acodec pcm_s32le dts_d10_coverage_12_frames_ffmpeg_ref.s32
//! ```
//!
//! The swept regions are chosen against the staged recovery record
//! (`docs/audio/dts/tables/dts-d10-2-hfreq-vq.meta.md`): the §D.10.2
//! bases cover the book head (0..=31 — including the unique all-zero
//! vector 0), the **duplicate-codeword cluster** (342..=373, the 22
//! repeating high-energy patterns the two licensee builds agree on),
//! the middle (512..=543), and the tail (992..=1023); the §D.10.1
//! bases cover 0..=63 (the spec-anchored index 0), 1024..=1087,
//! 2048..=2111, and the tail 4032..=4095; the two combined `HFLAG = 1`
//! frames add 640..=655 / 800..=815 and 3000..=3031 / 3500..=3531
//! with the §C.2.2 history primed across the frame boundary.
//!
//! With every one of the 480 swept vectors engaged, our decode is
//! shape-identical to the reference on **all twelve frames** (Pearson
//! 1.000000 per frame per channel) and matches above 90 dB SNR after
//! the one implementation-defined constant (the output `rScale` ratio,
//! √2 for this fixture's `PCMR`) — a broad behavioural confirmation of
//! the recovered book values, the §D.10.2 ÷ 2⁴ divisor and
//! low-byte-first element order, and the §C.2.2 predictor chain, on
//! index regions the narrow fixtures never touched. The same 480
//! vectors (and the whole rest of both books) are additionally pinned
//! bit-exactly against analytic reconstructions by the full
//! index-space sweeps in `tests/d10_vq_decode.rs`.

mod common;

use common::{build_d10_coverage_stream, d10_coverage_specs, spec_hf_vq_index, spec_pvq_index};
use oxideav_dts::{iter_frames, CoreStreamDecoder};

const FIXTURE: &[u8] = include_bytes!("fixtures/dts_d10_coverage_12_frames.bin");
const FFMPEG_REF_S32LE: &[u8] =
    include_bytes!("fixtures/dts_d10_coverage_12_frames_ffmpeg_ref.s32");

const FRAMES: usize = 12;
const SAMPLES_PER_FRAME: usize = 512;

/// The committed fixture is byte-for-byte the deterministic builder's
/// output — its provenance is `tests/common/mod.rs`, not an opaque
/// binary.
#[test]
fn committed_fixture_matches_builder() {
    assert_eq!(
        FIXTURE,
        build_d10_coverage_stream().as_slice(),
        "fixture must re-derive from the deterministic builder"
    );
}

/// The stream really carries the intended index coverage: 160
/// distinct §D.10.2 indices (including the whole duplicate cluster
/// 342..=373 and both book ends) and 320 distinct §D.10.1 indices
/// (including the spec-anchored 0 and the book tail 4095).
#[test]
fn fixture_sweeps_the_documented_index_regions() {
    let mut hf = [false; 1024];
    let mut adpcm = [false; 4096];
    for spec in &d10_coverage_specs() {
        let n_vqsub = spec.n_vqsub();
        for subframe in 0..spec.n_subframes {
            for ch in 0..2 {
                for n in n_vqsub[ch]..spec.n_subs[ch] {
                    hf[spec_hf_vq_index(spec, subframe, ch, n) as usize] = true;
                }
                for n in 0..spec.adpcm_subbands[ch] {
                    adpcm[spec_pvq_index(spec, subframe, ch, n) as usize] = true;
                }
            }
        }
    }
    assert_eq!(hf.iter().filter(|&&c| c).count(), 160);
    assert_eq!(adpcm.iter().filter(|&&c| c).count(), 320);
    assert!(
        (342..=373).all(|i| hf[i]),
        "the §D.10.2 duplicate-codeword cluster must be swept"
    );
    assert!(hf[0] && hf[1023] && adpcm[0] && adpcm[4095]);
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
/// built-in real books, no `set_vq_codebooks` call anywhere.
fn ours() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every swept frame decodes with the built-in books");
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
    let mut num = 0.0;
    let mut da = 0.0;
    let mut db = 0.0;
    for (&x, &y) in a.iter().zip(b) {
        let xa = f64::from(x) - ma;
        let yb = f64::from(y) - mb;
        num += xa * yb;
        da += xa * xa;
        db += yb * yb;
    }
    num / (da.sqrt() * db.sqrt())
}

/// Per-slice least-squares gain of `ours` onto `reference`, plus the
/// SNR of the gain-aligned residual (dB re the reference energy).
fn gain_and_snr(ours: &[i32], reference: &[i32]) -> (f64, f64) {
    let x: Vec<f64> = ours.iter().map(|&v| f64::from(v)).collect();
    let y: Vec<f64> = reference.iter().map(|&v| f64::from(v)).collect();
    let g = x.iter().zip(&y).map(|(a, b)| a * b).sum::<f64>()
        / x.iter().map(|a| a * a).sum::<f64>().max(1e-30);
    let err: f64 = x.iter().zip(&y).map(|(a, b)| (g * a - b).powi(2)).sum();
    let sig: f64 = y.iter().map(|b| b * b).sum();
    (g, 10.0 * (sig / err.max(1e-30)).log10())
}

/// Acceptance + exact framing: the reference reads the whole
/// 24 000-byte stream and emits exactly `12 × 512` samples per
/// channel; so do we.
#[test]
fn reference_accepts_all_twelve_frames_exactly() {
    let reference = reference();
    let ours = ours();
    for ch in 0..2 {
        assert_eq!(reference[ch].len(), FRAMES * SAMPLES_PER_FRAME);
        assert_eq!(ours[ch].len(), FRAMES * SAMPLES_PER_FRAME);
    }
}

/// Every frame — head/cluster/middle/tail HF-VQ sweeps, the four
/// all-32-subband ADPCM sweeps, and the two `HFLAG = 1` combined
/// frames — is shape-identical to the reference decode (Pearson
/// > 0.9999 per frame, per channel).
#[test]
fn every_swept_frame_is_shape_identical_to_reference() {
    let reference = reference();
    let ours = ours();
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

/// After the single known constant (the implementation-defined output
/// `rScale` ratio, √2 for this fixture), every frame reconstructs
/// above 90 dB SNR (measured ≈ 90.5-95.9 dB) — on the swept-index
/// frames as much as on the plain lead-in.
#[test]
fn every_swept_frame_matches_reference_at_90db_after_constant_gain() {
    let reference = reference();
    let ours = ours();
    let sqrt2 = 2f64.sqrt();
    for frame in 0..FRAMES {
        let a = frame * SAMPLES_PER_FRAME;
        let b = a + SAMPLES_PER_FRAME;
        for ch in 0..2 {
            let (g, snr) = gain_and_snr(&ours[ch][a..b], &reference[ch][a..b]);
            assert!(
                (g / sqrt2 - 1.0).abs() < 1e-4,
                "frame {frame} ch {ch}: gain {g} — expected the √2 rScale ratio"
            );
            assert!(snr > 90.0, "frame {frame} ch {ch}: SNR {snr} dB");
        }
    }
}

/// Every fixture frame parses with the intended §D.10 shape: 2 plain,
/// 4 HF-VQ (32 VQ subbands each), 4 ADPCM (64 `PMODE` subbands each),
/// and 2 combined frames with `HFLAG = 1`.
#[test]
fn fixture_frames_carry_the_intended_d10_shape() {
    let mut shapes = Vec::new();
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let hb = fv.header.header_bit_length() as usize;
        let (coding, ach_bits) =
            oxideav_dts::decode_audio_coding_header_at(fv.data, hb, fv.header.crc_present)
                .expect("audio coding header parses");
        let (side, _) = oxideav_dts::decode_primary_side_info_at(
            fv.data,
            hb + ach_bits,
            &coding.channel_params,
        )
        .expect("side info parses");
        let hf: usize = (0..2)
            .map(|ch| coding.n_subs()[ch] - coding.n_vqsub()[ch])
            .sum();
        let pmode: usize = side
            .channels
            .iter()
            .map(|c| c.pmode.iter().filter(|&&p| p != 0).count())
            .sum();
        shapes.push((hf, pmode, fv.header.predictor_history));
    }
    let expected: Vec<(usize, usize, bool)> = [(0, 0, false); 2]
        .into_iter()
        .chain([(32, 0, false); 4])
        .chain([(0, 64, false); 4])
        .chain([(16, 32, true); 2])
        .collect();
    assert_eq!(shapes, expected);
}
