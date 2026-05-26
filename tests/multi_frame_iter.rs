//! Round-6 multi-frame iterator + resync helper, exercised end-to-end
//! against a real `ffmpeg -c:a dca` byte stream.
//!
//! The fixture `tests/fixtures/dts_5_frames.bin` (5 120 bytes) was
//! generated as:
//!
//! ```text
//!   ffmpeg -f lavfi -i "sine=frequency=440:duration=0.05" \
//!          -ac 2 -ar 48000 -c:a dca -strict experimental \
//!          -b:a 768k -f dts tests/fixtures/dts_5_frames.bin
//! ```
//!
//! `ffmpeg` is invoked only as an opaque generator; its source is
//! not consulted. The captured stream contains 5 back-to-back DTS
//! Core frames of 1 024 bytes each. Every frame should parse
//! successfully through [`iter_frames`] and report
//! `frame_size_bytes == 1024`.

use oxideav_dts::{
    find_next_sync, iter_frames, iter_frames_resync, parse_frame_header, FrameType, ResyncCause,
    ResyncEvent, SyncWordEncoding,
};

const FIVE_FRAME_STREAM: &[u8] = include_bytes!("fixtures/dts_5_frames.bin");

#[test]
fn iter_frames_walks_all_five_frames_in_fixture() {
    let frames: Vec<_> = iter_frames(FIVE_FRAME_STREAM)
        .collect::<Result<Vec<_>, _>>()
        .expect("every frame must parse");
    assert_eq!(frames.len(), 5);
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame.offset, i * 1024, "frame {i} starts at offset");
        assert_eq!(frame.len, 1024, "frame {i} length");
        assert_eq!(frame.data.len(), 1024, "frame {i} data slice length");
        assert_eq!(
            frame.header.sync_word_encoding,
            SyncWordEncoding::RawBigEndian,
        );
        assert_eq!(frame.header.frame_type, FrameType::Normal);
        assert_eq!(frame.header.frame_size_bytes, 1024);
        assert_eq!(frame.header.blocks_per_frame, 15);
        assert_eq!(frame.header.sample_count_per_block, 32);
        assert_eq!(frame.header.amode, 2);
        assert_eq!(frame.header.sfreq_index, 13);
        assert_eq!(frame.header.rate_index, 15);
    }
}

#[test]
fn fixture_file_is_a_multiple_of_frame_size() {
    // Sanity check on the bundled fixture: 5 × 1 024 = 5 120 bytes.
    assert_eq!(FIVE_FRAME_STREAM.len(), 5 * 1024);
}

#[test]
fn find_next_sync_returns_each_frame_offset_in_turn() {
    let mut offsets = Vec::new();
    let mut cursor = 0usize;
    while let Some(m) = find_next_sync(FIVE_FRAME_STREAM, cursor) {
        assert_eq!(m.encoding, SyncWordEncoding::RawBigEndian);
        offsets.push(m.offset);
        cursor = m.offset + 1;
    }
    assert_eq!(offsets, vec![0, 1024, 2048, 3072, 4096]);
}

#[test]
fn iter_frames_handles_leading_garbage_before_first_sync() {
    // Prepend 13 bytes of garbage; the iterator should resync to the
    // first real frame at offset 13 and then walk all five.
    let mut buf = Vec::with_capacity(13 + FIVE_FRAME_STREAM.len());
    buf.extend_from_slice(&[0xAA; 13]);
    buf.extend_from_slice(FIVE_FRAME_STREAM);
    let frames: Vec<_> = iter_frames(&buf)
        .collect::<Result<Vec<_>, _>>()
        .expect("garbage-prefixed stream must still parse 5 frames");
    assert_eq!(frames.len(), 5);
    assert_eq!(frames[0].offset, 13);
    assert_eq!(frames[4].offset, 13 + 4 * 1024);
}

#[test]
fn iter_frames_advance_matches_single_frame_parse_at_each_offset() {
    // Every iterator step should produce the same header bytes as a
    // direct call to `parse_frame_header` at the frame's offset.
    for (i, view) in iter_frames(FIVE_FRAME_STREAM).enumerate() {
        let view = view.expect("frame must parse");
        let direct = parse_frame_header(&FIVE_FRAME_STREAM[view.offset..])
            .expect("direct parse at frame offset");
        assert_eq!(
            view.header, direct,
            "frame {i} header differs from direct parse"
        );
    }
}

#[test]
fn iter_frames_terminates_cleanly_after_last_frame() {
    let mut it = iter_frames(FIVE_FRAME_STREAM);
    let mut count = 0;
    for v in it.by_ref() {
        v.expect("frame must parse");
        count += 1;
    }
    assert_eq!(count, 5);
    // Further next() must keep returning None.
    assert!(it.next().is_none());
    assert!(it.next().is_none());
}

#[test]
fn iter_frames_yields_truncation_error_when_last_frame_overruns() {
    // Truncate the fixture to 4 full frames + 100 bytes of a 5th.
    let truncated = &FIVE_FRAME_STREAM[..4 * 1024 + 100];
    let mut ok = 0;
    let mut last_err = None;
    for v in iter_frames(truncated) {
        match v {
            Ok(_) => ok += 1,
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }
    assert_eq!(ok, 4, "first four frames must succeed");
    // The 5th iteration finds a sync at offset 4096 but reports
    // UnexpectedEof because the declared 1 024-byte frame extends
    // past end-of-buffer.
    assert!(last_err.is_some());
}

/// Round 159 — the resync iterator walks the clean ffmpeg fixture
/// byte-for-byte identically to the fail-fast iterator: every step is
/// `Ok` and the frame views match. Confirms the resync iterator is a
/// strict superset (well-formed input is unaffected).
#[test]
fn iter_frames_resync_walks_clean_ffmpeg_fixture_identically() {
    let strict: Vec<_> = iter_frames(FIVE_FRAME_STREAM)
        .collect::<Result<Vec<_>, _>>()
        .expect("every frame must parse via fail-fast iter");
    let resync: Vec<_> = iter_frames_resync(FIVE_FRAME_STREAM)
        .collect::<Result<Vec<_>, ResyncEvent>>()
        .expect("every frame must parse via resync iter");
    assert_eq!(strict.len(), 5);
    assert_eq!(resync.len(), 5);
    for (a, b) in strict.iter().zip(resync.iter()) {
        assert_eq!(a.offset, b.offset);
        assert_eq!(a.len, b.len);
        assert_eq!(a.header, b.header);
    }
}

/// Round 159 — corrupt the second frame's header (e.g. a single-byte
/// flip that lands a NBLKS_high bit and forces NBLKS into the
/// disallowed 0..=4 range) and confirm the resync iterator recovers
/// the third, fourth, and fifth frames after surfacing a ResyncEvent
/// for the corruption. The fail-fast iterator would terminate at the
/// second frame.
#[test]
fn iter_frames_resync_recovers_frames_after_corrupt_header() {
    // Clone the fixture so we can corrupt one byte without touching
    // the bundled file.
    let mut buf = FIVE_FRAME_STREAM.to_vec();
    // Frame 2 starts at offset 1024. Bytes 1024..1028 are the sync;
    // byte 1028 carries (MSB-first) FTYPE(1) | SHORT(5) | CRC(1) |
    // NBLKS_hi(1). Set the whole byte to 0 → NBLKS_hi cleared; then
    // also zero byte 1029 → NBLKS_lo(6) cleared. Result: NBLKS == 0
    // → BlockCountOutOfRange.
    buf[1028] = 0;
    buf[1029] = 0;
    // Make sure the sync at 1024 is still detectable.
    assert_eq!(&buf[1024..1028], &[0x7F, 0xFE, 0x80, 0x01]);

    // Fail-fast iterator: walks frame 1 ok, then errors out at 1024.
    let mut strict = iter_frames(&buf);
    let f1 = strict.next().unwrap().expect("frame 1 ok");
    assert_eq!(f1.offset, 0);
    match strict.next() {
        Some(Err(_)) => {}
        other => panic!("fail-fast must error at frame 2, got {other:?}"),
    }
    assert!(strict.next().is_none(), "fail-fast terminates");

    // Resync iterator: walks frame 1, emits a ResyncEvent for the
    // corrupted frame-2 sync, then keeps walking. Because find_next_sync
    // resumes from offset+1 it will land on frame 3 at offset 2048.
    let mut it = iter_frames_resync(&buf);
    let f1 = it.next().unwrap().expect("frame 1 ok");
    assert_eq!(f1.offset, 0);
    let event = it.next().unwrap().expect_err("frame 2 must fail");
    assert_eq!(event.offset, 1024);
    assert_eq!(event.encoding, SyncWordEncoding::RawBigEndian);
    assert!(matches!(event.cause, ResyncCause::StructuralBoundFailed(_)));
    // Frames 3, 4, 5 recover.
    let f3 = it.next().unwrap().expect("frame 3 ok");
    assert_eq!(f3.offset, 2048);
    let f4 = it.next().unwrap().expect("frame 4 ok");
    assert_eq!(f4.offset, 3072);
    let f5 = it.next().unwrap().expect("frame 5 ok");
    assert_eq!(f5.offset, 4096);
    assert!(it.next().is_none());
}

/// Round 138 — `FrameView::payload()` against the real ffmpeg
/// 5-frame fixture. The fixture's frames have `crc_present == false`
/// (verified by the round-3 black-box test), so the SUBFRAMES region
/// starts 13 bytes into each frame.
#[test]
fn frame_view_payload_matches_header_boundary_on_ffmpeg_fixture() {
    let frames: Vec<_> = iter_frames(FIVE_FRAME_STREAM)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(frames.len(), 5);
    for (i, frame) in frames.iter().enumerate() {
        // The ffmpeg fixture has CRC absent on every frame, so the
        // header window is exactly 13 bytes (104 bits) wide.
        assert!(!frame.header.crc_present, "frame {i}");
        assert_eq!(frame.header.header_byte_length(), 13);
        assert_eq!(frame.header.header_bit_length(), 104);

        let payload = frame.payload();
        // Frame size is 1024 B and the header window is 13 B, so the
        // SUBFRAMES region is exactly 1011 B.
        assert_eq!(payload.len(), 1024 - 13);
        // Payload must be the tail of the frame's data slice.
        assert_eq!(payload.as_ptr(), frame.data[13..].as_ptr());
        // First byte of payload is `data[13]` (the byte immediately
        // following the parsed header window).
        assert_eq!(payload[0], frame.data[13]);
    }
}
