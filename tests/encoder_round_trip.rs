//! Encoder → this crate's own decoder round trips: every stream the
//! round-453 Core encoder emits must parse with the §5.3 header
//! parser, walk with the frame iterator, and reconstruct the input
//! sample-close through the §C.2.5 decode chain (zero delay by
//! construction — frame `k` decodes to input samples `512k…`).

use oxideav_dts::{
    iter_frames, parse_frame_header, CoreEncoder, CoreStreamDecoder, EncoderConfig,
    ENCODER_FRAME_SAMPLES,
};

/// Full-scale-relative multitone: content across the whole spectrum,
/// peaking near −6 dBFS.
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

/// SNR skipping the first 512 samples: any DTS stream start carries
/// one frame-length of §C.2.5 synthesis-filter priming against zero
/// history (the reference decoder behaves identically), so the
/// steady-state measurement starts at sample 512.
fn snr_db(reference: &[f64], test: &[f64]) -> f64 {
    let n = reference.len().min(test.len());
    let sig: f64 = reference[512..n].iter().map(|v| v * v).sum();
    let err: f64 = reference[512..n]
        .iter()
        .zip(&test[512..n])
        .map(|(a, b)| (a - b) * (a - b))
        .sum();
    10.0 * (sig / err.max(1e-300)).log10()
}

fn encode_stream(config: EncoderConfig, planes: &[Vec<f64>]) -> Vec<u8> {
    let mut enc = CoreEncoder::new(config).expect("config is valid");
    let refs: Vec<&[f64]> = planes.iter().map(Vec::as_slice).collect();
    let mut out = Vec::new();
    for frame in enc.push(&refs).expect("push") {
        out.extend_from_slice(&frame);
    }
    for frame in enc.flush() {
        out.extend_from_slice(&frame);
    }
    out
}

fn decode_stream(bytes: &[u8], channels: usize) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut dec = CoreStreamDecoder::new(channels);
    let mut pcm: Vec<Vec<f64>> = vec![Vec::new(); channels];
    let mut lfe: Vec<f64> = Vec::new();
    for fv in iter_frames(bytes) {
        let fv = fv.expect("every encoded frame iterates");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("every encoded frame decodes");
        for (ch, samples) in block.iter().enumerate() {
            pcm[ch].extend(samples.iter().map(|&s| f64::from(s) / 2f64.powi(31)));
        }
        lfe.extend(
            dec.take_last_lfe_pcm()
                .iter()
                .map(|&s| f64::from(s) / 2f64.powi(31)),
        );
    }
    (pcm, lfe)
}

#[test]
fn stereo_768k_reconstructs_above_55_db() {
    let n = ENCODER_FRAME_SAMPLES * 6;
    let planes = vec![multitone(n, 0.0), multitone(n, 0.7)];
    let config = EncoderConfig::new(48_000, 2).unwrap();
    let bytes = encode_stream(config, &planes);
    assert_eq!(bytes.len() % 1024, 0, "whole 1024-byte frames");
    assert_eq!(bytes.len() / 1024, 6);

    let (pcm, _) = decode_stream(&bytes, 2);
    for ch in 0..2 {
        assert_eq!(pcm[ch].len(), n);
        let snr = snr_db(&planes[ch], &pcm[ch]);
        assert!(snr > 55.0, "ch {ch}: round-trip SNR {snr:.1} dB");
    }
}

#[test]
fn stereo_192k_still_reconstructs_above_25_db() {
    let n = ENCODER_FRAME_SAMPLES * 4;
    let planes = vec![multitone(n, 0.0), multitone(n, 1.3)];
    let config = EncoderConfig::new(48_000, 2)
        .unwrap()
        .with_bit_rate(192_000)
        .unwrap();
    let bytes = encode_stream(config, &planes);
    let (pcm, _) = decode_stream(&bytes, 2);
    for ch in 0..2 {
        let snr = snr_db(&planes[ch], &pcm[ch]);
        assert!(snr > 25.0, "ch {ch}: 192k round-trip SNR {snr:.1} dB");
    }
}

#[test]
fn five_one_with_lfe_reconstructs() {
    let n = ENCODER_FRAME_SAMPLES * 4;
    let mut planes: Vec<Vec<f64>> = (0..5).map(|ch| multitone(n, ch as f64 * 0.4)).collect();
    // LFE plane: a 60 Hz tone, well inside the 64×-decimation band.
    planes.push(
        (0..n)
            .map(|i| 0.25 * (2.0 * std::f64::consts::PI * 60.0 * i as f64 / 48_000.0).sin())
            .collect(),
    );
    let config = EncoderConfig::new(48_000, 5).unwrap().with_lfe(true);
    let bytes = encode_stream(config, &planes);

    let (pcm, lfe) = decode_stream(&bytes, 5);
    for ch in 0..5 {
        let snr = snr_db(&planes[ch], &pcm[ch]);
        assert!(snr > 30.0, "primary {ch}: round-trip SNR {snr:.1} dB");
    }
    assert_eq!(lfe.len(), n);
    let lfe_snr = snr_db(&planes[5], &lfe);
    assert!(lfe_snr > 20.0, "LFE round-trip SNR {lfe_snr:.1} dB");
}

#[test]
fn headers_describe_the_stream() {
    let n = ENCODER_FRAME_SAMPLES * 2;
    let planes = vec![multitone(n, 0.0), multitone(n, 0.5)];
    let config = EncoderConfig::new(48_000, 2).unwrap();
    let bytes = encode_stream(config, &planes);
    let hdr = parse_frame_header(&bytes).expect("encoded header parses");
    assert_eq!(hdr.sample_rate_hz(), Some(48_000));
    assert_eq!(hdr.channel_count(), Some(2));
    assert_eq!(hdr.bit_rate_bps(), Some(768_000));
    assert_eq!(hdr.frame_size_bytes, 1024);
    assert_eq!(hdr.blocks_per_frame, 15);
    assert_eq!(usize::from(hdr.sample_count_per_block), 32);
    assert!(!hdr.crc_present);
    assert!(hdr.multirate_inter, "FILTS = Perfect Reconstruction");
    assert_eq!(hdr.source_pcm_bits_per_sample(), Some(16));
    // Both frames found by the sync iterator.
    assert_eq!(iter_frames(&bytes).count(), 2);
}

#[test]
fn flush_covers_partial_tail_with_zero_padding() {
    let n = ENCODER_FRAME_SAMPLES + 100;
    let planes = vec![multitone(n, 0.0)];
    let config = EncoderConfig::new(48_000, 1).unwrap();
    let bytes = encode_stream(config, &planes);
    // Two frames: one full, one padded.
    assert_eq!(iter_frames(&bytes).count(), 2);
    let (pcm, _) = decode_stream(&bytes, 1);
    assert_eq!(pcm[0].len(), 2 * ENCODER_FRAME_SAMPLES);
    let snr = snr_db(&planes[0][..n], &pcm[0][..n]);
    assert!(snr > 50.0, "padded-tail round trip SNR {snr:.1} dB");
}

#[test]
fn silence_encodes_and_decodes_to_silence() {
    let n = ENCODER_FRAME_SAMPLES * 2;
    let planes = vec![vec![0.0_f64; n], vec![0.0_f64; n]];
    let config = EncoderConfig::new(48_000, 2).unwrap();
    let bytes = encode_stream(config, &planes);
    let (pcm, _) = decode_stream(&bytes, 2);
    for (ch, plane) in pcm.iter().enumerate() {
        assert!(plane.iter().all(|&v| v == 0.0), "ch {ch} must stay silent");
    }
}

/// Per-frame structure the encoder chose, read back through the
/// crate's own §5.3.2 / §5.4.1 parsers.
fn frame_structure(frame: &[u8]) -> (Vec<(usize, usize)>, Vec<Vec<u8>>) {
    let hdr = parse_frame_header(frame).unwrap();
    let hb = hdr.header_bit_length() as usize;
    let (coding, bits) =
        oxideav_dts::decode_audio_coding_header_at(frame, hb, hdr.crc_present).unwrap();
    let (side, _) =
        oxideav_dts::decode_primary_side_info_at(frame, hb + bits, &coding.channel_params).unwrap();
    let subs = coding
        .channel_params
        .iter()
        .map(|p| (p.n_subs, p.n_vqsub))
        .collect();
    let tmodes = side.channels.iter().map(|c| c.tmode.to_vec()).collect();
    (subs, tmodes)
}

#[test]
fn transients_flag_tmode_and_still_reconstruct() {
    // Quiet tone, then a burst starting exactly at the second
    // subsubframe of frame 2 (sample 1024 + 256).
    let n = ENCODER_FRAME_SAMPLES * 4;
    let plane: Vec<f64> = (0..n)
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
        .collect();
    let config = EncoderConfig::new(48_000, 1).unwrap();
    let bytes = encode_stream(config, std::slice::from_ref(&plane));
    let frames: Vec<&[u8]> = iter_frames(&bytes).map(|f| f.unwrap().data).collect();
    let (_, tmodes) = frame_structure(frames[2]);
    assert!(
        tmodes[0].iter().any(|&t| t > 0),
        "the burst frame must carry TMODE transients: {:?}",
        tmodes[0]
    );
    let (_, quiet_tmodes) = frame_structure(frames[0]);
    assert!(
        quiet_tmodes[0].iter().all(|&t| t == 0),
        "steady frame has no transient"
    );
    let (pcm, _) = decode_stream(&bytes, 1);
    let snr = snr_db(&plane, &pcm[0]);
    assert!(snr > 40.0, "transient stream round trip {snr:.1} dB");
    // The quiet pre-burst quarter of frame 2 stays quiet: no pre-echo
    // above −40 dB relative to the burst.
    let pre: f64 = pcm[0][1024..1280].iter().map(|v| v * v).sum::<f64>() / 256.0;
    assert!(pre.sqrt() < 0.6 * 0.01, "pre-burst rms {}", pre.sqrt());
}

#[test]
fn low_rate_uses_high_frequency_vq_above_nvqsub() {
    // A −40 dBFS tone at 20 kHz (band 26) on top of the multitone: at
    // 128 kbit/s the allocator cannot afford to quantize it, so the
    // band goes out as a §D.10.2 vector above nVQSUB.
    let n = ENCODER_FRAME_SAMPLES * 4;
    let planes: Vec<Vec<f64>> = (0..2)
        .map(|ch| {
            multitone(n, ch as f64 * 0.6)
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    v + 0.01 * (2.0 * std::f64::consts::PI * 20_000.0 * i as f64 / 48_000.0).sin()
                })
                .collect()
        })
        .collect();
    let config = EncoderConfig::new(48_000, 2)
        .unwrap()
        .with_bit_rate(128_000)
        .unwrap();
    let bytes = encode_stream(config, &planes);
    let mut vq_bands = 0usize;
    for fv in iter_frames(&bytes) {
        let (subs, _) = frame_structure(fv.unwrap().data);
        for (n_subs, n_vqsub) in subs {
            assert!(n_vqsub <= n_subs && n_subs >= 2);
            vq_bands += n_subs - n_vqsub;
        }
    }
    assert!(vq_bands > 0, "no high-frequency VQ band was used");
    let (pcm, _) = decode_stream(&bytes, 2);
    for ch in 0..2 {
        let snr = snr_db(&planes[ch], &pcm[ch]);
        assert!(snr > 15.0, "ch {ch}: 128k+VQ round trip {snr:.1} dB");
    }
}

/// `PMODE` usage per frame: predicted bands per channel.
fn predicted_bands(frame: &[u8]) -> Vec<usize> {
    let hdr = parse_frame_header(frame).unwrap();
    let hb = hdr.header_bit_length() as usize;
    let (coding, bits) =
        oxideav_dts::decode_audio_coding_header_at(frame, hb, hdr.crc_present).unwrap();
    let (side, _) =
        oxideav_dts::decode_primary_side_info_at(frame, hb + bits, &coding.channel_params).unwrap();
    side.channels
        .iter()
        .map(|c| c.pmode.iter().filter(|&&p| p > 0).count())
        .collect()
}

#[test]
fn adpcm_predicts_tonal_bands_and_improves_the_round_trip() {
    // Stationary tones are highly predictable in the subband domain.
    let n = ENCODER_FRAME_SAMPLES * 6;
    let planes = [multitone(n, 0.0), multitone(n, 0.7)];
    let base = EncoderConfig::new(48_000, 2)
        .unwrap()
        .with_bit_rate(192_000)
        .unwrap();
    let with = encode_stream(base.clone(), &planes);
    let without = encode_stream(base.with_adpcm(false), &planes);

    let predicted: usize = iter_frames(&with)
        .skip(1)
        .map(|f| predicted_bands(f.unwrap().data).iter().sum::<usize>())
        .sum();
    assert!(predicted > 0, "ADPCM must be used on the tonal stream");
    assert!(iter_frames(&without).all(|f| predicted_bands(f.unwrap().data).iter().all(|&c| c == 0)));

    let (pcm_with, _) = decode_stream(&with, 2);
    let (pcm_without, _) = decode_stream(&without, 2);
    for ch in 0..2 {
        let a = snr_db(&planes[ch], &pcm_with[ch]);
        let b = snr_db(&planes[ch], &pcm_without[ch]);
        assert!(
            a > b + 3.0,
            "ch {ch}: ADPCM {a:.1} dB must beat no-ADPCM {b:.1} dB by 3 dB"
        );
    }
}
