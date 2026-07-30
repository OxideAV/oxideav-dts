//! Black-box validation of the **§D.10-bearing** stream fixture
//! (`tests/fixtures/dts_d10_5_frames.bin`) against a reference decode
//! produced by the `ffmpeg` binary (used ONLY as an opaque reference
//! decoder — its source is never consulted).
//!
//! The fixture is spec-built (`tests/common/mod.rs`,
//! [`common::build_d10_stream`]): two plain normal stereo frames, an
//! HF-VQ frame (`nVQSUB < nSUBS`, 10-bit phase-1 indices), an ADPCM
//! frame (`PMODE = 1`, 12-bit PVQ plane, `HFLAG = 0`) and a combined
//! HF-VQ + ADPCM frame with `HFLAG = 1`. The reference PCM was
//! produced once, out of band:
//!
//! ```text
//!   ffmpeg -f dts -i dts_d10_5_frames.bin -f s32le \
//!          -acodec pcm_s32le dts_d10_5_frames_ffmpeg_ref.s32
//! ```
//!
//! **What this pins — and what it cannot.** The reference decoder
//! carries the *real* §D.10 books, whose numeric contents are the
//! recorded docs gap; our decode of the §D.10 frames uses synthetic
//! stand-in books, so their PCM is **not comparable numerically**.
//! What the reference run *does* pin, black-box:
//!
//! * **acceptance + exact framing** — the reference reads the whole
//!   10 000-byte stream, reports zero decode errors, and emits
//!   exactly `5 × 512` samples per channel. Our spec-built VQSUB
//!   plane, PMODE/PVQ planes, phase-1 index region, and HF-tail
//!   SCALES loop are therefore bit-compatible with a real decoder's
//!   §5.3.2/§5.4.1/§5.5 walk (any mis-sized field would desync the
//!   DSYNC checks or the frame framing);
//! * **the §D.10 frames really engage the books** — the reference's
//!   decode of frames 3-5 is non-silent and differs from its decode
//!   of the plain frames;
//! * **the books-independent prefix matches** — frames 1-2 contain no
//!   §D.10 material, so our PCM is shape-identical to the reference
//!   there (Pearson correlation ≈ 1.0, as for the other fixtures).
//!
//! The §D.10 frames' numeric decode is validated in-crate against
//! analytic reconstructions in `tests/d10_vq_decode.rs`.

mod common;

use common::{build_d10_stream, synthetic_vq_codebooks};
use oxideav_dts::{iter_frames, CoreStreamDecoder};

const FIXTURE: &[u8] = include_bytes!("fixtures/dts_d10_5_frames.bin");
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_d10_5_frames_ffmpeg_ref.s32");

const FRAMES: usize = 5;
const SAMPLES_PER_FRAME: usize = 512;
/// The first two frames carry no §D.10 material.
const PLAIN_PREFIX_FRAMES: usize = 2;

/// The committed fixture is byte-for-byte the deterministic builder's
/// output — its provenance is `tests/common/mod.rs`, not an opaque
/// binary.
#[test]
fn committed_fixture_matches_builder() {
    assert_eq!(
        FIXTURE,
        build_d10_stream().as_slice(),
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

fn ours() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    dec.set_vq_codebooks(synthetic_vq_codebooks());
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every fixture frame decodes with the stand-in books");
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

/// Acceptance + exact framing: the reference decodes all five frames
/// — including the HF-VQ / PMODE ones — to exactly `5 × 512` samples
/// per channel. Our decode (stand-in books) emits the same lengths.
#[test]
fn reference_accepts_all_five_frames_exactly() {
    let reference = reference();
    assert_eq!(reference[0].len(), FRAMES * SAMPLES_PER_FRAME);
    assert_eq!(reference[1].len(), FRAMES * SAMPLES_PER_FRAME);

    let ours = ours();
    assert_eq!(ours[0].len(), FRAMES * SAMPLES_PER_FRAME);
    assert_eq!(ours[1].len(), FRAMES * SAMPLES_PER_FRAME);
}

/// The reference really engages its §D.10 books on frames 3-5: that
/// region is non-silent (the HF-VQ subbands and predicted residuals
/// carry energy through its decode).
#[test]
fn reference_decodes_d10_frames_to_audio() {
    let reference = reference();
    let tail = &reference[0][PLAIN_PREFIX_FRAMES * SAMPLES_PER_FRAME..];
    let peak = tail.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(
        peak > 1000,
        "§D.10 frames must decode to real audio, peak {peak}"
    );
}

/// The books-independent prefix (frames 1-2) is shape-identical to
/// the reference, as for the other fixtures — pinning that the §D.10
/// knobs did not perturb the plain-frame path.
#[test]
fn plain_prefix_is_shape_identical_to_reference() {
    let reference = reference();
    let ours = ours();
    let len = PLAIN_PREFIX_FRAMES * SAMPLES_PER_FRAME;
    for ch in 0..2 {
        let r = pearson(&ours[ch][..len], &reference[ch][..len]);
        assert!(
            r > 0.999,
            "channel {ch} prefix correlation {r} — expected shape-identical"
        );
    }
}

/// Every fixture frame parses with the intended §D.10 shape: frames
/// 1-2 plain, frame 3 HF-VQ, frame 4 PMODE, frame 5 both.
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
    assert_eq!(shapes.len(), FRAMES);
    assert_eq!(shapes[0], (0, 0, false));
    assert_eq!(shapes[1], (0, 0, false));
    assert_eq!(shapes[2], (12, 0, false), "frame 3: 8+4 HF-VQ subbands");
    assert_eq!(shapes[3], (0, 6, false), "frame 4: 4+2 PMODE subbands");
    assert_eq!(shapes[4], (8, 4, true), "frame 5: both, HFLAG = 1");
}
