//! The framework half of the encoder's dual API (`make_encoder` /
//! `register`) and the on-wire word-format variants: every variant
//! must decode through the registry `Decoder` to PCM identical to the
//! raw-BE stream's.

use oxideav_core::{
    AudioFrame, ChannelLayout, CodecId, CodecParameters, Error as CoreError, Frame, Packet,
    RuntimeContext, SampleFormat, TimeBase,
};
use oxideav_dts::{
    iter_frames_14bit, make_decoder, make_encoder, register, CoreEncoder, EncoderConfig,
    SyncWordEncoding, CODEC_ID_STR, ENCODER_FRAME_SAMPLES,
};

fn multitone(n: usize, phase: f64) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            0.20 * (2.0 * std::f64::consts::PI * (440.0 * t + phase)).sin()
                + 0.12 * (2.0 * std::f64::consts::PI * (1_330.0 * t + phase)).sin()
                + 0.08 * (2.0 * std::f64::consts::PI * (3_700.0 * t + 0.3 * phase)).cos()
                + 0.05 * (2.0 * std::f64::consts::PI * (9_200.0 * t)).sin()
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

fn s32p_frame(planes: &[Vec<f64>], pts: i64) -> Frame {
    let data = planes
        .iter()
        .map(|p| {
            p.iter()
                .flat_map(|&v| ((v * 2_147_483_647.0) as i32).to_le_bytes())
                .collect()
        })
        .collect();
    Frame::Audio(AudioFrame {
        samples: planes[0].len() as u32,
        pts: Some(pts),
        data,
    })
}

fn decode_packets(packets: &[Packet], planes: usize) -> Vec<Vec<f64>> {
    let params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    let mut dec = make_decoder(&params).unwrap();
    let mut out = vec![Vec::new(); planes];
    for p in packets {
        dec.send_packet(p).unwrap();
        let Frame::Audio(a) = dec.receive_frame().unwrap() else {
            panic!("audio")
        };
        assert_eq!(a.data.len(), planes);
        for (ch, plane) in a.data.iter().enumerate() {
            out[ch].extend(
                plane.chunks_exact(4).map(|c| {
                    f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])) / 2f64.powi(31)
                }),
            );
        }
    }
    out
}

#[test]
fn registry_installs_the_encoder_factory() {
    let mut ctx = RuntimeContext::default();
    register(&mut ctx);
    assert!(ctx.codecs.has_encoder(&CodecId::new(CODEC_ID_STR)));
    assert!(ctx.codecs.has_decoder(&CodecId::new(CODEC_ID_STR)));
}

#[test]
fn stereo_s32p_round_trips_through_both_registry_halves() {
    let mut params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    params.sample_rate = Some(48_000);
    params.channel_layout = Some(ChannelLayout::Stereo);
    params.sample_format = Some(SampleFormat::S32P);
    params.bit_rate = Some(768_000);
    let mut enc = make_encoder(&params).unwrap();
    assert_eq!(enc.output_params().bit_rate, Some(768_000));
    assert_eq!(enc.output_params().channels, Some(2));

    let n = ENCODER_FRAME_SAMPLES * 4;
    let planes = [multitone(n, 0.0), multitone(n, 0.9)];
    // Feed in two uneven chunks to exercise the internal buffering.
    let split = 700;
    let a: Vec<Vec<f64>> = planes.iter().map(|p| p[..split].to_vec()).collect();
    let b: Vec<Vec<f64>> = planes.iter().map(|p| p[split..].to_vec()).collect();
    enc.send_frame(&s32p_frame(&a, 0)).unwrap();
    // 700 samples < one frame + lookahead: nothing to pull yet.
    assert!(matches!(enc.receive_packet(), Err(CoreError::NeedMore)));
    enc.send_frame(&s32p_frame(&b, split as i64)).unwrap();
    let mut packets = Vec::new();
    // The first frame is encodable once its 1280-sample lookahead is
    // covered; the rest come out of the flush.
    packets.push(enc.receive_packet().expect("first frame ready"));
    enc.flush().unwrap();
    loop {
        match enc.receive_packet() {
            Ok(p) => packets.push(p),
            Err(CoreError::Eof) => break,
            Err(e) => panic!("{e}"),
        }
    }
    // 4 frames of 512 samples, stamped in samples.
    assert_eq!(packets.len(), 4);
    for (k, p) in packets.iter().enumerate() {
        assert_eq!(p.pts, Some(512 * k as i64));
        assert_eq!(p.duration, Some(512));
        assert!(p.flags.keyframe);
        assert_eq!(p.time_base, TimeBase::new(1, 48_000));
        assert_eq!(p.data.len(), 1024);
    }
    let pcm = decode_packets(&packets, 2);
    for ch in 0..2 {
        let snr = snr_db(&planes[ch], &pcm[ch]);
        assert!(snr > 60.0, "ch {ch}: registry round trip {snr:.1} dB");
    }
}

#[test]
fn five_one_layout_maps_to_table_5_4_and_lfe() {
    // Framework 5.1 = FL FR FC LFE SL SR; DTS = C L R SL SR + LFE.
    let mut params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    params.sample_rate = Some(48_000);
    params.channel_layout = Some(ChannelLayout::Surround51);
    params.sample_format = Some(SampleFormat::F32);
    let mut enc = make_encoder(&params).unwrap();
    let n = ENCODER_FRAME_SAMPLES * 3;
    // Distinct content per framework channel.
    let fw: Vec<Vec<f64>> = (0..6)
        .map(|ch| {
            if ch == 3 {
                (0..n)
                    .map(|i| 0.2 * (2.0 * std::f64::consts::PI * 50.0 * i as f64 / 48_000.0).sin())
                    .collect()
            } else {
                multitone(n, ch as f64 * 0.5)
            }
        })
        .collect();
    // Interleaved F32.
    let mut data = Vec::with_capacity(n * 6 * 4);
    for i in 0..n {
        for plane in &fw {
            data.extend_from_slice(&(plane[i] as f32).to_le_bytes());
        }
    }
    enc.send_frame(&Frame::Audio(AudioFrame {
        samples: n as u32,
        pts: Some(0),
        data: vec![data],
    }))
    .unwrap();
    enc.flush().unwrap();
    let mut packets = Vec::new();
    while let Ok(p) = enc.receive_packet() {
        packets.push(p);
    }
    let hdr = oxideav_dts::parse_frame_header(&packets[0].data).unwrap();
    assert_eq!(hdr.amode, 9);
    assert!(hdr.lfe.is_present());
    // Decoder planes: C L R SL SR LFE ↔ framework FC FL FR SL SR LFE.
    let pcm = decode_packets(&packets, 6);
    let map = [2usize, 0, 1, 4, 5, 3];
    for (dts, &fwc) in map.iter().enumerate() {
        let snr = snr_db(&fw[fwc], &pcm[dts]);
        let floor = if dts == 5 { 20.0 } else { 35.0 };
        assert!(
            snr > floor,
            "DTS plane {dts} <- framework {fwc}: {snr:.1} dB"
        );
    }
}

#[test]
fn unsupported_layouts_and_formats_are_declined() {
    let mut params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    params.sample_rate = Some(48_000);
    params.channel_layout = Some(ChannelLayout::Surround71);
    assert!(make_encoder(&params).is_err());
    params.channel_layout = Some(ChannelLayout::Stereo);
    params.sample_format = Some(SampleFormat::U8);
    assert!(make_encoder(&params).is_err());
    params.sample_format = None;
    params.sample_rate = Some(96_000);
    assert!(make_encoder(&params).is_err());
}

/// Every wire variant decodes (through the registry decoder, which
/// routes on the sync word) to PCM bit-identical to the raw-BE stream.
#[test]
fn wire_variants_decode_identically() {
    let n = ENCODER_FRAME_SAMPLES * 3;
    let planes = [multitone(n, 0.0), multitone(n, 0.4)];
    let refs: Vec<&[f64]> = planes.iter().map(Vec::as_slice).collect();
    let encode = |encoding: SyncWordEncoding| -> Vec<Vec<u8>> {
        let config = EncoderConfig::new(48_000, 2)
            .unwrap()
            .with_bit_rate(768_000)
            .unwrap()
            .with_sync_word_encoding(encoding);
        let mut enc = CoreEncoder::new(config).unwrap();
        let mut frames = enc.push(&refs).unwrap();
        frames.extend(enc.flush());
        frames
    };
    let decode = |frames: &[Vec<u8>]| -> Vec<Vec<f64>> {
        let packets: Vec<Packet> = frames
            .iter()
            .map(|f| Packet::new(0, TimeBase::new(1, 48_000), f.clone()))
            .collect();
        decode_packets(&packets, 2)
    };
    let be = encode(SyncWordEncoding::RawBigEndian);
    let reference = decode(&be);
    assert!(snr_db(&planes[0], &reference[0]) > 60.0);

    let le = encode(SyncWordEncoding::RawLittleEndian);
    assert_eq!(le[0].len(), 1024);
    assert_eq!(&le[0][..4], &[0xFE, 0x7F, 0x01, 0x80]);
    assert_eq!(decode(&le), reference, "raw-LE must decode identically");

    for (encoding, sync) in [
        (
            SyncWordEncoding::FourteenBitBigEndian,
            [0x1F, 0xFF, 0xE8, 0x00],
        ),
        (
            SyncWordEncoding::FourteenBitLittleEndian,
            [0xFF, 0x1F, 0x00, 0xE8],
        ),
    ] {
        let frames = encode(encoding);
        // 14-bit frames are a multiple of 14 unpacked bytes (1022 here),
        // packed into 1022·8/14 = 584 containers = 1168 bytes.
        assert_eq!(frames[0].len(), 1168, "{encoding:?}");
        assert_eq!(&frames[0][..4], &sync, "{encoding:?}");
        let got = decode(&frames);
        // Same audio, but a 1022-byte budget instead of 1024: not
        // bit-identical to the BE stream, still ≥ 60 dB.
        assert!(snr_db(&planes[0], &got[0]) > 60.0, "{encoding:?}");
        // The concatenated 14-bit stream stays sync-aligned for the
        // 14-bit frame iterator.
        let stream: Vec<u8> = frames.concat();
        let views: Vec<_> = iter_frames_14bit(&stream)
            .map(|v| v.expect("14-bit frame iterates"))
            .collect();
        assert_eq!(views.len(), 3, "{encoding:?}");
        assert!(views.iter().all(|v| v.header.frame_size_bytes == 1022));
    }
}
