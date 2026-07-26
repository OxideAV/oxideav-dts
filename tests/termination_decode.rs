//! End-to-end decode battery for **termination frames** (§5.3.1
//! `FTYPE = 0`) with a **partial subsubframe** (§5.4.1 `PSC > 0`) —
//! ETSI TS 102 114 V1.3.1, staged PDF p.18 (FTYPE / SHORT / NBLKS)
//! and p.30 (PSC).
//!
//! Every stream here is spec-built by the deterministic builder in
//! `tests/common/mod.rs`: a termination frame carries `NBLKS + 1 =
//! Σ (nSSC·8 - (8 - PSC))` subband-sample blocks, its last
//! subsubframe holds `PSC < 8` samples per active subband, and the
//! DSYNC trailer follows the partial subsubframe. The batteries
//! sweep the PSC range, the SSC × PSC grid, the joint-intensity /
//! DYNF / CPF / ASPF / LFE / multi-subframe interactions, and the
//! normal-frame gating ("It exists only in a termination frame").

mod common;

use common::{JointFrameSpec, TERM_PSC, TERM_SAMPLES};
use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, TimeBase};
use oxideav_dts::{
    decode_core_frame, decode_core_frame_with_info, iter_frames, make_decoder, parse_frame_header,
    CoreFrameDecodeError, CoreStreamDecoder, FrameType, SubframePcmError, CODEC_ID_STR,
};

/// Decode one built frame with single-frame semantics, expecting
/// success; returns the planar PCM.
fn decode_one(frame: &[u8]) -> Vec<Vec<i32>> {
    let header = parse_frame_header(frame).expect("built frame header parses");
    decode_core_frame(frame, &header).expect("built frame decodes to PCM")
}

/// The default termination frame (`nSSC = 2`, `PSC = 5`) decodes to
/// exactly `(2·8 - 3) · 32 = 416` samples per channel — the valid
/// prefix, not a rounded-up 512.
#[test]
fn termination_frame_decodes_valid_prefix_length() {
    let spec = JointFrameSpec::default_termination(0xA5A5_0001);
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let pcm = decode_one(&frame);
    assert_eq!(pcm.len(), 2);
    for (ch, plane) in pcm.iter().enumerate() {
        assert_eq!(
            plane.len(),
            TERM_SAMPLES,
            "channel {ch} valid-prefix length"
        );
        let peak = plane.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 1000, "channel {ch} silent (peak {peak})");
    }
}

/// PSC boundary sweep: every legal `(nSSC, PSC)` pair whose block
/// count clears the NBLKS floor (`NBLKS raw >= 5`, i.e. >= 6 blocks)
/// decodes to `(nSSC·8 - (8 - PSC)) · 32` samples per channel.
#[test]
fn ssc_by_psc_grid_decodes_exact_lengths() {
    let template = common::template_header();
    for n_ssc in 1..=4usize {
        for psc in 1..=7u8 {
            let blocks = n_ssc * 8 - (8 - psc as usize);
            if blocks < 6 {
                continue; // below the NBLKS validity floor (PDF p.18)
            }
            let spec = JointFrameSpec {
                n_ssc,
                psc,
                ..JointFrameSpec::default_termination(0xB0B0 ^ ((n_ssc as u32) << 8) ^ psc as u32)
            };
            let frame = common::build_frame_from_spec(&template, &spec);
            let pcm = decode_one(&frame);
            for plane in &pcm {
                assert_eq!(
                    plane.len(),
                    blocks * 32,
                    "nSSC={n_ssc} PSC={psc}: expected {blocks} blocks"
                );
            }
        }
    }
}

/// The minimum legal termination frame — `NBLKS` raw 5 (6 blocks),
/// expressible only because the partial subsubframe is *counted by*
/// `nSSC` (`nSSC = 1`, `PSC = 6`) — decodes to 192 samples.
#[test]
fn minimum_legal_termination_frame_six_blocks() {
    let spec = JointFrameSpec {
        n_ssc: 1,
        psc: 6,
        ..JointFrameSpec::default_termination(0xC0DE_0006)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let header = parse_frame_header(&frame).unwrap();
    assert_eq!(
        header.blocks_per_frame, 5,
        "NBLKS raw at its validity floor"
    );
    let pcm = decode_one(&frame);
    for plane in &pcm {
        assert_eq!(plane.len(), 6 * 32);
    }
}

/// Multi-subframe termination: the partial subsubframe sits in the
/// **last** subframe; the earlier subframe is whole. Two subframes of
/// `nSSC = 2` with `PSC = 5` give `16 + 13 = 29` blocks, and the
/// second subframe's side info parses only if the first subframe's
/// bit budget was exact.
#[test]
fn multi_subframe_termination_decodes() {
    let spec = JointFrameSpec {
        n_subframes: 2,
        ..JointFrameSpec::default_termination(0xD00D_0002)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let header = parse_frame_header(&frame).unwrap();
    assert_eq!(header.blocks_per_frame, 28);
    let pcm = decode_one(&frame);
    for plane in &pcm {
        assert_eq!(plane.len(), 29 * 32);
    }
}

/// JOINX × termination: a jointly-coded channel 1 (importing
/// sub-bands 16..32 from channel 0 through the §D.3 scales) on a
/// partial subframe reconstructs the same valid-prefix length, with
/// both channels live and distinct.
#[test]
fn joint_intensity_on_termination_frame() {
    let spec = JointFrameSpec {
        n_subs: [32, 16],
        joinx: [0, 1],
        join_symbols: common::JOINT_SCALE_RAW.iter().map(|&r| r as i32).collect(),
        ..JointFrameSpec::default_termination(0xE0E0_0001)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let pcm = decode_one(&frame);
    for (ch, plane) in pcm.iter().enumerate() {
        assert_eq!(plane.len(), TERM_SAMPLES);
        let peak = plane.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 1000, "channel {ch} silent (peak {peak})");
    }
    assert_ne!(pcm[0], pcm[1]);
}

/// DYNF (legacy RANGE gain) on a termination frame: the tail field is
/// consumed and the gain applied over the valid prefix. A −6 dB code
/// halves the PCM relative to the ungained build of the same seed.
#[test]
fn dynf_range_applies_over_termination_prefix() {
    let base = JointFrameSpec::default_termination(0xF00F_0001);
    let gained = JointFrameSpec {
        dynf_code: Some((-24i8) as u8), // signed Q2: -24/4 = -6 dB
        ..JointFrameSpec::default_termination(0xF00F_0001)
    };
    let template = common::template_header();
    let plain = decode_one(&common::build_frame_from_spec(&template, &base));
    let scaled = decode_one(&common::build_frame_from_spec(&template, &gained));
    let gain = oxideav_dts::dts_dynrng_to_linear((-24i8) as u8);
    for ch in 0..2 {
        assert_eq!(scaled[ch].len(), TERM_SAMPLES);
        for (a, b) in plain[ch].iter().zip(&scaled[ch]) {
            let want = (f64::from(*a) * gain).round();
            assert!(
                (f64::from(*b) - want).abs() <= 1.0,
                "channel {ch}: {b} vs expected {want}"
            );
        }
    }
}

/// CPF on a termination frame: the header/audio-header/side-info CRC
/// words are consumed for framing without disturbing the partial
/// subsubframe's bit budget.
#[test]
fn cpf_crc_words_coexist_with_partial_subsubframe() {
    let spec = JointFrameSpec {
        cpf: true,
        ..JointFrameSpec::default_termination(0x0C4C_0001)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let pcm = decode_one(&frame);
    for plane in &pcm {
        assert_eq!(plane.len(), TERM_SAMPLES);
    }
}

/// ASPF × termination: a DSYNC follows every subsubframe including
/// the partial one.
#[test]
fn aspf_on_termination_frame() {
    let spec = JointFrameSpec {
        aspf: true,
        ..JointFrameSpec::default_termination(0xA5FF_0001)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let pcm = decode_one(&frame);
    for plane in &pcm {
        assert_eq!(plane.len(), TERM_SAMPLES);
    }
}

/// LFE × termination: the LFE phase is extracted at its spec-literal
/// whole-subsubframe size (`2·LFF·nSSC`, no PSC term) and the
/// interpolated plane is truncated to the primaries' valid-prefix
/// length so every output plane stays aligned.
#[test]
fn lfe_plane_truncated_to_termination_prefix() {
    let spec = JointFrameSpec {
        lfe: true,
        ..JointFrameSpec::default_termination(0x1FE0_0001)
    };
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let header = parse_frame_header(&frame).unwrap();
    let mut dec = CoreStreamDecoder::new(2);
    let pcm = dec.decode_frame(&frame, &header).expect("decodes");
    let lfe = dec.take_last_lfe_pcm();
    for plane in &pcm {
        assert_eq!(plane.len(), TERM_SAMPLES);
    }
    assert_eq!(
        lfe.len(),
        TERM_SAMPLES,
        "LFE plane aligned to the valid prefix"
    );
}

/// `PSC > 0` on a **normal** frame is structurally invalid ("It
/// exists only in a termination frame", PDF p.30) and surfaces the
/// typed decline. The frame is built as a termination frame and its
/// FTYPE bit is then flipped to normal (the builder itself refuses
/// the combination).
#[test]
fn psc_on_normal_frame_is_typed_error() {
    let spec = JointFrameSpec::default_termination(0xBAD0_0001);
    let mut frame = common::build_frame_from_spec(&common::template_header(), &spec);
    // FTYPE is the first bit after the 32-bit sync word: MSB of byte 4.
    frame[4] |= 0x80;
    let header = parse_frame_header(&frame).unwrap();
    assert_eq!(header.frame_type, FrameType::Normal);
    let err = decode_core_frame(&frame, &header).unwrap_err();
    assert!(
        matches!(
            err,
            CoreFrameDecodeError::Decode(SubframePcmError::PartialSubsubframeInNormalFrame {
                subframe: 0,
                psc: TERM_PSC,
            })
        ),
        "got {err:?}"
    );
}

/// The multi-frame stream shape the spec describes (normal frames,
/// then a termination frame aligning the sequence end): every frame
/// decodes through one persistent [`CoreStreamDecoder`], and the
/// stream total is `2·512 + 416`.
#[test]
fn stream_of_normal_frames_ends_with_termination() {
    let stream = common::build_termination_stream(3);
    let mut dec = CoreStreamDecoder::new(2);
    let mut lens = Vec::new();
    let mut total = [0usize; 2];
    for fv in iter_frames(&stream) {
        let fv = fv.expect("stream frames iterate cleanly");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("frame decodes");
        lens.push(pcm[0].len());
        for ch in 0..2 {
            total[ch] += pcm[ch].len();
        }
    }
    assert_eq!(lens, vec![512, 512, TERM_SAMPLES]);
    assert_eq!(total, [1024 + TERM_SAMPLES; 2]);
}

/// Dense corruption over the termination frame itself: every single
/// byte of the frame XORed with two masks (header, SSC/PSC prefix,
/// side-info planes, partial-subsubframe payload, DSYNC, zero-pad
/// tail) must yield a typed error or a clean decode — never a panic.
/// This is the per-byte-exhaustive complement of the strided
/// whole-stream sweep in `tests/corruption_robustness.rs`.
#[test]
fn termination_frame_dense_corruption_never_panics() {
    let spec = JointFrameSpec::default_termination(0xDEAD_0001);
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    for offset in 0..frame.len() {
        for mask in [0x80u8, 0xFF] {
            let mut damaged = frame.clone();
            damaged[offset] ^= mask;
            let Ok(header) = parse_frame_header(&damaged) else {
                continue; // typed header error is fine
            };
            // Typed decode errors are fine; panics are the failure.
            let _ = decode_core_frame(&damaged, &header);
        }
    }
}

/// The dual-API contract covers termination frames too: the registry
/// surface (`make_decoder` → `send_packet` / `receive_frame`) emits a
/// 416-sample final frame, bit-identical to the direct
/// [`CoreStreamDecoder`] path across the whole stream.
#[test]
fn registry_path_matches_direct_on_termination_stream() {
    let stream = common::build_termination_stream(3);

    let params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    let mut reg: Box<dyn Decoder> = make_decoder(&params).expect("factory builds");
    let mut via_registry: Vec<Vec<i32>> = vec![Vec::new(); 2];
    let mut frame_samples = Vec::new();
    for fv in iter_frames(&stream) {
        let fv = fv.expect("stream frames iterate cleanly");
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), fv.data.to_vec());
        reg.send_packet(&pkt)
            .expect("send_packet accepts the frame");
        let Frame::Audio(audio) = reg.receive_frame().expect("frame decodes") else {
            panic!("expected an audio frame");
        };
        frame_samples.push(audio.samples);
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                via_registry[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }
    assert_eq!(frame_samples, vec![512, 512, TERM_SAMPLES as u32]);

    let mut dec = CoreStreamDecoder::new(2);
    let mut direct: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(&stream) {
        let fv = fv.expect("frames iterate");
        let pcm = dec.decode_frame(fv.data, &fv.header).expect("decodes");
        for ch in 0..2 {
            direct[ch].extend(&pcm[ch]);
        }
    }
    assert_eq!(
        via_registry, direct,
        "registry and direct paths must be bit-identical on a termination stream"
    );
}

/// The termination stream survives the 14-bit container round trip:
/// each raw frame packed into the 14-bit big-endian container decodes
/// through the registry to PCM bit-identical to the raw-path decode —
/// the partial subsubframe and the container unpacking compose.
#[test]
fn fourteen_bit_container_round_trip_on_termination_stream() {
    use oxideav_dts::{pack_16bit_to_14bit, FourteenBitByteOrder};

    let stream = common::build_termination_stream(3);
    let params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));

    let mut raw_dec: Box<dyn Decoder> = make_decoder(&params).expect("factory builds");
    let mut packed_dec: Box<dyn Decoder> = make_decoder(&params).expect("factory builds");
    let mut raw_out: Vec<Vec<i32>> = vec![Vec::new(); 2];
    let mut packed_out: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(&stream) {
        let fv = fv.expect("stream frames iterate cleanly");

        let pkt = Packet::new(0, TimeBase::new(1, 48_000), fv.data.to_vec());
        raw_dec.send_packet(&pkt).expect("raw packet accepted");
        let Frame::Audio(audio) = raw_dec.receive_frame().expect("raw frame decodes") else {
            panic!("expected an audio frame");
        };
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                raw_out[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }

        let (packed, _bits) = pack_16bit_to_14bit(fv.data, FourteenBitByteOrder::BigEndian);
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), packed);
        packed_dec
            .send_packet(&pkt)
            .expect("14-bit packet accepted");
        let Frame::Audio(audio) = packed_dec.receive_frame().expect("14-bit frame decodes") else {
            panic!("expected an audio frame");
        };
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                packed_out[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }

    assert_eq!(raw_out[0].len(), 1024 + TERM_SAMPLES);
    assert_eq!(
        packed_out, raw_out,
        "14-bit container termination decode must be bit-identical to the raw path"
    );
}

/// The §5.3.1 `SHORT` deficit surfaces through
/// [`oxideav_dts::DtsFrameHeader::termination_pad_samples`]: the
/// committed shape asks for 11 pad samples; a normal frame asks for
/// none.
#[test]
fn termination_pad_samples_accessor() {
    let template = common::template_header();
    let term = common::build_frame_from_spec(&template, &JointFrameSpec::default_termination(1));
    let norm = common::build_frame_from_spec(&template, &JointFrameSpec::default_plain(1));
    let term_hdr = parse_frame_header(&term).unwrap();
    let norm_hdr = parse_frame_header(&norm).unwrap();
    assert_eq!(term_hdr.termination_pad_samples(), Some(11));
    assert_eq!(norm_hdr.termination_pad_samples(), None);
}

/// `decode_core_frame_with_info` walks the §5.6 optional-information
/// region from the exact end-of-audio cursor; on a termination frame
/// with no optional payload the walk yields the all-absent record —
/// a frame-level probe that the partial subsubframe's bit accounting
/// left the cursor where §5.6 expects it.
#[test]
fn optional_info_walk_starts_at_exact_end_of_audio() {
    let spec = JointFrameSpec::default_termination(0x0917_0001);
    let frame = common::build_frame_from_spec(&common::template_header(), &spec);
    let header = parse_frame_header(&frame).unwrap();
    let (pcm, info) = decode_core_frame_with_info(&frame, &header).expect("decodes with info");
    for plane in &pcm {
        assert_eq!(plane.len(), TERM_SAMPLES);
    }
    assert!(info.time_code_stamp.is_none());
    assert!(info.aux_bytes.is_empty());
    assert!(info.ocrc.is_none());
}
