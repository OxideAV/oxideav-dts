//! Black-box validation of the round-453 **encoder** against the
//! opaque reference decoder (the `ffmpeg` binary), the encoder-side
//! companion of `black_box_ffmpeg_pcm.rs` / `black_box_ffmpeg_lfe.rs`.
//!
//! Clean-room note: `ffmpeg` is used only as an opaque decoder of
//! streams *this crate's encoder* produced; its output is opaque
//! reference PCM. Each fixture pair was produced once, out of band:
//!
//! ```text
//!   # the .dts stream: CoreEncoder over the deterministic test signal
//!   # below (see `signal()`), written by this test's regeneration
//!   # path (DTS_ENCODER_FIXTURE_DIR=<dir> cargo test --test black_box_encoder)
//!   ffmpeg -f dts -i tests/fixtures/enc_<name>.dts \
//!          -f s32le -acodec pcm_s32le tests/fixtures/enc_<name>_ffmpeg_ref.s32
//! ```
//!
//! and committed so CI — which has no `ffmpeg` — can run the
//! comparison deterministically. The reference decoder emitted **no
//! diagnostics** on any of the three streams.
//!
//! ## What this pins down
//!
//! * The reference decoder reconstructs the *original input signal*
//!   from our streams: sample-close SNR floors per fixture, unity
//!   gain — i.e. the encoder's scale/step/level conventions are the
//!   ones a third-party decoder applies, not just our own decoder's.
//! * Our decoder and the reference agree on our streams (≥ 60 dB
//!   ours-vs-reference on every primary plane; the LFE plane agrees
//!   to > 100 dB) — the two decoders differ only in their float
//!   rounding, with the round-453 output calibration in place.
//! * The reference reports the channel layout / rate the headers
//!   claim (5.1 streams come back as six planes in the reference's
//!   `FL FR FC LFE SL SR` order, mapped below).

use oxideav_dts::{iter_frames, parse_frame_header, CoreStreamDecoder};

/// Frames per fixture.
const FRAMES: usize = 8;
const SAMPLES: usize = FRAMES * 512;
/// Steady-state measurement start (one frame of §C.2.5 priming).
const PRIMING: usize = 512;

const STEREO_768K: &[u8] = include_bytes!("fixtures/enc_stereo_768k.dts");
const STEREO_768K_REF: &[u8] = include_bytes!("fixtures/enc_stereo_768k_ffmpeg_ref.s32");
const STEREO_192K: &[u8] = include_bytes!("fixtures/enc_stereo_192k.dts");
const STEREO_192K_REF: &[u8] = include_bytes!("fixtures/enc_stereo_192k_ffmpeg_ref.s32");
const FIVE_ONE_768K: &[u8] = include_bytes!("fixtures/enc_51_lfe_768k.dts");
const FIVE_ONE_768K_REF: &[u8] = include_bytes!("fixtures/enc_51_lfe_768k_ffmpeg_ref.s32");
const MONO_TRANSIENT: &[u8] = include_bytes!("fixtures/enc_mono_transient_768k.dts");
const MONO_TRANSIENT_REF: &[u8] = include_bytes!("fixtures/enc_mono_transient_768k_ffmpeg_ref.s32");
const STEREO_128K_VQ: &[u8] = include_bytes!("fixtures/enc_stereo_128k_hfvq.dts");
const STEREO_128K_VQ_REF: &[u8] = include_bytes!("fixtures/enc_stereo_128k_hfvq_ffmpeg_ref.s32");

/// The deterministic multitone every fixture was encoded from
/// (identical to `encoder_round_trip.rs`).
fn multitone(n: usize, phase: f64) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            0.20 * (2.0 * std::f64::consts::PI * (440.0 * t + phase)).sin()
                + 0.12 * (2.0 * std::f64::consts::PI * (1_330.0 * t + phase)).sin()
                + 0.08 * (2.0 * std::f64::consts::PI * (3_700.0 * t + 0.3 * phase)).cos()
                + 0.05 * (2.0 * std::f64::consts::PI * (9_200.0 * t)).sin()
                + 0.02 * (2.0 * std::f64::consts::PI * (17_500.0 * t)).sin()
        })
        .collect()
}

/// Input planes per fixture (our plane order: primaries in Table 5-4
/// order, LFE last).
fn signal(name: &str) -> Vec<Vec<f64>> {
    match name {
        "stereo" => vec![multitone(SAMPLES, 0.0), multitone(SAMPLES, 0.7)],
        "five_one" => {
            let mut p: Vec<Vec<f64>> = (0..5)
                .map(|ch| multitone(SAMPLES, ch as f64 * 0.4))
                .collect();
            p.push(
                (0..SAMPLES)
                    .map(|i| 0.25 * (2.0 * std::f64::consts::PI * 60.0 * i as f64 / 48_000.0).sin())
                    .collect(),
            );
            p
        }
        // Quiet tone with one loud burst starting at the second
        // subsubframe of frame 2: exercises TMODE (two scale factors).
        "mono_transient" => vec![(0..SAMPLES)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                let quiet = 0.002 * (2.0 * std::f64::consts::PI * 700.0 * t).sin();
                let burst = if (1280..1280 + 400).contains(&i) {
                    0.6 * (2.0 * std::f64::consts::PI * 2_900.0 * t).sin()
                } else {
                    0.0
                };
                quiet + burst
            })
            .collect()],
        // Multitone plus a −40 dBFS 20 kHz tone: at 128 kbit/s the top
        // band goes out as §D.10.2 high-frequency VQ.
        "stereo_hfvq" => (0..2)
            .map(|ch| {
                multitone(SAMPLES, ch as f64 * 0.6)
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        v + 0.01
                            * (2.0 * std::f64::consts::PI * 20_000.0 * i as f64 / 48_000.0).sin()
                    })
                    .collect()
            })
            .collect(),
        _ => unreachable!(),
    }
}

fn reference(bytes: &[u8], planes: usize) -> Vec<Vec<f64>> {
    let mut out = vec![Vec::new(); planes];
    for (i, c) in bytes.chunks_exact(4).enumerate() {
        let v = i32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        out[i % planes].push(f64::from(v) / 2f64.powi(31));
    }
    out
}

fn ours(stream: &[u8], channels: usize, lfe: bool) -> Vec<Vec<f64>> {
    let mut dec = CoreStreamDecoder::new(channels);
    let mut out = vec![Vec::new(); channels + usize::from(lfe)];
    for fv in iter_frames(stream) {
        let fv = fv.expect("fixture frames iterate");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("fixture decodes");
        for (ch, samples) in block.iter().enumerate() {
            out[ch].extend(samples.iter().map(|&s| f64::from(s) / 2f64.powi(31)));
        }
        if lfe {
            out[channels].extend(
                dec.take_last_lfe_pcm()
                    .iter()
                    .map(|&s| f64::from(s) / 2f64.powi(31)),
            );
        }
    }
    out
}

/// `(snr_db, gain)` of `test` against `reference`, steady state.
fn snr_gain(reference: &[f64], test: &[f64]) -> (f64, f64) {
    let n = reference.len().min(test.len());
    let r = &reference[PRIMING..n];
    let t = &test[PRIMING..n];
    let sig: f64 = r.iter().map(|v| v * v).sum();
    let err: f64 = r.iter().zip(t).map(|(a, b)| (a - b) * (a - b)).sum();
    let dot: f64 = r.iter().zip(t).map(|(a, b)| a * b).sum();
    (10.0 * (sig / err.max(1e-300)).log10(), dot / sig)
}

/// Reference plane index for each of our planes (5.1: ours
/// `C L R SL SR LFE` → reference `FL FR FC LFE SL SR`).
const FIVE_ONE_MAP: [usize; 6] = [2, 0, 1, 4, 5, 3];

#[test]
fn fixtures_parse_as_the_configured_streams() {
    let h = parse_frame_header(STEREO_768K).unwrap();
    assert_eq!(
        (h.channel_count(), h.sample_rate_hz(), h.bit_rate_bps()),
        (Some(2), Some(48_000), Some(768_000))
    );
    assert_eq!(h.frame_size_bytes, 1024);
    assert_eq!(iter_frames(STEREO_768K).count(), FRAMES);
    let h = parse_frame_header(STEREO_192K).unwrap();
    assert_eq!(
        (h.channel_count(), h.bit_rate_bps()),
        (Some(2), Some(192_000))
    );
    assert_eq!(h.frame_size_bytes, 256);
    assert_eq!(iter_frames(STEREO_192K).count(), FRAMES);
    let h = parse_frame_header(FIVE_ONE_768K).unwrap();
    assert_eq!((h.channel_count(), h.amode), (Some(5), 9));
    assert!(h.lfe.is_present());
    assert_eq!(iter_frames(FIVE_ONE_768K).count(), FRAMES);
    // Reference plane counts match (6 planes for 5.1).
    assert_eq!(STEREO_768K_REF.len(), SAMPLES * 2 * 4);
    assert_eq!(STEREO_192K_REF.len(), SAMPLES * 2 * 4);
    assert_eq!(FIVE_ONE_768K_REF.len(), SAMPLES * 6 * 4);
}

#[test]
fn reference_decoder_reconstructs_the_input_stereo_768k() {
    let input = signal("stereo");
    let reference = reference(STEREO_768K_REF, 2);
    for ch in 0..2 {
        let (snr, gain) = snr_gain(&input[ch], &reference[ch]);
        assert!(snr > 60.0, "ch {ch}: reference SNR vs input {snr:.1} dB");
        assert!((gain - 1.0).abs() < 1e-3, "ch {ch}: gain {gain}");
    }
}

#[test]
fn reference_decoder_reconstructs_the_input_stereo_192k() {
    let input = signal("stereo");
    let reference = reference(STEREO_192K_REF, 2);
    for ch in 0..2 {
        let (snr, gain) = snr_gain(&input[ch], &reference[ch]);
        assert!(snr > 30.0, "ch {ch}: reference SNR vs input {snr:.1} dB");
        assert!((gain - 1.0).abs() < 5e-3, "ch {ch}: gain {gain}");
    }
}

#[test]
fn reference_decoder_reconstructs_the_input_five_one_lfe() {
    let input = signal("five_one");
    let reference = reference(FIVE_ONE_768K_REF, 6);
    for (ours_idx, &ref_idx) in FIVE_ONE_MAP.iter().enumerate() {
        let (snr, gain) = snr_gain(&input[ours_idx], &reference[ref_idx]);
        let (floor, tol) = if ours_idx == 5 {
            (22.0, 2e-2)
        } else {
            (40.0, 1e-3)
        };
        assert!(
            snr > floor,
            "plane {ours_idx} (ref {ref_idx}): reference SNR vs input {snr:.1} dB"
        );
        assert!((gain - 1.0).abs() < tol, "plane {ours_idx}: gain {gain}");
    }
}

#[test]
fn reference_decoder_reconstructs_the_transient_and_hf_vq_streams() {
    // TMODE / two-scale-factor path (mono burst) and the §D.10.2
    // high-frequency VQ path (128k with a 20 kHz tone) decode on the
    // reference at unity gain.
    let input = signal("mono_transient");
    let ref_mono = reference(MONO_TRANSIENT_REF, 1);
    let (snr, gain) = snr_gain(&input[0], &ref_mono[0]);
    assert!(snr > 40.0, "transient: reference SNR vs input {snr:.1} dB");
    assert!((gain - 1.0).abs() < 1e-3, "transient: gain {gain}");
    // The burst frame (frame 2) carries TMODE transients.
    let burst_frame: Vec<&[u8]> = iter_frames(MONO_TRANSIENT)
        .map(|f| f.unwrap().data)
        .collect();
    let (_, tmodes) = frame_structure(burst_frame[2]);
    assert!(tmodes[0].iter().any(|&t| t > 0), "burst frame flags TMODE");
    let (subs, _) = frame_structure(STEREO_128K_VQ);
    assert!(
        subs.iter().any(|&(s, v)| s > v),
        "128k fixture carries HF-VQ bands"
    );
    let input = signal("stereo_hfvq");
    let ref_vq = reference(STEREO_128K_VQ_REF, 2);
    for ch in 0..2 {
        let (snr, gain) = snr_gain(&input[ch], &ref_vq[ch]);
        assert!(
            snr > 15.0,
            "hf-vq ch {ch}: reference SNR vs input {snr:.1} dB"
        );
        assert!((gain - 1.0).abs() < 2e-2, "hf-vq ch {ch}: gain {gain}");
    }
}

/// First-frame structure of a stream: `(nSUBS, nVQSUB)` per channel
/// and the per-band `TMODE` vectors.
fn frame_structure(frame: &[u8]) -> (Vec<(usize, usize)>, Vec<Vec<u8>>) {
    let hdr = parse_frame_header(frame).unwrap();
    let hb = hdr.header_bit_length() as usize;
    let (coding, bits) =
        oxideav_dts::decode_audio_coding_header_at(frame, hb, hdr.crc_present).unwrap();
    let (side, _) =
        oxideav_dts::decode_primary_side_info_at(frame, hb + bits, &coding.channel_params).unwrap();
    (
        coding
            .channel_params
            .iter()
            .map(|p| (p.n_subs, p.n_vqsub))
            .collect(),
        side.channels.iter().map(|c| c.tmode.to_vec()).collect(),
    )
}

#[test]
fn our_decoder_agrees_with_the_reference_on_our_streams() {
    for (name, stream, refb, channels, lfe) in [
        ("stereo_768k", STEREO_768K, STEREO_768K_REF, 2usize, false),
        ("stereo_192k", STEREO_192K, STEREO_192K_REF, 2, false),
        ("five_one_768k", FIVE_ONE_768K, FIVE_ONE_768K_REF, 5, true),
        (
            "mono_transient",
            MONO_TRANSIENT,
            MONO_TRANSIENT_REF,
            1,
            false,
        ),
        (
            "stereo_128k_hfvq",
            STEREO_128K_VQ,
            STEREO_128K_VQ_REF,
            2,
            false,
        ),
    ] {
        let mine = ours(stream, channels, lfe);
        let planes = channels + usize::from(lfe);
        let reference = reference(refb, planes);
        for (idx, plane) in mine.iter().enumerate() {
            let ref_idx = if lfe { FIVE_ONE_MAP[idx] } else { idx };
            let (snr, gain) = snr_gain(plane, &reference[ref_idx]);
            let floor = if lfe && idx == channels { 100.0 } else { 60.0 };
            assert!(
                snr > floor,
                "{name} plane {idx}: ours vs reference {snr:.1} dB"
            );
            assert!((gain - 1.0).abs() < 1e-3, "{name} plane {idx}: gain {gain}");
        }
    }
}

/// Regeneration path: with `DTS_ENCODER_FIXTURE_DIR` set, re-encode
/// the fixture signals with the current encoder into that directory
/// (the reference `.s32` files are then re-decoded out of band with
/// the command in the module docs). Never runs in CI.
#[test]
fn regenerate_fixture_streams_when_requested() {
    let Ok(dir) = std::env::var("DTS_ENCODER_FIXTURE_DIR") else {
        return;
    };
    use oxideav_dts::{CoreEncoder, EncoderConfig};
    let jobs: [(&str, EncoderConfig, Vec<Vec<f64>>); 5] = [
        (
            "enc_stereo_768k",
            EncoderConfig::new(48_000, 2).unwrap(),
            signal("stereo"),
        ),
        (
            "enc_stereo_192k",
            EncoderConfig::new(48_000, 2)
                .unwrap()
                .with_bit_rate(192_000)
                .unwrap(),
            signal("stereo"),
        ),
        (
            "enc_51_lfe_768k",
            EncoderConfig::new(48_000, 5).unwrap().with_lfe(true),
            signal("five_one"),
        ),
        (
            "enc_mono_transient_768k",
            EncoderConfig::new(48_000, 1).unwrap(),
            signal("mono_transient"),
        ),
        (
            "enc_stereo_128k_hfvq",
            EncoderConfig::new(48_000, 2)
                .unwrap()
                .with_bit_rate(128_000)
                .unwrap(),
            signal("stereo_hfvq"),
        ),
    ];
    for (name, config, planes) in jobs {
        let mut enc = CoreEncoder::new(config).unwrap();
        let refs: Vec<&[f64]> = planes.iter().map(Vec::as_slice).collect();
        let mut bytes = Vec::new();
        for f in enc.push(&refs).unwrap() {
            bytes.extend_from_slice(&f);
        }
        for f in enc.flush() {
            bytes.extend_from_slice(&f);
        }
        std::fs::write(format!("{dir}/{name}.dts"), bytes).expect("write fixture");
    }
}
