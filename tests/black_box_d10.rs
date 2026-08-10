//! Black-box validation of the **§D.10-bearing** stream fixture
//! (`tests/fixtures/dts_d10_5_frames.bin`) against a reference decode
//! produced by the `ffmpeg` binary (invoked as an opaque black-box
//! reference decoder; its output is treated as opaque reference data).
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
//! **What this pins.** The reference decoder carries the *real*
//! §D.10 books. Since round 439 so do we
//! ([`oxideav_dts::VqCodebooks::builtin`], transcribed from the
//! staged clean-room tables), so the §D.10 frames are compared
//! **numerically**, not just structurally:
//!
//! * **acceptance + exact framing** — the reference reads the whole
//!   10 000-byte stream, reports zero decode errors, and emits
//!   exactly `5 × 512` samples per channel;
//! * **all five frames are shape-identical** — with the built-in
//!   books our decode of the HF-VQ frame (3), the ADPCM frame (4)
//!   and the combined `HFLAG = 1` frame (5) matches the reference at
//!   Pearson ≈ 1.0 and > 90 dB SNR after the one known constant: the
//!   implementation-defined output `rScale` ratio (√2 for this
//!   fixture's `PCMR`, exactly as pinned for the plain fixtures —
//!   see [`oxideav_dts::DtsFrameHeader::output_r_scale`] and the
//!   round-356 record). This end-to-end confirms the recovered book
//!   values, the §D.10.2 ÷ 2⁴ element divisor (the spec's printed
//!   "24" is a typo — a per-subband 2/3 gain error would break the
//!   mixed VQ/non-VQ frame correlation), the low-byte-first
//!   intra-entry order, and the §C.2.2 predictor-history chain
//!   across the `HFLAG` gate;
//! * **the drop-in API still works** — the synthetic stand-in books
//!   ([`common::synthetic_vq_codebooks`]) decode the same stream to
//!   the same lengths, with the books-independent prefix (frames
//!   1-2) shape-identical to the reference.
//!
//! The §D.10 frames' bit-exact numeric decode is additionally
//! validated in-crate against analytic reconstructions in
//! `tests/d10_vq_decode.rs`.

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

/// Decode the whole stream with the **built-in real books** (the
/// round-439 default), returning planar PCM.
fn ours_real_books() -> Vec<Vec<i32>> {
    let mut dec = CoreStreamDecoder::new(2);
    dec.set_vq_codebooks(oxideav_dts::VqCodebooks::builtin());
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every fixture frame decodes with the built-in books");
        for (ch, samples) in block.into_iter().enumerate() {
            pcm[ch].extend(samples);
        }
    }
    pcm
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

/// **The round-439 headline**: with the built-in real books, every
/// frame — including the HF-VQ frame (3), the ADPCM frame (4), and
/// the combined `HFLAG = 1` frame (5) — is shape-identical to the
/// reference decode (Pearson > 0.9999 per frame, per channel).
/// Before the books landed, frames 3-5 were only structurally
/// checkable; a wrong §D.10.2 divisor or intra-entry order would
/// break this on the mixed VQ/non-VQ frames.
#[test]
fn real_books_shape_identical_on_all_frames() {
    let reference = reference();
    let ours = ours_real_books();
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

/// The only difference from the reference is the one known constant:
/// the implementation-defined output `rScale` ratio, √2 for this
/// fixture. After that single gain, every frame reconstructs above
/// 90 dB SNR (measured ≈ 95-98 dB), on the §D.10 frames as much as
/// on the plain ones.
#[test]
fn real_books_match_reference_at_90db_after_constant_gain() {
    let reference = reference();
    let ours = ours_real_books();
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

/// The registry surface (`make_decoder` → `send_packet` /
/// `receive_frame`) decodes the §D.10 stream **by default** — no
/// `set_vq_codebooks` call anywhere — bit-identically to the direct
/// default-book `CoreStreamDecoder` path, which the tests above pin
/// against the reference. This is the round-439 flip made visible at
/// the public framework surface: a stream that used to map to
/// `Unsupported` at frames 3-5 now emits five audio frames.
#[test]
fn registry_default_decodes_d10_stream() {
    use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, TimeBase};

    let params = CodecParameters::audio(CodecId::new(oxideav_dts::CODEC_ID_STR));
    let mut dec: Box<dyn Decoder> = oxideav_dts::make_decoder(&params).expect("factory builds");
    let mut via_registry: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), fv.data.to_vec());
        dec.send_packet(&pkt)
            .expect("send_packet accepts the frame");
        let frame = dec.receive_frame().expect("§D.10 frame decodes by default");
        let Frame::Audio(audio) = frame else {
            panic!("expected an audio frame");
        };
        assert_eq!(audio.data.len(), 2, "stereo planar output");
        assert_eq!(audio.samples as usize, SAMPLES_PER_FRAME);
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                via_registry[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }

    // Bit-identical to the direct default-book stream decode (which
    // real_books_shape_identical_on_all_frames pins to the reference,
    // since the built-in books ARE the default).
    let mut direct = CoreStreamDecoder::new(2);
    let mut direct_pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("frames iterate");
        let pcm = direct.decode_frame(fv.data, &fv.header).expect("decodes");
        for ch in 0..2 {
            direct_pcm[ch].extend(&pcm[ch]);
        }
    }
    assert_eq!(via_registry, direct_pcm, "registry and direct paths agree");
}

/// The default decoder path IS the built-in-book path: the direct
/// default `CoreStreamDecoder` (no `set_vq_codebooks` call) produces
/// exactly the PCM of the explicit-builtin decode used above.
#[test]
fn default_decoder_equals_explicit_builtin_books() {
    let mut dec = CoreStreamDecoder::new(2);
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("frames iterate");
        let block = dec.decode_frame(fv.data, &fv.header).expect("decodes");
        for (ch, samples) in block.into_iter().enumerate() {
            pcm[ch].extend(samples);
        }
    }
    assert_eq!(pcm, ours_real_books());
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
