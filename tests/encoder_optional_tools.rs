//! The encoder's optional tools: embedded dynamic range (`DYNF` /
//! `RANGE`), joint intensity coding (`JOINX` / `JOIN_SCALES`) and the
//! §5.7.1 auxiliary chunk with dynamic downmix coefficients and its
//! `nAUXCRC16`, each verified through this crate's own parsers and
//! decoder.

use oxideav_dts::{
    decode_core_frame_with_info, iter_frames, parse_aux_data, parse_frame_header, CoreEncoder,
    CoreStreamDecoder, DownmixSpec, DownmixType, EncoderConfig, ENCODER_FRAME_SAMPLES,
};

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

fn decode_stream(bytes: &[u8], channels: usize) -> Vec<Vec<f64>> {
    let mut dec = CoreStreamDecoder::new(channels);
    let mut pcm: Vec<Vec<f64>> = vec![Vec::new(); channels];
    for fv in iter_frames(bytes) {
        let fv = fv.expect("frames iterate");
        let block = dec
            .decode_frame(fv.data, &fv.header)
            .expect("frames decode");
        for (ch, samples) in block.iter().enumerate() {
            pcm[ch].extend(samples.iter().map(|&s| f64::from(s) / 2f64.powi(31)));
        }
    }
    pcm
}

#[test]
fn dynamic_range_coefficient_scales_the_decoded_output() {
    let n = ENCODER_FRAME_SAMPLES * 4;
    let planes = vec![multitone(n, 0.0), multitone(n, 0.5)];
    let config = EncoderConfig::new(48_000, 2)
        .unwrap()
        .with_dynamic_range_db(Some(-6.0));
    let bytes = encode_stream(config, &planes);
    let hdr = parse_frame_header(&bytes).unwrap();
    assert!(hdr.dynamic_range, "DYNF set");
    // §D.4: −6 dB is code −24 (Q2), applied as 10^(−6/20) after the
    // §C.2.5 synthesis.
    let gain = 10f64.powf(-6.0 / 20.0);
    let pcm = decode_stream(&bytes, 2);
    for ch in 0..2 {
        let scaled: Vec<f64> = planes[ch].iter().map(|v| v * gain).collect();
        let snr = snr_db(&scaled, &pcm[ch]);
        assert!(snr > 55.0, "ch {ch}: DRC-scaled round trip {snr:.1} dB");
        let unscaled = snr_db(&planes[ch], &pcm[ch]);
        assert!(
            unscaled < 10.0,
            "the gain must actually be applied ({unscaled:.1} dB)"
        );
    }
}

#[test]
fn joint_intensity_copies_the_source_high_bands() {
    let n = ENCODER_FRAME_SAMPLES * 4;
    // R = L shifted in phase: identical band energies, so the unity-
    // bounded linear JOIN_SCALES are exact.
    let planes = vec![multitone(n, 0.0), multitone(n, 0.25)];
    let config = EncoderConfig::new(48_000, 2)
        .unwrap()
        .with_bit_rate(192_000)
        .unwrap()
        .with_joint_intensity_start(Some(8));
    let bytes = encode_stream(config, &planes);
    for fv in iter_frames(&bytes) {
        let fv = fv.unwrap();
        let hb = fv.header.header_bit_length() as usize;
        let (coding, _) = oxideav_dts::decode_audio_coding_header_at(fv.data, hb, false).unwrap();
        assert_eq!(coding.joinx, vec![0, 1], "R is joint-coded from L");
        assert!(
            coding.channel_params[1].n_subs <= 8,
            "R carries nothing above band 8"
        );
        assert!(
            coding.channel_params[0].n_subs > 8,
            "L keeps its high bands"
        );
    }
    let pcm = decode_stream(&bytes, 2);
    // L is coded normally; R below band 8 (< 6 kHz) is its own, above
    // it is L's — so R matches the input on the 440 Hz / 1.33 kHz /
    // 3.7 kHz tones and carries L's 9.2 / 17.5 kHz tones instead.
    assert!(snr_db(&planes[0], &pcm[0]) > 30.0);
    let low_only: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            0.20 * (2.0 * std::f64::consts::PI * (440.0 * t + 0.25)).sin()
                + 0.12 * (2.0 * std::f64::consts::PI * (1_330.0 * t + 0.25)).sin()
                + 0.08 * (2.0 * std::f64::consts::PI * (3_700.0 * t + 0.3 * 0.25)).cos()
        })
        .collect();
    let high_from_l: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            0.05 * (2.0 * std::f64::consts::PI * (9_200.0 * t)).sin()
                + 0.02 * (2.0 * std::f64::consts::PI * (17_500.0 * t)).sin()
        })
        .collect();
    let expect_r: Vec<f64> = low_only
        .iter()
        .zip(&high_from_l)
        .map(|(a, b)| a + b)
        .collect();
    let snr = snr_db(&expect_r, &pcm[1]);
    assert!(snr > 25.0, "joint R vs (own lows + L's highs): {snr:.1} dB");
}

#[test]
fn aux_chunk_carries_crc_checked_downmix_coefficients() {
    let n = ENCODER_FRAME_SAMPLES * 2;
    let mut planes: Vec<Vec<f64>> = (0..5).map(|ch| multitone(n, ch as f64 * 0.3)).collect();
    planes.push(vec![0.0; n]);
    // 5.1 -> Lo/Ro: C at −3 dB to both, L/R straight, surrounds at
    // −6 dB, LFE dropped.
    let c = 10f64.powf(-3.0 / 20.0);
    let s = 0.5;
    let coefficients = vec![
        c, 1.0, 0.0, s, 0.0, 0.0, // Lo <- C L R SL SR LFE
        c, 0.0, 1.0, 0.0, s, 0.0, // Ro
    ];
    let config = EncoderConfig::new(48_000, 5)
        .unwrap()
        .with_lfe(true)
        .with_downmix(Some(DownmixSpec {
            downmix_type: DownmixType::LoRo,
            coefficients: coefficients.clone(),
        }));
    let bytes = encode_stream(config, &planes);
    for fv in iter_frames(&bytes) {
        let fv = fv.unwrap();
        assert!(fv.header.aux_data, "AUXF set");
        let aux = parse_aux_data(fv.data, &fv.header)
            .expect("aux parses")
            .expect("aux chunk found");
        assert!(aux.crc_valid, "nAUXCRC16 verifies");
        assert_eq!(aux.time_stamp, None);
        let dmix = aux.dynamic_downmix.expect("downmix present");
        assert_eq!(dmix.downmix_type, DownmixType::LoRo);
        assert_eq!(dmix.input_channel_count, 6);
        let matrix = dmix.coefficient_matrix().unwrap();
        for out in 0..2 {
            for inp in 0..6 {
                let want = coefficients[out * 6 + inp];
                let got = matrix[out][inp];
                assert!(
                    (got - want).abs() < 0.02,
                    "coefficient [{out}][{inp}] {got} vs {want}"
                );
            }
        }
        // The §5.6 walker sees the AUXCT-counted bytes too.
        let (_, info) = decode_core_frame_with_info(fv.data, &fv.header).unwrap();
        assert!(!info.aux_bytes.is_empty());
    }
    // Shape validation.
    assert!(
        CoreEncoder::new(
            EncoderConfig::new(48_000, 2)
                .unwrap()
                .with_downmix(Some(DownmixSpec {
                    downmix_type: DownmixType::LoRo,
                    coefficients: vec![1.0; 3],
                }))
        )
        .is_err()
    );
}
