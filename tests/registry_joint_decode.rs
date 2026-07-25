//! Registry-surface (`make_decoder` → `send_packet` / `receive_frame`)
//! decode of the **joint-intensity** fixture: the framework path must
//! produce exactly the PCM the direct `CoreStreamDecoder` path
//! produces on a `JOINX != 0` stream — the dual-API contract covers
//! the joint-coding feature, not just the common Core case.

use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, TimeBase};
use oxideav_dts::{
    iter_frames, make_decoder, pack_16bit_to_14bit, CoreStreamDecoder, FourteenBitByteOrder,
    CODEC_ID_STR,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/dts_joint_5_frames.bin");

/// Feed the joint stream frame-by-frame through the registry decoder
/// handle and collect the planar S32 planes.
fn decode_via_registry() -> Vec<Vec<i32>> {
    let params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    let mut dec: Box<dyn Decoder> = make_decoder(&params).expect("factory builds");

    let mut out: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), fv.data.to_vec());
        dec.send_packet(&pkt)
            .expect("send_packet accepts the frame");
        let frame = dec.receive_frame().expect("joint frame decodes");
        let Frame::Audio(audio) = frame else {
            panic!("expected an audio frame");
        };
        assert_eq!(audio.data.len(), 2, "stereo planar output");
        assert_eq!(audio.samples, 512, "one frame = 512 samples/ch");
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                out[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }
    out
}

/// The registry path's PCM equals the direct `CoreStreamDecoder`
/// path's bit-for-bit — including the §C.2.3 joint import and the
/// §C.2.5 effective-`nSUBS` widening, and the persistent inter-packet
/// filter tail.
#[test]
fn registry_joint_decode_matches_direct_path() {
    let via_registry = decode_via_registry();

    let mut dec = CoreStreamDecoder::new(2);
    let mut direct: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("frames iterate");
        let pcm = dec
            .decode_frame(fv.data, &fv.header)
            .expect("direct decode succeeds");
        for ch in 0..2 {
            direct[ch].extend(&pcm[ch]);
        }
    }

    assert_eq!(via_registry.len(), direct.len());
    for ch in 0..2 {
        assert_eq!(via_registry[ch].len(), 2560, "5 frames x 512");
        assert_eq!(
            via_registry[ch], direct[ch],
            "channel {ch}: registry and direct paths must be bit-identical"
        );
    }
    let peak = via_registry[1]
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap();
    assert!(
        peak > 1000,
        "jointly-coded channel non-silent (peak {peak})"
    );
}

/// The joint stream survives the 14-bit container round trip through
/// the registry: each raw frame packed into the 14-bit big-endian
/// container decodes to PCM bit-identical to the raw-path decode —
/// joint-intensity and the container unpacking compose.
#[test]
fn registry_decodes_14bit_joint_container_matching_raw() {
    let raw = decode_via_registry();

    let params = CodecParameters::audio(CodecId::new(CODEC_ID_STR));
    let mut dec: Box<dyn Decoder> = make_decoder(&params).expect("factory builds");
    let mut packed_out: Vec<Vec<i32>> = vec![Vec::new(); 2];
    for fv in iter_frames(FIXTURE) {
        let fv = fv.expect("fixture frames iterate cleanly");
        let (packed, _bits) = pack_16bit_to_14bit(fv.data, FourteenBitByteOrder::BigEndian);
        let pkt = Packet::new(0, TimeBase::new(1, 48_000), packed);
        dec.send_packet(&pkt).expect("14-bit packet accepted");
        let Frame::Audio(audio) = dec.receive_frame().expect("14-bit joint frame decodes") else {
            panic!("expected an audio frame");
        };
        for (ch, plane) in audio.data.iter().enumerate() {
            for c in plane.chunks_exact(4) {
                packed_out[ch].push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }

    assert_eq!(
        packed_out, raw,
        "14-bit container joint decode must be bit-identical to the raw path"
    );
}
