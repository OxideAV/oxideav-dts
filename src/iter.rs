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
//! Round 159 (2026-05-27) adds an error-tolerant counterpart to
//! [`FrameIterator`]:
//!
//! - [`FrameIteratorResync`] / [`iter_frames_resync`] — when a
//!   candidate sync turns out to be a false positive (random payload
//!   bytes that happened to match a 4-byte sync sequence and whose
//!   subsequent header bits fail the structural NBLKS / FSIZE bounds,
//!   or whose declared `frame_size_bytes` overruns end-of-buffer),
//!   surface a [`ResyncEvent`] reporting the discarded offset + cause
//!   and continue scanning from `offset + 1` instead of terminating.
//!   Useful for stream-integrity tooling that needs to walk a
//!   partially-corrupted `.dts` stream past a malformed-syncword
//!   patch.
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

/// Reason a [`FrameIteratorResync`] step discarded a candidate sync
/// position and resumed scanning one byte further.
///
/// Surfaced through [`ResyncEvent::cause`]; callers can route on the
/// variant (e.g. log truncated tails differently from false-positive
/// syncs mid-buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncCause {
    /// The header bits that follow the sync failed one of the
    /// structural bounds the spec mandates (NBLKS &lt; 5 → [`Error::BlockCountOutOfRange`],
    /// frame size &lt; 95 → [`Error::FrameSizeOutOfRange`]) — strong
    /// evidence the sync was a coincidental byte sequence inside a
    /// previous frame's payload rather than the start of a real
    /// frame.
    StructuralBoundFailed(Error),
    /// The candidate sync sat too close to end-of-buffer for the
    /// header bit-width to fit — the parser returned
    /// [`Error::UnexpectedEof`] while reading the header itself.
    HeaderEof,
    /// The header parsed successfully but its declared
    /// `frame_size_bytes` runs past end-of-buffer. The fail-fast
    /// [`FrameIterator`] reports this as
    /// [`Error::UnexpectedEof`] and terminates; the resync iterator
    /// treats the candidate as a false-positive sync and resumes
    /// scanning at `offset + 1`. A genuine truncated tail will
    /// produce one or more `FrameLengthOverrunsBuffer` events near
    /// the end of the input and no further successful frames.
    FrameLengthOverrunsBuffer {
        /// `header.frame_size_bytes` at the discarded position.
        declared_len: u16,
    },
    /// A 14-bit sync was encountered. The fail-fast iterator yields
    /// [`Error::UnsupportedFourteenBit`] and terminates; the resync
    /// iterator instead surfaces this event and continues scanning
    /// past the 14-bit sync, so a stream with intermixed encodings
    /// (e.g. a raw-16-bit stream with a few stray 14-bit-shaped byte
    /// sequences in payload) still walks to completion.
    FourteenBitSyncSkipped,
}

/// One step of [`FrameIteratorResync`] when a candidate sync turned
/// out to be a false positive.
///
/// `offset` and `encoding` come from the underlying
/// [`find_next_sync`] match; `cause` documents which check rejected
/// the candidate (see [`ResyncCause`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResyncEvent {
    /// Absolute byte offset of the discarded sync candidate within
    /// the iterator's input.
    pub offset: usize,
    /// Sync encoding that matched at `offset`.
    pub encoding: SyncWordEncoding,
    /// Why the candidate was rejected.
    pub cause: ResyncCause,
}

/// Error-tolerant counterpart to [`FrameIterator`].
///
/// The fail-fast [`FrameIterator`] surfaces a structural-bound
/// failure at a candidate sync (NBLKS &lt; 5, FSIZE &lt; 95) or a
/// header-truncation by yielding the parser's [`Error`] and
/// terminating — appropriate for callers walking a known-good raw
/// `.dts` stream where any malformed sync is a hard fault.
///
/// `FrameIteratorResync` instead treats those cases as **evidence
/// the candidate sync was a false positive** (a coincidental
/// 4-byte sequence inside another frame's payload that matched the
/// 32-bit syncword), yielding a [`ResyncEvent`] documenting the
/// offset + cause and advancing the cursor by one byte so the scan
/// resumes past the spurious match. This lets stream-integrity
/// tooling walk a partially-corrupted stream past malformed-sync
/// patches and recover frames after the damage.
///
/// Iteration logic per step:
/// 1. Find the next sync at or after the current cursor via
///    [`find_next_sync`]. If none, the iterator ends (yields
///    `None`).
/// 2. If the sync is a 14-bit variant: yield
///    [`ResyncCause::FourteenBitSyncSkipped`] and advance the
///    cursor to `offset + 1` (rather than terminating like
///    [`FrameIterator`] does).
/// 3. Parse the header at the matched offset. On structural
///    failure (`BlockCountOutOfRange` / `FrameSizeOutOfRange` /
///    `UnexpectedEof` while reading the header bits) yield a
///    [`ResyncEvent`] with the appropriate [`ResyncCause`] and
///    advance the cursor to `offset + 1`.
/// 4. If the header parses but `frame_size_bytes` overruns
///    end-of-buffer, yield
///    [`ResyncCause::FrameLengthOverrunsBuffer`] with the declared
///    length and advance the cursor to `offset + 1`. A genuine
///    truncated tail will therefore emit one or more overrun
///    events and then iteration ends naturally when no further
///    sync is found.
/// 5. Otherwise yield `Ok(FrameView)` and advance the cursor by
///    `frame_size_bytes`.
///
/// This iterator is only meaningful for the raw 16-bit encodings
/// (the [`FrameIterator`] docs spell out the 14-bit container-byte
/// advance gap). 14-bit syncs are skipped per step (2) above
/// rather than walked, so a raw-16-bit stream that contains stray
/// 14-bit-shaped byte sequences in payload still walks past them.
#[derive(Debug)]
pub struct FrameIteratorResync<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> FrameIteratorResync<'a> {
    /// Construct a resync iterator positioned at byte 0 of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        FrameIteratorResync { bytes, cursor: 0 }
    }

    /// Current cursor offset (advances on every yielded step,
    /// whether `Ok` or `Err`).
    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

impl<'a> Iterator for FrameIteratorResync<'a> {
    type Item = core::result::Result<FrameView<'a>, ResyncEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        let sync_match = find_next_sync(self.bytes, self.cursor)?;
        let off = sync_match.offset;
        if matches!(
            sync_match.encoding,
            SyncWordEncoding::FourteenBitBigEndian | SyncWordEncoding::FourteenBitLittleEndian
        ) {
            self.cursor = off + 1;
            return Some(Err(ResyncEvent {
                offset: off,
                encoding: sync_match.encoding,
                cause: ResyncCause::FourteenBitSyncSkipped,
            }));
        }
        match parse_frame_header(&self.bytes[off..]) {
            Ok(hdr) => {
                let len = hdr.frame_size_bytes as usize;
                if off + len > self.bytes.len() {
                    self.cursor = off + 1;
                    return Some(Err(ResyncEvent {
                        offset: off,
                        encoding: sync_match.encoding,
                        cause: ResyncCause::FrameLengthOverrunsBuffer {
                            declared_len: hdr.frame_size_bytes,
                        },
                    }));
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
            Err(e) => {
                let cause = match e {
                    Error::UnexpectedEof => ResyncCause::HeaderEof,
                    Error::BlockCountOutOfRange { .. } | Error::FrameSizeOutOfRange { .. } => {
                        ResyncCause::StructuralBoundFailed(e)
                    }
                    // `find_next_sync` already filtered out `NoSync`;
                    // the 14-bit branch is handled above so
                    // `UnsupportedFourteenBit` can't fire here;
                    // `FieldOutOfRange` is encoder-only. Treat any
                    // other variant as a structural false-positive
                    // so the iterator stays total over future enum
                    // extensions.
                    _ => ResyncCause::StructuralBoundFailed(e),
                };
                self.cursor = off + 1;
                Some(Err(ResyncEvent {
                    offset: off,
                    encoding: sync_match.encoding,
                    cause,
                }))
            }
        }
    }
}

/// Convenience constructor — equivalent to
/// [`FrameIteratorResync::new`].
///
/// See [`FrameIteratorResync`] for the resync vs fail-fast
/// trade-off and the per-step yield contract.
pub fn iter_frames_resync(bytes: &[u8]) -> FrameIteratorResync<'_> {
    FrameIteratorResync::new(bytes)
}

/// Scan an entire byte buffer and return every documented DTS sync
/// occurrence it contains.
///
/// This is the bulk-scan counterpart to [`find_next_sync`]: instead of
/// returning the first hit at or after a cursor, it walks the buffer
/// from start to end and collects every position where one of the four
/// documented sync sequences appears. Useful for tooling that needs to
/// validate stream integrity, count frames without parsing them, or
/// build an index of resync points up front.
///
/// The scan honours the same matching rules as [`find_next_sync`]:
///
/// - `7F FE 80 01` — raw big-endian (4 bytes).
/// - `FE 7F 01 80` — raw little-endian (4 bytes).
/// - `1F FF E8 00 07 Fx` — 14-bit packed big-endian (6 bytes, matched
///   on the lower 14 bits of each container per [`detect_sync`]).
/// - `FF 1F 00 E8 Fx 07` — 14-bit packed little-endian (6 bytes,
///   matched on the lower 14 bits of each container).
///
/// Each yielded [`SyncMatch`] reports the absolute byte offset of the
/// first sync byte plus the matched encoding. Overlapping matches are
/// not possible because the four sync sequences differ in their first
/// two bytes (`7F`/`FE`/`1F`/`FF`), but the scan still advances
/// one byte at a time so adjacent sync occurrences (one ending and the
/// next starting on consecutive bytes) are both reported.
///
/// The scan is `O(n)` — each byte is visited at most twice by
/// [`detect_sync`] (one 4-byte raw check, one 6-byte 14-bit check).
/// Callers that only need the first sync should prefer
/// [`find_next_sync`] to avoid materialising the result vector.
///
/// ## Example
///
/// ```
/// use oxideav_dts::{find_all_syncs, SyncWordEncoding};
///
/// let mut buf = vec![0u8; 32];
/// buf[0..4].copy_from_slice(&[0x7F, 0xFE, 0x80, 0x01]);
/// buf[8..12].copy_from_slice(&[0xFE, 0x7F, 0x01, 0x80]);
///
/// let matches = find_all_syncs(&buf);
/// assert_eq!(matches.len(), 2);
/// assert_eq!(matches[0].offset, 0);
/// assert_eq!(matches[0].encoding, SyncWordEncoding::RawBigEndian);
/// assert_eq!(matches[1].offset, 8);
/// assert_eq!(matches[1].encoding, SyncWordEncoding::RawLittleEndian);
/// ```
pub fn find_all_syncs(bytes: &[u8]) -> Vec<SyncMatch> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(m) = find_next_sync(bytes, cursor) {
        out.push(m);
        // Advance by one byte so consecutive (non-overlapping) syncs
        // are both reported. The four documented sync prefixes start
        // with `7F`/`FE`/`1F`/`FF`, all distinct, so no two sync
        // sequences can overlap; a one-byte step is therefore both
        // sufficient and minimal.
        cursor = m.offset + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameType;

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

    // ---------------------------------------------------------------
    // Round 151 — find_all_syncs() bulk-scan helper.
    //
    // Counterpart to find_next_sync() that returns every sync match in
    // a buffer, useful for stream-integrity tooling that needs to know
    // about every resync point up front rather than walking one at a
    // time.
    // ---------------------------------------------------------------

    #[test]
    fn find_all_syncs_empty_buffer_returns_empty_vec() {
        let buf: [u8; 0] = [];
        assert!(find_all_syncs(&buf).is_empty());
    }

    #[test]
    fn find_all_syncs_no_sync_returns_empty_vec() {
        let buf = vec![0xAAu8; 64];
        assert!(find_all_syncs(&buf).is_empty());
    }

    #[test]
    fn find_all_syncs_returns_single_match_for_single_sync() {
        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(&RAW_BE_SYNC);
        let matches = find_all_syncs(&buf);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[0].encoding, SyncWordEncoding::RawBigEndian);
    }

    #[test]
    fn find_all_syncs_mixed_raw_be_and_le_in_one_buffer() {
        // Stream pattern: BE at 0, LE at 8, BE at 16, LE at 24.
        let mut buf = vec![0xAAu8; 32];
        buf[0..4].copy_from_slice(&RAW_BE_SYNC);
        buf[8..12].copy_from_slice(&RAW_LE_SYNC);
        buf[16..20].copy_from_slice(&RAW_BE_SYNC);
        buf[24..28].copy_from_slice(&RAW_LE_SYNC);
        let matches = find_all_syncs(&buf);
        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[0].encoding, SyncWordEncoding::RawBigEndian);
        assert_eq!(matches[1].offset, 8);
        assert_eq!(matches[1].encoding, SyncWordEncoding::RawLittleEndian);
        assert_eq!(matches[2].offset, 16);
        assert_eq!(matches[2].encoding, SyncWordEncoding::RawBigEndian);
        assert_eq!(matches[3].offset, 24);
        assert_eq!(matches[3].encoding, SyncWordEncoding::RawLittleEndian);
    }

    #[test]
    fn find_all_syncs_with_all_four_encodings() {
        // Pack one of each documented sync encoding into a single
        // buffer; find_all_syncs must report all four.
        let mut buf = vec![0xAAu8; 48];
        buf[0..4].copy_from_slice(&RAW_BE_SYNC);
        buf[8..12].copy_from_slice(&RAW_LE_SYNC);
        buf[16..22].copy_from_slice(&[0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xFA]);
        buf[24..30].copy_from_slice(&[0xFF, 0x1F, 0x00, 0xE8, 0xF3, 0x07]);
        let matches = find_all_syncs(&buf);
        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0].encoding, SyncWordEncoding::RawBigEndian);
        assert_eq!(matches[1].encoding, SyncWordEncoding::RawLittleEndian);
        assert_eq!(matches[2].encoding, SyncWordEncoding::FourteenBitBigEndian);
        assert_eq!(
            matches[3].encoding,
            SyncWordEncoding::FourteenBitLittleEndian
        );
    }

    #[test]
    fn find_all_syncs_consecutive_back_to_back_frames() {
        // Two raw-BE syncs adjacent at offsets 0 and 4 — no gap.
        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(&RAW_BE_SYNC);
        buf[4..8].copy_from_slice(&RAW_BE_SYNC);
        let matches = find_all_syncs(&buf);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].offset, 0);
        assert_eq!(matches[1].offset, 4);
    }

    #[test]
    fn find_all_syncs_walks_full_buffer_independent_of_starting_garbage() {
        // 5 bytes of garbage, then BE sync at 5, more garbage, LE sync
        // at 20, trailing tail of garbage.
        let mut buf = vec![0xAAu8; 32];
        buf[5..9].copy_from_slice(&RAW_BE_SYNC);
        buf[20..24].copy_from_slice(&RAW_LE_SYNC);
        let matches = find_all_syncs(&buf);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].offset, 5);
        assert_eq!(matches[1].offset, 20);
    }

    /// Cross-check parity with `find_next_sync` looped from `offset+1`
    /// — `find_all_syncs` must agree with the explicit loop on every
    /// match.
    #[test]
    fn find_all_syncs_matches_find_next_sync_loop() {
        let mut buf = vec![0xAAu8; 64];
        buf[3..7].copy_from_slice(&RAW_BE_SYNC);
        buf[15..19].copy_from_slice(&RAW_LE_SYNC);
        buf[40..44].copy_from_slice(&RAW_BE_SYNC);

        // find_next_sync loop reference.
        let mut reference: Vec<SyncMatch> = Vec::new();
        let mut cursor = 0usize;
        while let Some(m) = find_next_sync(&buf, cursor) {
            reference.push(m);
            cursor = m.offset + 1;
        }

        let bulk = find_all_syncs(&buf);
        assert_eq!(bulk, reference);
    }

    // ---------------------------------------------------------------
    // Round 151 — iter_frames coverage for raw-LE streams.
    //
    // The iterator's `frame_size_bytes`-based advance is documented as
    // applying to "raw 16-bit encodings" (both RawBigEndian AND
    // RawLittleEndian — the FSIZE field is the byte length of the
    // raw 16-bit-per-word on-wire stream, which is byte-equivalent
    // between BE and LE because the LE form is just a pairwise
    // byte-swap of the BE form). The existing
    // `multi_frame_iter.rs::iter_frames_walks_all_five_frames_in_fixture`
    // test exercises the BE path against the ffmpeg fixture; the
    // raw-LE path is exercised here by byte-swapping that fixture
    // (synthesised inline so the iterator's LE walk is covered without
    // adding a new fixture file).
    // ---------------------------------------------------------------

    /// Build a multi-frame raw-LE byte buffer by byte-swapping a
    /// raw-BE frame buffer pairwise. The frame layout (count, sizes,
    /// SUBFRAMES contents) is preserved because raw-LE is defined by
    /// the wiki as the 16-bit-word-swapped form of raw-BE.
    fn build_raw_le_two_frame_stream() -> Vec<u8> {
        // Two back-to-back 96-byte termination frames (NBLKS=5,
        // FSIZE=96). We hand-build the BE form via the same bit-
        // table the parser consumes and then word-swap to LE.
        let mut be = Vec::with_capacity(2 * 96);
        for _frame in 0..2 {
            let mut bv: Vec<bool> = Vec::new();
            fn push(bv: &mut Vec<bool>, value: u32, width: u32) {
                for i in (0..width).rev() {
                    bv.push(((value >> i) & 1) == 1);
                }
            }
            push(&mut bv, 0x7FFE_8001, 32);
            push(&mut bv, 0, 1); // ftype = termination
            push(&mut bv, 0, 5); // sample_count_m1
            push(&mut bv, 0, 1); // crc_present
            push(&mut bv, 5, 7); // nblks
            push(&mut bv, 95, 14); // fsize-1 = 95 -> frame_size = 96
            push(&mut bv, 0, 6); // amode
            push(&mut bv, 0, 4); // sfreq
            push(&mut bv, 0, 5); // rate
            push(&mut bv, 0, 13); // trailing flags
            push(&mut bv, 0, 16); // post-CRC window
            while bv.len() % 8 != 0 {
                bv.push(false);
            }
            // Convert MSB-first bit-vector to bytes.
            let mut frame_bytes = vec![0u8; 96];
            for (i, chunk) in bv.chunks(8).enumerate() {
                let mut b: u8 = 0;
                for (k, bit) in chunk.iter().enumerate() {
                    if *bit {
                        b |= 1 << (7 - k);
                    }
                }
                frame_bytes[i] = b;
            }
            // Fill bytes 13..96 (SUBFRAMES region) with a distinct
            // per-frame pattern so payload checks can differentiate.
            for byte in frame_bytes.iter_mut().skip(13) {
                *byte = 0xCD;
            }
            be.extend_from_slice(&frame_bytes);
        }
        // Word-swap each 16-bit pair to produce the raw-LE form.
        for pair in be.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
        be
    }

    #[test]
    fn iter_frames_walks_raw_le_two_frame_stream() {
        let buf = build_raw_le_two_frame_stream();
        // Sanity: starts with the canonical raw-LE sync.
        assert_eq!(&buf[..4], &[0xFE, 0x7F, 0x01, 0x80]);
        // Second frame's sync at offset 96.
        assert_eq!(&buf[96..100], &[0xFE, 0x7F, 0x01, 0x80]);

        let frames: Vec<_> = iter_frames(&buf)
            .collect::<core::result::Result<Vec<_>, _>>()
            .expect("raw-LE multi-frame stream must walk");
        assert_eq!(frames.len(), 2);

        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame.header.sync_word_encoding,
                SyncWordEncoding::RawLittleEndian,
                "frame {i} encoding",
            );
            assert_eq!(frame.offset, i * 96);
            assert_eq!(frame.len, 96);
            assert_eq!(frame.header.frame_size_bytes, 96);
            assert_eq!(frame.header.blocks_per_frame, 5);
            assert_eq!(frame.header.frame_type, FrameType::Termination);
            assert!(!frame.header.crc_present);
            // The byte-length accessor returns the unpacked-bitstream
            // (raw-BE) header byte count regardless of the on-wire
            // sync encoding — 13 bytes when `crc_present == 0`.
            assert_eq!(frame.header.header_byte_length(), 13);
        }
    }

    /// The raw-LE walk is robust to leading garbage in the same way
    /// the raw-BE walk is — `find_next_sync` skips past unrelated
    /// bytes and lands on the first LE sync.
    #[test]
    fn iter_frames_raw_le_handles_leading_garbage() {
        let stream = build_raw_le_two_frame_stream();
        let mut prefixed = Vec::with_capacity(7 + stream.len());
        prefixed.extend_from_slice(&[0xAA; 7]);
        prefixed.extend_from_slice(&stream);

        let frames: Vec<_> = iter_frames(&prefixed)
            .collect::<core::result::Result<Vec<_>, _>>()
            .expect("garbage-prefixed raw-LE stream must still walk");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].offset, 7);
        assert_eq!(frames[1].offset, 7 + 96);
        for f in &frames {
            assert_eq!(
                f.header.sync_word_encoding,
                SyncWordEncoding::RawLittleEndian
            );
        }
    }

    // -----------------------------------------------------------------
    // Round 159 — FrameIteratorResync.
    //
    // Error-tolerant counterpart to FrameIterator: a candidate sync
    // whose subsequent header bits fail the structural NBLKS / FSIZE
    // bounds (or whose declared frame_size_bytes overruns
    // end-of-buffer) is treated as a false-positive sync rather than a
    // hard fault; the iterator yields a ResyncEvent and continues
    // scanning from offset + 1. Useful for walking partially-corrupted
    // streams past malformed-sync patches.
    // -----------------------------------------------------------------

    /// Bit-vector helper used by the synthetic-frame builders below.
    /// Pushes the low `width` bits of `value` MSB-first into `bv`.
    fn push_bits(bv: &mut Vec<bool>, value: u32, width: u32) {
        for i in (0..width).rev() {
            bv.push(((value >> i) & 1) == 1);
        }
    }

    /// Pack an MSB-first bool vector into bytes, panicking if the
    /// length is not a multiple of 8.
    fn bits_to_bytes(bv: &[bool]) -> Vec<u8> {
        assert_eq!(bv.len() % 8, 0, "bit count must be multiple of 8");
        let mut out = vec![0u8; bv.len() / 8];
        for (i, chunk) in bv.chunks(8).enumerate() {
            let mut b: u8 = 0;
            for (k, bit) in chunk.iter().enumerate() {
                if *bit {
                    b |= 1 << (7 - k);
                }
            }
            out[i] = b;
        }
        out
    }

    /// Build a minimum-size (95-byte) termination raw-BE frame whose
    /// SUBFRAMES region is filled with `fill`. Header fields chosen so
    /// the parser accepts the frame.
    fn build_min_frame_be(fill: u8) -> Vec<u8> {
        let mut bv: Vec<bool> = Vec::new();
        push_bits(&mut bv, 0x7FFE_8001, 32);
        push_bits(&mut bv, 0, 1); // ftype = termination
        push_bits(&mut bv, 0, 5); // sample_count_m1
        push_bits(&mut bv, 0, 1); // crc_present
        push_bits(&mut bv, 5, 7); // nblks = 5
        push_bits(&mut bv, 94, 14); // fsize-1 = 94 → frame_size = 95
        push_bits(&mut bv, 0, 6); // amode
        push_bits(&mut bv, 0, 4); // sfreq
        push_bits(&mut bv, 0, 5); // rate
        push_bits(&mut bv, 0, 13); // trailing flags
        push_bits(&mut bv, 0, 16); // post-CRC window
        while bv.len() % 8 != 0 {
            bv.push(false);
        }
        let mut buf = bits_to_bytes(&bv);
        buf.resize(95, fill);
        for byte in buf.iter_mut().skip(13) {
            *byte = fill;
        }
        buf
    }

    /// On a well-formed stream the resync walker is byte-for-byte
    /// equivalent to the fail-fast iterator: every step yields `Ok`
    /// and the frame views match.
    #[test]
    fn resync_walks_clean_stream_identically_to_fail_fast() {
        // Build a 3-frame raw-BE stream by concatenating three
        // build_min_frame_be calls. Each frame is 95 B → 3 × 95 = 285 B.
        let mut stream = Vec::with_capacity(3 * 95);
        for fill in [0x11u8, 0x22, 0x33] {
            stream.extend_from_slice(&build_min_frame_be(fill));
        }
        assert_eq!(stream.len(), 285);

        let strict: Vec<FrameView<'_>> = iter_frames(&stream)
            .collect::<core::result::Result<Vec<_>, _>>()
            .expect("clean stream must walk");
        let resync: Vec<FrameView<'_>> = iter_frames_resync(&stream)
            .collect::<core::result::Result<Vec<_>, ResyncEvent>>()
            .expect("clean stream must walk through resync iter too");
        assert_eq!(strict.len(), 3);
        assert_eq!(resync.len(), 3);
        for (a, b) in strict.iter().zip(resync.iter()) {
            assert_eq!(a.offset, b.offset);
            assert_eq!(a.len, b.len);
            assert_eq!(a.header, b.header);
        }
    }

    /// A coincidental 4-byte raw-BE sync pattern in the SUBFRAMES
    /// region of a preceding frame triggers a false-positive sync at
    /// that offset. The fail-fast iterator can't see it (it advances
    /// by frame_size_bytes), but a stand-alone sync planted at an
    /// arbitrary offset followed by zero bytes will fail
    /// structural-bound checks: the 7 NBLKS bits decode as 0 → after
    /// the +1 increment that is < 5 → BlockCountOutOfRange. The
    /// resync iterator must surface that as a `StructuralBoundFailed`
    /// ResyncEvent and continue, recovering the real frame that
    /// follows.
    #[test]
    fn resync_skips_false_positive_sync_in_garbage_and_recovers_real_frame() {
        // Layout: [garbage with embedded false sync 0..32] +
        // [real frame at offset 32, 95 B].
        let real = build_min_frame_be(0xCC);
        let mut buf = vec![0xAAu8; 32];
        // Plant the canonical raw-BE sync at offset 8, followed by all
        // zeros for the next ~11 bytes — the parser will read
        // NBLKS == 0 → BlockCountOutOfRange.
        buf[8..12].copy_from_slice(&RAW_BE_SYNC);
        for byte in buf.iter_mut().take(32).skip(12) {
            *byte = 0;
        }
        buf.extend_from_slice(&real);
        // Sanity: fail-fast iter_frames terminates with an error at
        // the false sync without recovering the real frame.
        let mut strict = iter_frames(&buf);
        match strict.next() {
            Some(Err(Error::BlockCountOutOfRange { .. })) => {}
            other => panic!("fail-fast must surface BlockCountOutOfRange, got {other:?}"),
        }
        assert!(strict.next().is_none(), "fail-fast iterator terminates");

        // Resync iterator: one Err event for the false sync, then the
        // real frame, then end.
        let mut it = iter_frames_resync(&buf);
        let first = it.next().expect("must yield event");
        match first {
            Err(ResyncEvent {
                offset: 8,
                encoding: SyncWordEncoding::RawBigEndian,
                cause: ResyncCause::StructuralBoundFailed(Error::BlockCountOutOfRange { .. }),
            }) => {}
            other => panic!("expected StructuralBoundFailed at offset 8, got {other:?}"),
        }
        let second = it.next().expect("must yield real frame");
        let view = second.expect("real frame must parse");
        assert_eq!(view.offset, 32);
        assert_eq!(view.len, 95);
        assert_eq!(
            view.header.sync_word_encoding,
            SyncWordEncoding::RawBigEndian
        );
        assert_eq!(view.header.frame_size_bytes, 95);
        assert!(it.next().is_none());
    }

    /// A false-positive sync at an offset whose declared frame size
    /// overruns end-of-buffer must surface as
    /// `FrameLengthOverrunsBuffer`, not terminate the iterator. After
    /// the event the scan resumes past the spurious sync and finds the
    /// next real frame (or ends naturally if none).
    #[test]
    fn resync_overrun_event_lets_iterator_continue() {
        // Build a buffer: real frame at offset 0 (95 B); then plant a
        // raw-BE sync at offset 96 with a header declaring a huge
        // frame_size (overruns end-of-buffer); then the genuine next
        // frame at offset 96 + 5 = 101.
        //
        // To engineer the "header parses OK but length overruns" case
        // we use a real 95-byte termination frame but place it inside
        // a buffer that is exactly 96+95 = 191 B starting at offset 96
        // — but if we plant the well-formed frame at offset 96, the
        // resync iterator would correctly walk it. So instead we plant
        // a HAND-CRAFTED header at offset 96 with FSIZE that exceeds
        // remaining buffer space.

        let real = build_min_frame_be(0x11);
        let mut buf = real.clone(); // frame at offset 0..95.
                                    // 1 byte of garbage so the false sync is well-separated.
        buf.push(0xAA);
        let false_sync_off = buf.len(); // == 96
                                        // Craft a header with FSIZE that overruns the remaining buffer.
        let total_after_false_sync = 50; // we'll make remaining = 50.
                                         // Declared frame_size = 200 > 50.
        let mut bv: Vec<bool> = Vec::new();
        push_bits(&mut bv, 0x7FFE_8001, 32);
        push_bits(&mut bv, 0, 1);
        push_bits(&mut bv, 0, 5);
        push_bits(&mut bv, 0, 1);
        push_bits(&mut bv, 5, 7); // nblks = 5
        push_bits(&mut bv, 199, 14); // fsize-1 = 199 → frame_size = 200
        push_bits(&mut bv, 0, 6);
        push_bits(&mut bv, 0, 4);
        push_bits(&mut bv, 0, 5);
        push_bits(&mut bv, 0, 13);
        push_bits(&mut bv, 0, 16);
        while bv.len() % 8 != 0 {
            bv.push(false);
        }
        let hdr_bytes = bits_to_bytes(&bv);
        buf.extend_from_slice(&hdr_bytes);
        // Pad with garbage until we have `total_after_false_sync`
        // bytes after the false sync (so the header is fully readable
        // — 13 B — but the declared 200-byte frame overruns).
        while buf.len() - false_sync_off < total_after_false_sync {
            buf.push(0xBB);
        }
        assert_eq!(buf.len() - false_sync_off, total_after_false_sync);

        // Walk with the resync iterator.
        let events: Vec<_> = iter_frames_resync(&buf).collect();
        // Expect: Ok(real frame at 0), Err(FrameLengthOverrunsBuffer at
        // 96, declared_len=200), then end (find_next_sync from 97
        // finds nothing — the canonical sync was only planted at 96).
        assert_eq!(events.len(), 2);
        let frame = events[0].as_ref().unwrap();
        assert_eq!(frame.offset, 0);
        assert_eq!(frame.len, 95);

        match events[1] {
            Err(ResyncEvent {
                offset: 96,
                encoding: SyncWordEncoding::RawBigEndian,
                cause: ResyncCause::FrameLengthOverrunsBuffer { declared_len: 200 },
            }) => {}
            ref other => panic!("expected overrun event at 96 with len 200, got {other:?}"),
        }
    }

    /// A 14-bit sync encountered by the resync iterator is reported
    /// via `FourteenBitSyncSkipped` rather than terminating the walk.
    /// The iterator continues past the 14-bit sync so subsequent raw
    /// frames are still recovered.
    #[test]
    fn resync_skips_fourteen_bit_sync_and_keeps_walking() {
        let real = build_min_frame_be(0xDD);
        let mut buf = Vec::new();
        // 14-bit-BE sync at offset 0 (6 bytes, lower-14-bits match
        // 0x1FFF / 0x2800 / 0x07Fx).
        buf.extend_from_slice(&[0x1F, 0xFF, 0xE8, 0x00, 0x07, 0xFA]);
        // 2 bytes of garbage so the canonical raw-BE sync starts on a
        // byte boundary the scanner will reach without colliding.
        buf.extend_from_slice(&[0xAA, 0xAA]);
        let real_off = buf.len();
        buf.extend_from_slice(&real);

        let mut it = iter_frames_resync(&buf);
        match it.next() {
            Some(Err(ResyncEvent {
                offset: 0,
                encoding: SyncWordEncoding::FourteenBitBigEndian,
                cause: ResyncCause::FourteenBitSyncSkipped,
            })) => {}
            other => panic!("expected 14-bit-BE skip at offset 0, got {other:?}"),
        }
        // Next step recovers the real raw-BE frame.
        let view = it.next().expect("must yield frame").expect("must parse");
        assert_eq!(view.offset, real_off);
        assert_eq!(
            view.header.sync_word_encoding,
            SyncWordEncoding::RawBigEndian
        );
        assert!(it.next().is_none());
    }

    /// Cursor progresses correctly across mixed events: a real frame
    /// advances by frame_size_bytes; a ResyncEvent advances by exactly
    /// one byte.
    #[test]
    fn resync_cursor_advances_one_byte_on_event_and_frame_size_on_ok() {
        let real = build_min_frame_be(0x44);
        let mut buf = vec![0xAAu8; 4];
        buf[0..4].copy_from_slice(&RAW_BE_SYNC); // false sync at 0.
                                                 // Pad so the false-sync header reads zeros → BlockCountOutOfRange.
        buf.resize(20, 0);
        // Real frame at offset 20.
        let real_off = buf.len();
        buf.extend_from_slice(&real);

        let mut it = iter_frames_resync(&buf);
        // Step 1: false sync at 0; cursor must advance to 1.
        let _ = it.next();
        assert_eq!(it.cursor(), 1);
        // Step 2: real frame at 20; cursor advances by frame_size_bytes.
        let _ = it.next();
        assert_eq!(it.cursor(), real_off + 95);
        // Step 3: no more syncs.
        assert!(it.next().is_none());
    }

    /// An empty buffer yields nothing.
    #[test]
    fn resync_empty_buffer_yields_none() {
        let buf: [u8; 0] = [];
        assert!(iter_frames_resync(&buf).next().is_none());
    }

    /// A buffer with no sync at all yields nothing.
    #[test]
    fn resync_no_sync_yields_none() {
        let buf = vec![0xAAu8; 64];
        assert!(iter_frames_resync(&buf).next().is_none());
    }

    /// Multiple consecutive false-positive raw-BE sync sequences are
    /// each reported (in order) and the iterator finally terminates
    /// when no more syncs exist.
    #[test]
    fn resync_multiple_consecutive_false_positives_each_reported() {
        let mut buf = vec![0u8; 64];
        // Three false sync sequences at offsets 0, 16, 32 with all
        // zeros following → each fails NBLKS bound.
        for &off in &[0usize, 16, 32] {
            buf[off..off + 4].copy_from_slice(&RAW_BE_SYNC);
        }
        let events: Vec<_> = iter_frames_resync(&buf).collect();
        assert_eq!(events.len(), 3);
        for (i, &expected_off) in [0usize, 16, 32].iter().enumerate() {
            match events[i] {
                Err(ResyncEvent {
                    offset,
                    encoding: SyncWordEncoding::RawBigEndian,
                    cause: ResyncCause::StructuralBoundFailed(Error::BlockCountOutOfRange { .. }),
                }) if offset == expected_off => {}
                ref other => panic!("event {i} mismatch: {other:?}"),
            }
        }
    }

    /// A genuine truncated tail surfaces as a single
    /// `FrameLengthOverrunsBuffer` event with the declared length;
    /// the iterator then ends because no subsequent sync exists in
    /// the truncated buffer.
    #[test]
    fn resync_truncated_tail_surfaces_overrun_event_then_ends() {
        // Build a valid-header buffer whose declared frame_size_bytes
        // is 95 but the buffer holds only 50 bytes total.
        let real = build_min_frame_be(0xEE);
        let truncated = &real[..50];
        let events: Vec<_> = iter_frames_resync(truncated).collect();
        assert_eq!(events.len(), 1);
        match events[0] {
            Err(ResyncEvent {
                offset: 0,
                encoding: SyncWordEncoding::RawBigEndian,
                cause: ResyncCause::FrameLengthOverrunsBuffer { declared_len: 95 },
            }) => {}
            ref other => panic!("expected overrun event, got {other:?}"),
        }
    }

    /// A header that's truncated mid-bits (sync present, body too
    /// short to read all 104 header bits) yields `HeaderEof`. The
    /// iterator then ends because no subsequent sync follows.
    #[test]
    fn resync_truncated_header_surfaces_header_eof() {
        // Sync + 4 bytes (so the parser sees 8 bytes total — far less
        // than the 13-byte minimum header window).
        let buf = [0x7F, 0xFE, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00];
        let events: Vec<_> = iter_frames_resync(&buf).collect();
        assert_eq!(events.len(), 1);
        match events[0] {
            Err(ResyncEvent {
                offset: 0,
                encoding: SyncWordEncoding::RawBigEndian,
                cause: ResyncCause::HeaderEof,
            }) => {}
            ref other => panic!("expected HeaderEof, got {other:?}"),
        }
    }

    /// The convenience constructor `iter_frames_resync` is equivalent
    /// to `FrameIteratorResync::new`.
    #[test]
    fn iter_frames_resync_matches_struct_new() {
        let real = build_min_frame_be(0x55);
        let a: Vec<_> = iter_frames_resync(&real).collect();
        let b: Vec<_> = FrameIteratorResync::new(&real).collect();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            // FrameView lifetimes are anonymous; compare via fields.
            match (x, y) {
                (Ok(fx), Ok(fy)) => {
                    assert_eq!(fx.offset, fy.offset);
                    assert_eq!(fx.len, fy.len);
                    assert_eq!(fx.header, fy.header);
                }
                (Err(ex), Err(ey)) => assert_eq!(ex, ey),
                _ => panic!("variants differ"),
            }
        }
    }
}
