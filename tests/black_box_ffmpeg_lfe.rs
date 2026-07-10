//! Black-box PCM validation of a **5.1 (LFE-bearing, five-primary-
//! channel) DTS Core stream** against a reference decode produced by
//! the `ffmpeg` binary — the multichannel + LFE companion to
//! `black_box_ffmpeg_pcm.rs`.
//!
//! Clean-room note: `ffmpeg` is used ONLY as an opaque black-box
//! encoder/decoder — its source is never consulted. The fixture pair
//! was produced once, out of band:
//!
//! ```text
//!   ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.1" \
//!          -f lavfi -i "sine=frequency=80:sample_rate=48000:duration=0.1" \
//!          -filter_complex \
//!            "[0:a][0:a][0:a][0:a][0:a][1:a]join=inputs=6:channel_layout=5.1[a]" \
//!          -map "[a]" -c:a dca -b:a 768k -strict -2 -f dts \
//!          tests/fixtures/dts_51_lfe.bin
//!   ffmpeg -f dts -i tests/fixtures/dts_51_lfe.bin \
//!          -f s32le -acodec pcm_s32le \
//!          tests/fixtures/dts_51_lfe_ffmpeg_ref.s32
//! ```
//!
//! (a 440 Hz sine on the five full-band channels, an 80 Hz sine on the
//! LFE input) and committed so CI — which has no `ffmpeg` — can run the
//! comparison deterministically.
//!
//! ## What this pins down
//!
//! * The frame headers report `AMODE 9` (`C L R SL SR`, 5 primary
//!   channels) + `LFE Mode2` (`LFF == 2` → the 64× §C.2.6 decimation
//!   filter), so the stream exercises the §5.5 **LFE phase** (the
//!   `2·LFF·nSSC` 8-bit samples + `LFEscaleIndex` + §C.2.6
//!   `InterpolationFIR()` chain of
//!   `docs/audio/dts/dts-lfe-interpolation-and-audio-walker.md` §2.2 /
//!   §1) on every subframe — a regression net over the corrected
//!   walker trace (spurious inner `nDeciIndex++` removed; every
//!   decimated sample consumed).
//! * Five-primary-channel §C.2.5 QMF reconstruction with the
//!   inter-frame filter tail (`CoreStreamDecoder`) — previously only
//!   validated on stereo.
//! * The LFE interpolation output lands **exactly** the per-frame
//!   primary PCM length (`nSSC·256` samples: `2·2·nSSC` decimated
//!   samples × 64), keeping the trailing LFE plane aligned.
//!
//! As in the stereo harness, the comparison is **shape-based**
//! (Pearson correlation + sign agreement): the §C.2.5 output `rScale`
//! is implementation-defined (non-normative), so a scale-invariant
//! check validates the whole reconstruction chain without baking in a
//! reference-derived gain constant. Measured on this fixture:
//! correlation 1.000000 on all five primary channels **and the LFE
//! channel** against the reference's respective planes.

use oxideav_dts::{iter_frames, AmodeArrangement, CoreStreamDecoder, LfeMode};

/// The 10-frame raw-16-bit 5.1 DTS Core fixture (48 kHz, AMODE 9 +
/// LFE, real `ffmpeg -c:a dca` output).
const FIXTURE: &[u8] = include_bytes!("fixtures/dts_51_lfe.bin");

/// `ffmpeg`'s reference decode of [`FIXTURE`] — interleaved s32le in
/// the reference's 5.1 channel order `FL FR FC LFE BL BR`, 5120
/// samples per channel (16 blocks × 32 samples × 10 frames).
const FFMPEG_REF_S32LE: &[u8] = include_bytes!("fixtures/dts_51_lfe_ffmpeg_ref.s32");

/// Reference channel count and interleave order (`FL FR FC LFE BL BR`).
const REF_CHANNELS: usize = 6;

/// Our DTS transmission-order primary planes are `C L R SL SR`
/// (AMODE 9); this maps each of ours onto the reference plane index.
const PRIMARY_TO_REF: [usize; 5] = [2, 0, 1, 4, 5];

/// The reference plane carrying the LFE channel.
const REF_LFE: usize = 3;

/// Decode the fixture through the streaming Core decoder: five planar
/// primary channels plus the accumulated LFE plane.
fn decode_ours() -> (Vec<Vec<i32>>, Vec<i32>) {
    let mut dec = CoreStreamDecoder::new(5);
    let mut primary: Vec<Vec<i32>> = vec![Vec::new(); 5];
    let mut lfe: Vec<i32> = Vec::new();
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames must iterate cleanly");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every 5.1 fixture frame must decode to PCM");
        assert_eq!(pcm.len(), 5);
        for (plane, frame_pcm) in primary.iter_mut().zip(&pcm) {
            plane.extend(frame_pcm);
        }
        let frame_lfe = dec.take_last_lfe_pcm();
        // The §C.2.6 interpolation must land exactly the primary
        // per-frame PCM length so the LFE plane stays aligned.
        assert_eq!(frame_lfe.len(), pcm[0].len());
        lfe.extend(frame_lfe);
    }
    (primary, lfe)
}

/// Deinterleave the committed reference into its six planar channels.
fn reference() -> Vec<Vec<i32>> {
    let mut out: Vec<Vec<i32>> = vec![Vec::new(); REF_CHANNELS];
    for (i, c) in FFMPEG_REF_S32LE.chunks_exact(4).enumerate() {
        out[i % REF_CHANNELS].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    out
}

/// Pearson correlation of two equal-length sample vectors.
fn pearson(a: &[i32], b: &[i32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let n = a.len() as f64;
    let mean_a = a.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let mean_b = b.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let (mut num, mut den_a, mut den_b) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b) {
        let dx = f64::from(x) - mean_a;
        let dy = f64::from(y) - mean_b;
        num += dx * dy;
        den_a += dx * dx;
        den_b += dy * dy;
    }
    num / (den_a.sqrt() * den_b.sqrt())
}

/// Fraction of positions where the two signals agree in sign
/// (treating zero as agreeing with anything).
fn sign_agreement(a: &[i32], b: &[i32]) -> f64 {
    let agree = a
        .iter()
        .zip(b)
        .filter(|(&x, &y)| x == 0 || y == 0 || (x > 0) == (y > 0))
        .count();
    agree as f64 / a.len() as f64
}

/// Every fixture frame reports the expected 5.1 layout: AMODE 9
/// (`C L R SL SR`) plus the 64×-decimated LFE channel.
#[test]
fn fixture_frames_are_amode9_with_lfe() {
    let mut frames = 0usize;
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames must parse");
        assert_eq!(fv.header.amode, 9);
        assert_eq!(
            fv.header.amode_arrangement(),
            AmodeArrangement::ClRSlSr,
            "AMODE 9 is C L R SL SR"
        );
        assert_eq!(fv.header.channel_count(), Some(5));
        assert_eq!(fv.header.lfe, LfeMode::Mode2); // LFF == 2 -> 64×
        assert_eq!(fv.header.sample_rate_hz(), Some(48_000));
        frames += 1;
    }
    assert_eq!(frames, 10);
}

/// The full 5.1 decode is shape-identical to the black-box reference:
/// every primary channel and the LFE channel correlate at ≥ 0.999999
/// with the reference plane it maps to, with ≥ 99.9 % sign agreement.
#[test]
fn decodes_51_lfe_fixture_matching_reference_shape() {
    let (primary, lfe) = decode_ours();
    let reference = reference();

    assert_eq!(primary[0].len(), reference[0].len());
    assert_eq!(lfe.len(), reference[REF_LFE].len());

    for (ours_idx, &ref_idx) in PRIMARY_TO_REF.iter().enumerate() {
        let corr = pearson(&primary[ours_idx], &reference[ref_idx]);
        assert!(
            corr > 0.999_999,
            "primary plane {ours_idx} vs reference plane {ref_idx}: correlation {corr}"
        );
        let signs = sign_agreement(&primary[ours_idx], &reference[ref_idx]);
        assert!(
            signs > 0.999,
            "primary plane {ours_idx}: sign agreement {signs}"
        );
    }

    let lfe_corr = pearson(&lfe, &reference[REF_LFE]);
    assert!(
        lfe_corr > 0.999_999,
        "LFE plane vs reference plane {REF_LFE}: correlation {lfe_corr}"
    );
    let lfe_signs = sign_agreement(&lfe, &reference[REF_LFE]);
    assert!(lfe_signs > 0.999, "LFE plane: sign agreement {lfe_signs}");
}

/// The LFE channel genuinely comes from the LFE payload, not a
/// full-band channel: it correlates with the reference LFE plane and
/// essentially not at all with the reference front planes (which carry
/// the 440 Hz tone instead of the 80 Hz one).
#[test]
fn lfe_plane_is_the_lfe_signal_not_a_fullband_copy() {
    let (_, lfe) = decode_ours();
    let reference = reference();
    assert!(pearson(&lfe, &reference[REF_LFE]) > 0.999_999);
    for front in [0usize, 1, 2] {
        let corr = pearson(&lfe, &reference[front]).abs();
        assert!(
            corr < 0.1,
            "LFE must not correlate with front plane {front}: {corr}"
        );
    }
}
