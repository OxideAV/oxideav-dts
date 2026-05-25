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

use oxideav_dts::{find_next_sync, iter_frames, parse_frame_header, FrameType, SyncWordEncoding};

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
