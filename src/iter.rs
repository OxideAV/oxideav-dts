//! Multi-frame iteration over a DTS Core byte stream.
//!
//! Round 6 (2026-05-25) adds two demuxer-friendly helpers on top of
//! the single-frame [`crate::parse_frame_header`] /
//! [`crate::parse_frame_header_14bit`] entry points:
//!
//! - [`find_next_sync`] — scan a byte buffer for the next DTS sync
//!   sequence starting at an arbitrary offset, returning both the
//!   offset and the [`crate::SyncWordEncoding`] that matched. Useful
//!   for callers that need to resynchronise after lost bytes, drop
//!   leading container padding, or walk a raw `.dts` file frame by
//!   frame.
//! - [`FrameIterator`] / [`iter_frames`] — walk a byte buffer one
//!   frame at a time. Each successful step parses the frame's header,
//!   reports its byte range, and advances by
//!   [`crate::DtsFrameHeader::frame_size_bytes`] (for raw 16-bit
//!   encodings; see the function docs for the 14-bit advance rule).
//!
//! Neither helper depends on the [`oxideav-core`] integration; both
//! are available in the `--no-default-features` build alongside the
//! standalone parsers.
//!
//! ## What stays out of scope
//!
//! - Container-stream parsing: a raw `.dts` file is a concatenation
//!   of self-delimited Core frames, which is the only shape these
//!   helpers walk. AVI / MP4 / Matroska sample carriage stays in
//!   their respective container crates; the helpers here operate on
//!   raw codec bytes only.
//! - PCM sample emission. The iterator surfaces parsed
//!   [`crate::DtsFrameHeader`] records and the raw frame byte slice;
//!   subband / QMF / Huffman decoding remains gated on the spec
//!   tables in the docs gaps.
//! - 14-bit byte-advance for [`FrameIterator`]. The iterator only
//!   advances on the **raw 16-bit** sync variants because the
//!   `frame_size_bytes` field is documented as the byte length of
//!   the unpacked stream — for 14-bit-packed containers the
//!   corresponding container-byte advance would be
//!   `frame_size_bytes * 8 / 14` rounded up to the next even byte,
//!   which the wiki snapshot does **not** spell out. The 14-bit
//!   single-frame [`crate::parse_frame_header_14bit`] entry point
//!   remains available for callers that have already partitioned
//!   their 14-bit input into frame-sized slices. See `README.md`'s
//!   round-6 docs gap #7.

use crate::header::{detect_sync, parse_frame_header};
use crate::{DtsFrameHeader, Error, Result, SyncWordEncoding};

/// Result of a [`find_next_sync`] lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncMatch {
    /// Absolute byte offset (within the input slice passed to
    /// [`find_next_sync`]) where the sync sequence starts.
    pub offset: usize,
    /// Which of the four documented sync encodings was found.
    pub encoding: SyncWordEncoding,
}

/// Scan `bytes[start..]` for the next DTS Core sync sequence.
///
/// Returns the byte offset (in the original `bytes` slice, not in
/// `bytes[start..]`) and the matched [`SyncWordEncoding`] of the
/// first sync found at or after `start`, or `None` if no sync
/// appears before end-of-buffer.
///
/// All four documented sync sequences are accepted:
///
/// - `7F FE 80 01` — raw big-endian (4 bytes).
/// - `FE 7F 01 80` — raw little-endian (4 bytes).
/// - `1F FF E8 00 07 Fx` — 14-bit packed big-endian (6 bytes).
/// - `FF 1F 00 E8 Fx 07` — 14-bit packed little-endian (6 bytes).
///
/// The 14-bit variants are matched via the lower-14-bit payloads of
/// the first three containers (`0x1FFF`, `0x2800`,
/// top-4-of-`0x07F?`), matching the same widened detection rule the
/// single-frame parser uses (see [`detect_sync`] in `header.rs`).
///
/// The scan is `O(n)`: every byte is visited at most twice (once for
/// the 4-byte raw-sync check, once for the 6-byte 14-bit-sync
/// check). Calling [`find_next_sync`] in a loop with the previous
/// `offset + 1` is the standard resync pattern.
pub fn find_next_sync(bytes: &[u8], start: usize) -> Option<SyncMatch> {
    if start >= bytes.len() {
        return None;
    }
    let mut i = start;
    // We need at least 4 bytes for the shortest sync (raw 16-bit) and
    // 6 bytes for the longest (14-bit packed). Stop the scan at the
    // last position that could still hold a raw sync.
    while i + 4 <= bytes.len() {
        if let Ok(enc) = detect_sync(&bytes[i..]) {
            return Some(SyncMatch {
                offset: i,
                encoding: enc,
            });
        }
        i += 1;
    }
    None
}

/// One step of a [`FrameIterator`] over a raw-16-bit DTS Core
/// stream.
#[derive(Debug, Clone, Copy)]
pub struct FrameView<'a> {
    /// Parsed header.
    pub header: DtsFrameHeader,
    /// Absolute byte offset of the frame's first sync byte within
    /// the input passed to [`iter_frames`].
    pub offset: usize,
    /// Byte length of the frame (from
    /// [`DtsFrameHeader::frame_size_bytes`]). Always 95..=16384.
    pub len: usize,
    /// Borrowed frame bytes (`bytes[offset..offset + len]`).
    pub data: &'a [u8],
}

impl<'a> FrameView<'a> {
    /// SUBFRAMES region of the frame: the bytes that follow the
    /// fully-decoded frame-sync header.
    ///
    /// Equivalent to `&self.data[self.header.header_byte_length()..]`.
    /// The wiki snapshot (`docs/audio/dts/wiki/DTS.wiki`) marks this
    /// region as `'''TODO'''`; subband / QMF / Huffman / VQ decoding
    /// remains gated on the §5.3.1 value tables and the §5.4 polyphase
    /// filterbank landing in `docs/`. The helper is exposed so
    /// downstream code (re-muxers, payload-CRC validators, future
    /// subframe decoders) can carve out the region without recomputing
    /// the header boundary.
    ///
    /// Always non-empty for well-formed frames: the smallest documented
    /// frame size is 95 B and the largest header window is 15 B
    /// (`crc_present == true`), so at least 80 B of SUBFRAMES region
    /// is guaranteed.
    pub fn payload(&self) -> &'a [u8] {
        &self.data[self.header.header_byte_length()..]
    }
}

/// Iterator that walks a raw-16-bit DTS Core byte buffer frame by
/// frame.
///
/// Each [`Iterator::next`] step:
/// 1. Calls [`find_next_sync`] from the current cursor to handle any
///    leading garbage / inter-frame padding the source may have
///    introduced. The iterator does NOT assume `bytes[0]` is a sync
///    byte; it scans for one.
/// 2. Calls [`parse_frame_header`] at the located offset. A parse
///    failure (no sync within reach, truncated header, NBLKS or
///    FSIZE out of range) is returned as the next item's `Err`
///    variant; subsequent calls then return `None`.
/// 3. On success, yields a [`FrameView`] borrowing the frame's
///    bytes from the input and advances the cursor by
///    `header.frame_size_bytes` so the following step parses the
///    next sync.
///
/// The iterator only accepts raw 16-bit encodings (`RawBigEndian` /
/// `RawLittleEndian`). When [`find_next_sync`] locates a 14-bit
/// sync, the iterator yields a single [`Error::UnsupportedFourteenBit`]
/// item and then terminates. Callers walking 14-bit container
/// streams must externally partition the input into frame-sized
/// slices and feed each slice to [`crate::parse_frame_header_14bit`]
/// directly. See `README.md`'s round-6 docs gap #7 for the
/// container-byte-advance rule.
#[derive(Debug)]
pub struct FrameIterator<'a> {
    bytes: &'a [u8],
    cursor: usize,
    done: bool,
}

impl<'a> FrameIterator<'a> {
    /// Construct an iterator positioned at byte 0 of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        FrameIterator {
            bytes,
            cursor: 0,
            done: false,
        }
    }

    /// Current cursor offset. After a successful step the cursor
    /// points at the next frame's expected sync byte (or one past
    /// end-of-buffer if the previous frame was the last). After a
    /// failure the cursor stays at the point of failure.
    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

impl<'a> Iterator for FrameIterator<'a> {
    type Item = Result<FrameView<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let sync_match = find_next_sync(self.bytes, self.cursor)?;
        match sync_match.encoding {
            SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian => {
                // 14-bit container streams need an out-of-band
                // container-byte advance rule which the wiki snapshot
                // does not enumerate. Surface the limitation and
                // terminate so the caller switches to the
                // single-frame 14-bit entry point.
                self.done = true;
                return Some(Err(Error::UnsupportedFourteenBit));
            }
            _ => {}
        }
        let off = sync_match.offset;
        let parse_result = parse_frame_header(&self.bytes[off..]);
        let hdr = match parse_result {
            Ok(h) => h,
            Err(e) => {
                self.done = true;
                self.cursor = off;
                return Some(Err(e));
            }
        };
        let len = hdr.frame_size_bytes as usize;
        if off + len > self.bytes.len() {
            // Header says the frame extends past end-of-buffer. We
            // still surface the header (the caller may want to know
            // the truncation occurred at this offset with this
            // size), but mark the iterator done.
            self.done = true;
            self.cursor = off;
            return Some(Err(Error::UnexpectedEof));
        }
        let view = FrameView {
            header: hdr,
            offset: off,
            len,
            data: &self.bytes[off..off + len],
        };
        self.cursor = off + len;
        Some(Ok(view))
    }
}

/// Convenience constructor — equivalent to [`FrameIterator::new`].
pub fn iter_frames(bytes: &[u8]) -> FrameIterator<'_> {
    FrameIterator::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW_BE_SYNC: [u8; 4] = [0x7F, 0xFE, 0x80, 0x01];
    const RAW_LE_SYNC: [u8; 4] = [0xFE, 0x7F, 0x01, 0x80];

    #[test]
    fn find_next_sync_at_offset_zero() {
        let mut buf = vec![0u8; 16];
        buf[..4].copy_from_slice(&RAW_BE_SYNC);
        let m = find_next_sync(&buf, 0).unwrap();
        assert_eq!(m.offset, 0);
        assert_eq!(m.encoding, SyncWordEncoding::RawBigEndian);
    }

    #[test]
    fn find_next_sync_skips_leading_garbage() {
        let mut buf = vec![0xAAu8; 32];
        buf[7..11].copy_from_slice(&RAW_BE_SYNC);
        let m = find_next_sync(&buf, 0).unwrap();
        assert_eq!(m.offset, 7);
        assert_eq!(m.encoding, SyncWordEncoding::RawBigEndian);
    }

    #[test]
    fn find_next_sync_le_variant() {
        let mut buf = vec![0u8; 16];
        buf[3..7].copy_from_slice(&RAW_LE_SYNC);
        let m = find_next_sync(&buf, 0).unwrap();
        assert_eq!(m.offset, 3);
        assert_eq!(m.encoding, SyncWordEncoding::RawLittleEndian);
    }

    #[test]
    fn find_next_sync_14bit_be() {
        let mut buf = vec![0u8; 16];
        buf[5..11].copy_from_slice(&[0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xFA]);
        let m = find_next_sync(&buf, 0).unwrap();
        assert_eq!(m.offset, 5);
        assert_eq!(m.encoding, SyncWordEncoding::FourteenBitBigEndian);
    }

    #[test]
    fn find_next_sync_14bit_le() {
        let mut buf = vec![0u8; 16];
        buf[5..11].copy_from_slice(&[0xFF, 0x1F, 0x00, 0xE8, 0xF3, 0x07]);
        let m = find_next_sync(&buf, 0).unwrap();
        assert_eq!(m.offset, 5);
        assert_eq!(m.encoding, SyncWordEncoding::FourteenBitLittleEndian);
    }

    #[test]
    fn find_next_sync_honours_start_offset() {
        // Two syncs in the buffer; start past the first.
        let mut buf = vec![0xAAu8; 32];
        buf[2..6].copy_from_slice(&RAW_BE_SYNC);
        buf[20..24].copy_from_slice(&RAW_BE_SYNC);
        let m = find_next_sync(&buf, 7).unwrap();
        assert_eq!(m.offset, 20);
    }

    #[test]
    fn find_next_sync_returns_none_when_absent() {
        let buf = vec![0xAAu8; 64];
        assert_eq!(find_next_sync(&buf, 0), None);
    }

    #[test]
    fn find_next_sync_returns_none_when_start_past_end() {
        let buf = vec![0u8; 16];
        assert_eq!(find_next_sync(&buf, 100), None);
    }

    #[test]
    fn find_next_sync_returns_none_when_only_partial_sync_at_tail() {
        // Three bytes of an in-progress raw sync (`7F FE 80`) but no
        // fourth byte — the scanner walks up to length-4, finds
        // nothing, and returns None.
        let mut buf = vec![0xAAu8; 8];
        buf[5..8].copy_from_slice(&[0x7F, 0xFE, 0x80]);
        assert_eq!(find_next_sync(&buf, 0), None);
    }

    // ---------------------------------------------------------------
    // Round 138 — FrameView::payload()
    // ---------------------------------------------------------------

    /// Hand-build a 95-byte raw-BE termination frame (NBLKS=5,
    /// FSIZE=95, crc_present=0) padded with zeros for the SUBFRAMES
    /// region; then confirm `FrameView::payload()` returns exactly
    /// `frame.len - header_byte_length()` bytes (= 95 - 13 = 82).
    #[test]
    fn frame_view_payload_slice_length_matches_header_boundary_no_crc() {
        // The base 13-byte header window is followed by 82 bytes of
        // SUBFRAMES region we fill with a distinctive 0xCD pattern
        // so the slice content can be checked too.
        let mut buf = vec![0u8; 95];
        // Sync.
        buf[0..4].copy_from_slice(&[0x7F, 0xFE, 0x80, 0x01]);
        // FTYPE=0 (termination, MSB), SHORT=0, CRC_PRESENT=0,
        // NBLKS=5 (7 bits), FSIZE-1=94 (14 bits = frame_size 95),
        // AMODE=0 (6 bits), SFREQ=0 (4 bits), RATE=0 (5 bits) +
        // 13 zero trailing bits + 16 zero post-CRC bits = 75 bits
        // after the sync = 13 bytes total.
        // Easier: just call build_be_header equivalent through
        // parse_frame_header on a synthesised buffer.
        // Bytes layout (after sync):
        //  byte 4: FTYPE(1)=0 SHORT(5)=0 CRC_PRESENT(1)=0 NBLKS_hi(1)=0 -> 0b00000000 = 0x00
        //  byte 5: NBLKS_lo(6)=000101 FSIZE_hi(2)=00 -> 0b00010100 = 0x14
        //  byte 6: FSIZE_mid(8)=00010111 -> 0x17  (FSIZE-1 = 94 = 0b00_00000101_1110, hi 2 bits=00, mid 8=00010111? — recompute below)
        // Rather than hand-bit-fiddle, sidestep the layout: use
        // `parse_frame_header` on a 95-byte buffer we synthesise via
        // the header.rs test helper indirectly by re-deriving the
        // bit layout here. Simpler: directly construct via a tiny
        // local builder.
        fn push(bv: &mut Vec<bool>, value: u32, width: u32) {
            for i in (0..width).rev() {
                bv.push(((value >> i) & 1) == 1);
            }
        }
        let mut bv: Vec<bool> = Vec::new();
        push(&mut bv, 0x7FFE_8001, 32);
        push(&mut bv, 0, 1); // ftype = termination
        push(&mut bv, 0, 5); // sample_count_m1
        push(&mut bv, 0, 1); // crc_present
        push(&mut bv, 5, 7); // nblks
        push(&mut bv, 94, 14); // fsize-1 = 94 -> frame_size = 95
        push(&mut bv, 0, 6); // amode
        push(&mut bv, 0, 4); // sfreq
        push(&mut bv, 0, 5); // rate
        push(&mut bv, 0, 13); // trailing flags
        push(&mut bv, 0, 16); // post-CRC window
        while bv.len() % 8 != 0 {
            bv.push(false);
        }
        // Convert bit-vector to bytes (MSB-first).
        for (i, chunk) in bv.chunks(8).enumerate() {
            let mut b: u8 = 0;
            for (k, bit) in chunk.iter().enumerate() {
                if *bit {
                    b |= 1 << (7 - k);
                }
            }
            buf[i] = b;
        }
        // Fill the SUBFRAMES region (bytes 13..95) with a pattern so
        // the payload-slice contents can be verified.
        for byte in buf.iter_mut().skip(13) {
            *byte = 0xCD;
        }

        let mut it = iter_frames(&buf);
        let view = it.next().expect("frame must yield").expect("must parse");
        assert_eq!(view.offset, 0);
        assert_eq!(view.len, 95);
        assert_eq!(view.header.frame_size_bytes, 95);
        assert!(!view.header.crc_present);
        assert_eq!(view.header.header_byte_length(), 13);

        let payload = view.payload();
        assert_eq!(payload.len(), 95 - 13);
        assert!(payload.iter().all(|&b| b == 0xCD));
        // No more frames in the buffer.
        assert!(it.next().is_none());
    }

    /// `FrameView::payload()` shifts by 2 bytes (15 vs 13) when
    /// `crc_present == 1` because the optional HEADER_CRC slot
    /// extends the header window from 104 to 120 bits.
    #[test]
    fn frame_view_payload_offset_shifts_when_crc_present() {
        // Build a 95-byte crc-present termination frame.
        let mut buf = vec![0u8; 95];
        fn push(bv: &mut Vec<bool>, value: u32, width: u32) {
            for i in (0..width).rev() {
                bv.push(((value >> i) & 1) == 1);
            }
        }
        let mut bv: Vec<bool> = Vec::new();
        push(&mut bv, 0x7FFE_8001, 32);
        push(&mut bv, 0, 1);
        push(&mut bv, 0, 5);
        push(&mut bv, 1, 1); // crc_present = true
        push(&mut bv, 5, 7);
        push(&mut bv, 94, 14);
        push(&mut bv, 0, 6);
        push(&mut bv, 0, 4);
        push(&mut bv, 0, 5);
        push(&mut bv, 0, 13);
        push(&mut bv, 0xABCD, 16); // header_crc
        push(&mut bv, 0, 16); // post-CRC window
        while bv.len() % 8 != 0 {
            bv.push(false);
        }
        for (i, chunk) in bv.chunks(8).enumerate() {
            let mut b: u8 = 0;
            for (k, bit) in chunk.iter().enumerate() {
                if *bit {
                    b |= 1 << (7 - k);
                }
            }
            buf[i] = b;
        }
        // Fill bytes 15..95 with a distinct pattern.
        for byte in buf.iter_mut().skip(15) {
            *byte = 0xEF;
        }

        let view = iter_frames(&buf).next().unwrap().unwrap();
        assert!(view.header.crc_present);
        assert_eq!(view.header.header_byte_length(), 15);
        let payload = view.payload();
        assert_eq!(payload.len(), 95 - 15);
        assert!(payload.iter().all(|&b| b == 0xEF));
    }
}
