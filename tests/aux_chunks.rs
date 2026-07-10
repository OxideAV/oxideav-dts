//! Integration coverage for the §5.7 optional-information chunk
//! parsers (§5.7.1 Auxiliary Data + §5.7.2 Rev2 Auxiliary Data)
//! against a real elementary stream and a composed full-size frame.
//!
//! The real-stream half walks the bundled 5-frame fixture and checks
//! that the DWORD-aligned backward scans produce **no false
//! positives** over genuine compressed audio payload. The synthetic
//! half rebuilds a full frame (real header, declared frame size,
//! both chunks embedded at spec-conformant DWORD-aligned offsets)
//! and drives the chunk parsers through the public `FrameView`
//! accessors.

use oxideav_dts::{
    decode_core_frame, decode_core_frame_with_info, iter_frames, parse_aux_data,
    parse_frame_header, parse_rev2_aux, DownmixType, AUX_SYNC_WORD, AUX_TIME_STAMP_MARKER,
    REV2_AUX_SYNC_WORD, REV2_DRC_VERSION_SINGLE_BAND,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/dts_5_frames.bin");

/// Minimal MSB-first bit writer for composing the synthetic chunks.
struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            bytes: Vec::new(),
            bit_len: 0,
        }
    }

    fn push_bits(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            let bit = (value >> i) & 1;
            if self.bit_len % 8 == 0 {
                self.bytes.push(0);
            }
            let byte = self.bytes.last_mut().unwrap();
            *byte |= (bit as u8) << (7 - (self.bit_len % 8));
            self.bit_len += 1;
        }
    }

    fn align_byte(&mut self) {
        while self.bit_len % 8 != 0 {
            self.push_bits(0, 1);
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.align_byte();
        self.bytes
    }
}

/// The real fixture's frames carry no §5.7 chunks: the backward
/// DWORD-aligned scans must return `None` on every frame — i.e. no
/// false-positive sync matches inside genuine compressed payload —
/// and the header flags agree.
#[test]
fn fixture_frames_carry_no_optional_chunks() {
    let mut frames = 0;
    for frame in iter_frames(FIXTURE) {
        let frame = frame.expect("fixture frame parses");
        assert!(!frame.header.aux_data, "fixture sets AUXF unexpectedly");
        assert!(!frame.header.time_stamp, "fixture sets TIMEF unexpectedly");
        assert_eq!(frame.aux_data().unwrap(), None);
        assert_eq!(frame.rev2_aux().unwrap(), None);
        frames += 1;
    }
    assert_eq!(frames, 5);
}

/// The §5.6 Table 5-30 walk runs off the real end-of-audio cursor:
/// on the fixture (TIMEF/AUXF clear, CPF·DYNF clear) the
/// `*_with_info` decode must produce bit-identical PCM to the plain
/// decode plus an empty optional-information region.
#[test]
fn fixture_decode_with_info_matches_plain_decode() {
    for frame in iter_frames(FIXTURE) {
        let frame = frame.expect("fixture frame parses");
        let plain = decode_core_frame(frame.data, &frame.header).expect("plain decode");
        let (pcm, info) =
            decode_core_frame_with_info(frame.data, &frame.header).expect("decode with info");
        assert_eq!(pcm, plain);
        assert_eq!(info.time_code_stamp, None);
        assert_eq!(info.aux_bytes, Vec::<u8>::new());
        assert_eq!(info.ocrc, None);
    }
}

/// Compose a full-size frame — a real fixture header re-encoded with
/// its declared frame size, compressed-looking filler, then both §5.7
/// chunks at DWORD-aligned offsets — and drive the public accessors.
#[test]
fn composed_frame_round_trips_both_chunks() {
    // Real header from the fixture (raw-BE, stereo, no LFE, 16
    // blocks per frame -> 2 subsubframes).
    let first = iter_frames(FIXTURE)
        .next()
        .expect("fixture has a frame")
        .expect("fixture frame parses");
    let header_len = first.header.header_byte_length();
    let frame_size = usize::from(first.header.frame_size_bytes);
    assert_eq!(usize::from(first.header.blocks_per_frame) + 1, 16);

    let mut frame = vec![0u8; frame_size];
    frame[..header_len].copy_from_slice(&first.data[..header_len]);
    // Non-sync filler standing in for the audio payload.
    for (i, b) in frame[header_len..].iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(37) | 0x01;
    }

    // §5.7.1 chunk: time stamp + LoRo downmix for the stereo frame.
    let stamp: u64 = 0xA_BCDE_F012;
    let dmix_codes: [u16; 4] = [0x100 | 241, 0, 0, 0x100 | 241];
    let mut w = BitWriter::new();
    w.push_bits(u64::from(AUX_SYNC_WORD), 32);
    w.push_bits(1, 1); // time stamp present
    w.push_bits(0, 3); // Advance2Next4BitPos
    w.push_bits((stamp >> 28) & 0xFF, 8);
    w.push_bits(u64::from(AUX_TIME_STAMP_MARKER), 4);
    w.push_bits(stamp & 0x0FFF_FFFF, 28);
    w.push_bits(u64::from(AUX_TIME_STAMP_MARKER), 4);
    w.push_bits(1, 1); // dynamic coeffs present
    w.push_bits(u64::from(DownmixType::LoRo.code()), 3);
    for &c in &dmix_codes {
        w.push_bits(u64::from(c), 9);
    }
    w.align_byte();
    w.push_bits(0x1357, 16); // nAUXCRC16
    let aux_chunk = w.into_bytes();

    // §5.7.2 chunk: ES scale + broadcast DRC (2 subsubframes) +
    // dialnorm, declared byte size 8.
    let mut w = BitWriter::new();
    w.push_bits(u64::from(REV2_AUX_SYNC_WORD), 32);
    w.push_bits(8 - 1, 7); // nRev2AUXDataByteSize = 8
    w.push_bits(1, 1); // bESMetaDataFlag
    w.push_bits(100, 8); // nEmbESDownMixScaleIndex (-20 dB)
    w.push_bits(1, 1); // bBroadcastMetadataPresent
    w.push_bits(1, 1); // bDRCMetadataPresent
    w.push_bits(1, 1); // bDialnormMetadata
    w.push_bits(u64::from(REV2_DRC_VERSION_SINGLE_BAND), 4);
    w.align_byte(); // nByteAlign0
    w.push_bits(127, 8); // subsubframe 0 DRC (unity)
    w.push_bits(87, 8); // subsubframe 1 DRC (§D.4 0.3162)
    w.push_bits(9, 5); // DIALNORM_rev2aux -> -9 dB
    let mut rev2_chunk = w.into_bytes();
    // Pad so the CRC lands exactly size-2 bytes past the size field
    // (4 sync bytes + 8 - 2 = byte 10 of the chunk).
    rev2_chunk.resize(10, 0);
    rev2_chunk.extend_from_slice(&0x2468u16.to_be_bytes());

    // Embed both chunks DWORD-aligned: aux first, Rev2 after it.
    let rev2_at = frame_size - rev2_chunk.len(); // frame size is DWORD-multiple
    assert_eq!(rev2_at % 4, 0);
    let aux_at = (rev2_at - aux_chunk.len() - 8) & !3;
    frame[aux_at..aux_at + aux_chunk.len()].copy_from_slice(&aux_chunk);
    // Clear the gap so no stale filler byte fakes a sync.
    for b in &mut frame[aux_at + aux_chunk.len()..rev2_at] {
        *b = 0;
    }
    frame[rev2_at..].copy_from_slice(&rev2_chunk);

    // Parse through the standalone entry points.
    let header = parse_frame_header(&frame).expect("composed frame header parses");
    let aux = parse_aux_data(&frame, &header)
        .expect("aux chunk parses")
        .expect("aux chunk found");
    assert_eq!(aux.offset, aux_at);
    assert_eq!(aux.time_stamp, Some(stamp));
    assert_eq!(aux.crc16, 0x1357);
    let dmix = aux.dynamic_downmix.as_ref().expect("downmix present");
    assert_eq!(dmix.downmix_type, DownmixType::LoRo);
    assert_eq!(dmix.input_channel_count, 2);
    let matrix = dmix.coefficient_matrix().unwrap();
    assert_eq!(matrix, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    // Identity fold: planar PCM passes through unchanged.
    let pcm = vec![vec![3, -5, 32767], vec![-2, 9, -32768]];
    assert_eq!(dmix.apply_planar(&pcm).unwrap(), pcm);

    let rev2 = parse_rev2_aux(&frame, &header)
        .expect("rev2 chunk parses")
        .expect("rev2 chunk found");
    assert_eq!(rev2.offset, rev2_at);
    assert_eq!(rev2.data_byte_size, 8);
    assert_eq!(rev2.es_downmix_scale_index, Some(100));
    let es = rev2.es_downmix_scale().unwrap();
    assert!((es - 3277.0 / 32768.0).abs() < 1e-12); // -20 dB
    let drc = rev2.drc.as_ref().expect("DRC present");
    assert_eq!(drc.version, 1);
    assert_eq!(drc.codes, vec![127, 87]);
    let mult = drc.multipliers();
    assert_eq!(mult[0], 1.0);
    assert!((mult[1] - 0.3162).abs() < 1e-12);
    assert_eq!(rev2.dialnorm, Some(9));
    assert_eq!(rev2.dialog_normalization_gain_db(), Some(-9));
    assert_eq!(rev2.crc16, 0x2468);

    // And through the FrameView accessors.
    let view = iter_frames(&frame)
        .next()
        .expect("composed frame iterates")
        .expect("composed frame parses");
    assert_eq!(view.aux_data().unwrap().as_ref(), Some(&aux));
    assert_eq!(view.rev2_aux().unwrap().as_ref(), Some(&rev2));
}
