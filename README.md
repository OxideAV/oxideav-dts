# oxideav-dts

A pure-Rust DTS audio decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework.

## Status

**Round 195 — §5.4.1 ABITS / SCALES (a.k.a. ALLOC / SCFAC) side-info decoders.**
Round 195 (2026-05-31) lands the side-information half of the core
subframe decode path: extracting the per-channel × per-subband
ABITS bit-allocation index field and the per-channel × per-subband
SCALES scale-factor field from a packed bit stream, given the
channel-wide `BHUFF[ch]` / `SHUFF[ch]` codebook selectors read
earlier from the AUDIO CODING HEADER. Three new public entry points:
`AbitsCodebook::from_bhuff(u8)` / `ScalesCodebook::from_shuff(u8)`
(Table 5-25 / Table 5-24 selectors), and the byte-slice + bit-offset
single-field decoders `decode_abits_at(bytes, bit_offset, codebook)`
(returns `(abits, bits_consumed)`) and
`decode_scales_at(bytes, bit_offset, codebook, n_scale_sum)`
(returns `(scale, updated_n_scale_sum, bits_consumed)`). Backing
tables: Annex D §D.5.6 five 12-level Huffman codebooks A12/B12/C12/
D12/E12 (BHUFF=0..4), Annex D §D.5.3 + §D.5.4 small-Huffman
codebooks A5/B5/C5 + A7/B7 routed to SA129..SE129 difference
symbols (SHUFF=0..4), and the §D.1.1 / §D.1.2 RMS square-root
quantisation tables (`RMS_6BIT: [u32; 64]` /
`RMS_7BIT: [u32; 128]`) as `pub const` arrays. Two new error
variants — `Error::InvalidSideInfo { field, value }` (reserved
BHUFF/SHUFF/SCALES values) and `Error::HuffmanDecodeFailed { table }`
(defensive bound — the Annex D codebooks are all complete prefix
codes by Kraft equality, so this fires only on EOF or stream-format
corruption). Nineteen unit tests in `src/side_info.rs` plus three
integration tests in `tests/side_info_decode.rs` lock down the
behavioural contract: BHUFF/SHUFF reserved-value rejection,
exhaustive 7-code dispatch, every ABITS Huffman symbol round-trip
across all five 12-level codebooks, Kraft completeness across all
ten transcribed codebooks, RMS table length + anchor-value
cross-check against the staged PDF, SA129 difference accumulation
across `(+2, +1, 0, -1, -2)` from `n_scale_sum=10`, SD129 7-level
table with ±3 range, negative-accumulator + reserved-index
rejection, and a 5-subband end-to-end block walked through the
public API. Scope: single-field decode + tables only; the full
subframe walker (which also requires the §5.3.x AUDIO CODING
HEADER fields SUBFS/PCHS/SUBS/VQSUB/JOINX and the SCALES loop over
`nPCHS × nSUBS[ch]`) is a follow-up. The 129-entry SA129..SE129
full mappings (Table 5-24's nominal codebook names, not
transcribed under those names in the staged Annex D revision)
remain a docs-completeness gap; this round routes SHUFF=0..4
through the small-Huffman §D.5.3 / §D.5.4 codebooks the staged
PDF does enumerate, treating their symbols as scale-factor index
differences per the §5.4.1 pseudocode.

**Round 192 — 14-bit container-byte frame iterator `iter_frames_14bit`.**
Round 192 (2026-05-30) closes the empirical half of round-6 docs gap
#7 by wiring the round-189 `frame_size_container_bytes` accessor into
a multi-frame walker that operates directly on 14-bit-packed
container bytes. The new `iter_frames_14bit(bytes)` returns a
`FrameIterator14<'_>` whose `Iterator::next` step calls
`find_next_sync` to handle leading garbage, accepts only the two
14-bit syncs (`FourteenBitBigEndian` / `FourteenBitLittleEndian`),
calls the existing `parse_frame_header_14bit` to recover the typed
header from each frame's container window, and advances the cursor
by `header.frame_size_container_bytes(encoding)` container bytes —
the round-189 formula `2 * ceil(frame_size_bytes * 8 / 14)`. The
per-step `FrameView14` is a deliberate separate type (not
`FrameView`) because the `len` and `data` fields carry container-
domain semantics here (container-byte advance + container-byte
window) rather than the unpacked-domain semantics they carry in
`FrameView`. A raw 16-bit sync at the iterator's cursor surfaces the
new `Error::UnsupportedRaw16Bit` variant (symmetric counterpart to
the round-6 `Error::UnsupportedFourteenBit` on `iter_frames`) and
terminates. Twelve new tests lock the iterator's contract down: ten
unit tests (single-frame BE / LE walks; back-to-back BE frames with
cursor + length cross-check; leading garbage before first sync;
raw-16-bit sync rejection; empty buffer; no-sync buffer; truncated
tail reporting `UnexpectedEof`; `view.data` round-trips through
`parse_frame_header_14bit`; `cursor()` advances by exactly
`frame_size_container_bytes` per step) plus two integration tests
that repackage the bundled ffmpeg 5-frame fixture (5 × 1024 raw-BE
bytes) into 14-bit-packed BE and LE streams (5 × 1172 container
bytes each) and verify all five frames walk with the expected
header fields and container-byte length. The fail-fast
`iter_frames` from round 6 is unchanged — it still rejects 14-bit
syncs with `UnsupportedFourteenBit` because raw streams and
container streams live in distinct domains; callers route by sync
encoding up-front.

**Round 189 — 14-bit container-byte frame-advance accessor (ETSI §5.3.1 + §6.1.3.1).**
Round 189 (2026-05-30) adds a single new accessor,
`DtsFrameHeader::frame_size_container_bytes(SyncWordEncoding) -> u32`,
that returns the container-byte distance from this frame's syncword
to the next frame's syncword for each of the four wire encodings.
For the raw 16-bit encodings (`RawBigEndian` / `RawLittleEndian`)
the answer is just `frame_size_bytes`: per ETSI TS 102 114 V1.3.1
§5.3.1 the `FSIZE+1` field already counts on-wire container bytes
of the 16-bit-per-word stream. For the 14-bit-packed encodings
(`FourteenBitBigEndian` / `FourteenBitLittleEndian`) the same
`FSIZE+1` logical bytes are carried at 14 logical bits per 2
container bytes (one 16-bit container word carries 14 payload bits
per §3.2 / §6.1.3.1), so the span occupies
`ceil(frame_size_bytes * 8 / 14)` container words =
`2 * ceil(frame_size_bytes * 8 / 14)` container bytes. The
formula is the analytical half of round-6 docs gap #7,
transcribed verbatim from
`docs/audio/dts/dts-core-extracts.md` §3.3 (which synthesises
ETSI §5.3.1's `FSIZE` definition with the §6.1.3.1 / §6.3.x
"28-bit-word boundary" invariant). Seven new unit tests lock the
formula down: raw-equals-`frame_size_bytes`,
1024-logical→1172-container, minimum 95→110 / maximum 16384→18726
container-byte advance, strict-greater-than-raw + closed-form
`16/14` scaling upper bound, BE/LE equivalence (both raw and
14-bit pairs), the 14-bit advance is always even (the
28-bit-boundary invariant forces a two-container-word step), and a
closed-form cross-check on a spread of frame sizes. No new docs
gap; the formula's empirical half — actually walking a 14-bit
container stream through `iter_frames` — is still pending a
streaming 14↔16-bit per-frame header unpacker (the parser reads
fields from the unpacked stream, so the iterator needs that
conversion step before it can call `parse_frame_header_14bit` on
each frame slice).

**Round 185 — `RATE` → targeted bit-rate (ETSI §5.3.1 Table 5-7).**
Round 185 (2026-05-29) wires ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-7
("RATE parameter versus targeted bit-rate", transcribed in
`docs/audio/dts/dts-core-extracts.md` §1) into the header resolvers.
The new `TargetedBitRate` enum distinguishes the 25 fixed targeted
rates (`Fixed(bps)`), the open-mode code `0b11101` (`Open`), and
every reserved code (`Invalid`); `DtsFrameHeader::targeted_bit_rate()`
returns it, and `DtsFrameHeader::bit_rate_bps()` — which had returned
`None` since round 1 — now resolves the fixed codes to bits per
second (e.g. code `0b01111` → `Some(768_000)`). The mapping is
cross-validated against the existing 768 kb/s ffmpeg black-box
fixture, whose `RATE` index 15 now resolves to exactly the 768 000 bps
ffprobe reports for the same frame. Tables 5-8 (`DYNF`) / 5-9
(`TIMEF`) from the same clause are present/not-present flags already
surfaced as `dynamic_range` / `time_stamp`; their field docs now cite
the tables. One new exhaustive test walks all 32 `RATE` codes
(25 fixed + open + 6 invalid); the black-box tests assert the
768 000 bps result across the raw-BE, 14-bit-BE, and 14-bit-LE input
encodings. This closes the bitrate half of docs gap #928; the
SFREQ (sample-rate) and AMODE (channel-count) value tables remain
open (`sample_rate_hz()` / `channel_count()` still return `None`).

**Round 179 — `iter_syncs` lazy streaming iterator + `SyncWordEncoding` / `SyncMatch` accessor surface.**
Round 179 (2026-05-29) adds a streaming counterpart to the
round-151 `find_all_syncs` bulk helper plus a small accessor surface
on `SyncWordEncoding` and `SyncMatch` derived directly from the wiki
sync-sequence table (`docs/audio/dts/wiki/DTS.wiki`'s
"How to distinguish different versions" enumeration). The new
`iter_syncs(bytes) -> SyncIterator<'_>` returns an
`Iterator<Item = SyncMatch>` that walks the buffer one
`find_next_sync` hop at a time and yields matches as they appear —
same matching rules, same walk order, same `O(n)` cost as
`find_all_syncs`, but no upfront `Vec<SyncMatch>` allocation. Useful
when the caller is fine with element-by-element consumption,
wants to stop early after a `take(N)` window, or routes through
standard `Iterator` combinators (e.g.
`iter_syncs(bytes).filter(|m| m.encoding.is_raw_16bit())`). The new
`SyncWordEncoding::sync_byte_length()` reports the on-wire byte
length of the matched sync sequence (4 for raw-BE / raw-LE per the
wiki's `7F FE 80 01` / `FE 7F 01 80` rows; 6 for the two
14-bit-packed encodings per `1F FF E8 00 07 Fx` / `FF 1F 00 E8 Fx 07`);
`SyncWordEncoding::is_raw_16bit()` / `is_14bit_packed()` are
mutually-exclusive predicates that partition the enum into the
raw-vs-container distinction the wiki documents. `SyncMatch`
forwards both into `sync_byte_length()` / `sync_byte_range()` so
the common "advance the cursor past the matched sync" / "slice the
matched bytes" patterns read naturally
(`cursor = m.offset + m.sync_byte_length()` /
`&bytes[m.sync_byte_range()]`). Eleven new tests (plus one new
doc-test) lock down the byte counts against the wiki table, the
raw-vs-packed partition, the streaming-vs-bulk equivalence
(`iter_syncs(...).collect() == find_all_syncs(...)`) on a
mixed-encoding buffer, an empty-result buffer, `take(N)` window
correctness, `is_raw_16bit` filter combinator usage, and a 4 KB
pseudo-random buffer cross-check against the existing
`reference_find_all_syncs`. No new docs gap is introduced; the
existing #928 / #1055 / #1084 docs gaps remain open.

**Round 165 — `find_next_sync` first-byte gate (`O(n)` constant-factor speedup).**
Round 165 (2026-05-27) gates the multi-byte `detect_sync` call inside
`find_next_sync` behind a one-byte filter
(`is_sync_first_byte_candidate`). The four documented DTS sync
sequences (`7F FE 80 01` raw-BE, `FE 7F 01 80` raw-LE,
`1F FF E8 00 07 Fx` 14-bit-BE, `FF 1F 00 E8 Fx 07` 14-bit-LE) all
begin with distinct first bytes — `0x7F`, `0xFE`, `0x1F`, `0xFF` per
the wiki bit-table — so 252 of 256 possible payload bytes can be
rejected with a single compare-and-branch rather than the previous
4-byte raw-sync equality check + two 6-byte 14-bit container
unpacks. On uniform-random payload the inner loop visits ~98.4% of
positions with the cheap path; the walk order, returned `SyncMatch
{ offset, encoding }`, and end-of-buffer bookkeeping are
**unchanged** from round 6 — round 165 also adds eight new tests
(171 total, up from 163) including:

- a `find_next_sync_matches_pre_optimization_reference_on_candidate_dense_payload`
  harness that packs every fourth byte with a first-byte sync
  candidate but a non-sync continuation, and proves the optimised
  scanner returns the same `None` (and then the same embedded sync
  at offset 100) the pre-round-165 brute-force reference returns;
- a 4 KB pseudo-random-buffer cross-check sweeping every possible
  `start` offset and asserting per-call agreement with the
  reference;
- a `find_all_syncs_matches_reference_on_random_buffer_with_embedded_syncs`
  bulk-scan parity test that embeds one sync of each of the four
  encodings at known positions and verifies the optimised
  `find_all_syncs` recovers every (offset, encoding) pair the
  reference recovers;
- an all-`0xFF` payload stress test (every position is a first-byte
  candidate — the negative filter's degenerate case) with one real
  raw-LE sync embedded mid-buffer;
- an exhaustive 256-input check that the filter accepts exactly the
  four documented first bytes `{0x1F, 0x7F, 0xFE, 0xFF}` and
  rejects the other 252.

The downstream walkers (`iter_frames`, `iter_frames_resync`,
`find_all_syncs`) inherit the speedup transparently because they
all dispatch through `find_next_sync`. No public API surface change;
no docs gap touched (#928 / #1055 / #1084 still open).

**Round 159 — `iter_frames_resync` error-tolerant frame walker.**
Round 159 (2026-05-27) adds an error-tolerant counterpart to the
round-6 `iter_frames`: `iter_frames_resync(bytes) -> FrameIteratorResync<'_>`
walks the same raw-16-bit DTS Core stream as `iter_frames`, but when
a candidate sync turns out to be a false positive (random payload
bytes that happened to match a 4-byte sync sequence and whose
subsequent header bits fail the structural NBLKS / FSIZE bounds, or
whose declared `frame_size_bytes` overruns end-of-buffer), the
iterator yields a `ResyncEvent { offset, encoding, cause }` and
**continues scanning** from `offset + 1` instead of terminating. The
new `ResyncCause` enum documents the four discard reasons:
`StructuralBoundFailed(Error)` (NBLKS &lt; 5 or FSIZE &lt; 95 — the
classic false-positive sync signature), `HeaderEof` (sync too close
to end-of-buffer for the 13–15-byte header window),
`FrameLengthOverrunsBuffer { declared_len }` (header parses but the
declared length runs past the input), and `FourteenBitSyncSkipped`
(a 14-bit sync at the cursor; skipped rather than terminating like
the fail-fast iterator does, so a raw-16-bit stream with stray
14-bit-shaped byte sequences in payload still walks). The fail-fast
`iter_frames` from round 6 is unchanged — well-formed input walks
through both iterators identically and round 159 confirms this via a
fixture-level equivalence test (the bundled ffmpeg 5-frame fixture
yields the same five frames through both). A corrupted-header
variant of the same fixture (header byte flip in frame 2 →
NBLKS=0) demonstrates the recovery contract: the resync iterator
surfaces one `StructuralBoundFailed` event at offset 1024 and then
walks frames 3, 4, and 5 (1024 B each); the fail-fast iterator
terminates at frame 2. Useful for demuxers, stream-integrity
tooling, and forensic walkers that need to survive a corrupted
patch in the middle of a `.dts` byte stream.

**Round 151 — `find_all_syncs` bulk-scan helper + raw-LE `iter_frames` test coverage.**
Round 151 (2026-05-26) adds `find_all_syncs(bytes) -> Vec<SyncMatch>`,
the bulk-scan counterpart to the round-6 `find_next_sync`: instead of
returning the first sync at or after a cursor, it walks the entire
input buffer and returns every documented sync occurrence (all four
encodings) as a vector. Same `O(n)` cost as a `find_next_sync` loop
from `offset + 1`; the bulk helper just materialises the result for
stream-integrity tooling that needs every resync point up front
rather than walking one at a time. The round also closes a missing
coverage gap by exercising `iter_frames` against a hand-built
multi-frame raw-LE stream — the iterator was already raw-LE-capable
because `frame_size_bytes` is byte-equivalent across both raw
encodings (the wiki defines raw-LE as the 16-bit-word-swap of
raw-BE), but the previous test grid only exercised raw-BE via the
bundled ffmpeg fixture.

**Round 148 — 14-bit-packed encoder variants (all four sync encodings covered).**
Round 1 landed
the structural frame-header parser; round 2 added the two 14-bit-packed
sync encodings (`1F FF E8 00 07 Fx` BE and `FF 1F 00 E8 Fx 07` LE) via
`unpack_14bit_to_16bit` plus the dedicated `parse_frame_header_14bit`
entry point. Round 3 (2026-05-21) extended the typed header through
the 13 single-bit and small-field flags that follow RATE in the wiki
layout (downmix, dynamic-range, time-stamp, aux-data, HDCD, 3-bit
extension-audio descriptor, extension-audio coding, ASPF, 2-bit LFE
mode, predictor-history) plus the optional 16-bit `HEADER_CRC` field
that is emitted iff `crc_present` is set. Round 4 (2026-05-22) wired
the crate into `oxideav-core`'s `Decoder` trait surface behind a
default-on `registry` cargo feature, claimed the `dts` and `dtsc`
FourCC tags in the codec registry, and exposed a standalone
`probe_dts` confidence helper. Round 5 (2026-05-25) extends
`DtsFrameHeader` through the 16-bit post-CRC trailing window the
wiki snapshot enumerates after `HEADER_CRC`: `multirate_inter` (1
bit), `version` (4 bits), `copy_history` (2 bits),
`source_pcm_resolution_index` (3 bits), `front_sum` (1 bit),
`surround_sum` (1 bit), and `dialog_normalization` (4 bits). The
parser consumes these bits unconditionally — the wiki shows them
following the HEADER_CRC slot whether or not CRC was emitted —
so they are recovered for `crc_present == 0` frames as well as
`crc_present == 1` frames. Round 6 (2026-05-25) adds the
multi-frame iterator helpers built on top of the existing
single-frame parsers: `find_next_sync(bytes, start)` scans for the
next DTS sync sequence (all four documented encodings) at or after
an arbitrary offset, and `iter_frames(bytes)` walks a raw-16-bit
DTS Core byte buffer frame by frame, using each frame's
`frame_size_bytes` to advance to the next sync. A new
ffmpeg-generated 5-frame fixture
(`tests/fixtures/dts_5_frames.bin`, 5 120 bytes) exercises the
iterator end-to-end: every frame parses, the iterator handles
leading garbage via resync, the cursor advances correctly across
all five frames, and a truncated-tail variant surfaces
`Error::UnexpectedEof` at the boundary. With `--no-default-features`
the crate has no `oxideav-core` dep and surfaces only the
structural parsers plus the round-6 iterator helpers; an inline
`ci-standalone` CI job exercises that path on every push.
Round 138 (2026-05-26) surfaces the header→SUBFRAMES boundary
through three new accessors derived entirely from the wiki bit-table:
`DtsFrameHeader::header_bit_length()` (104 when `crc_present == 0`,
120 when `crc_present == 1`), `DtsFrameHeader::header_byte_length()`
(13 or 15 — both totals are exact multiples of 8), and
`FrameView::payload()` which slices off the SUBFRAMES region
(`data[header_byte_length()..]`) for downstream re-muxers and the
future subframe decoder.
Round 141 (2026-05-26) closes the parse↔encode round-trip on the
frame-sync header window: new
`encode_frame_header_be(&DtsFrameHeader) -> Result<Vec<u8>>` writes
a parsed `DtsFrameHeader` back into the on-wire bytes the wiki
bit-table prescribes. The output is exactly `header_byte_length()`
bytes long (13 or 15) and always begins with the canonical raw-BE
sync `7F FE 80 01` regardless of `sync_word_encoding`; the encoder
validates the parser's structural bounds plus per-field bit-width
bounds (a new `Error::FieldOutOfRange { field, value, max }`
variant) so a malformed `DtsFrameHeader` cannot smuggle bits into
the next field. The round-trip property
`parse(pad15(encode_frame_header_be(hdr)))` recovers `hdr` on
every field except `sync_word_encoding` (the parser tags the
output as `RawBigEndian` by construction); a real ffmpeg fixture's
13-byte header window is reproduced byte-for-byte.
Round 145 (2026-05-26) extends the encoder side with two new
primitives: `encode_frame_header_le(&DtsFrameHeader)` emits the
raw-LE on-wire header window (canonical sync `FE 7F 01 80`, always
16 bytes long — the parser's minimum raw-LE input length, i.e.
`encode_frame_header_be` zero-padded to 16 and 16-bit-word-swapped);
and `pack_16bit_to_14bit(input, order) -> (Vec<u8>, usize)` is the
inverse of the existing `unpack_14bit_to_16bit`, packing an
MSB-first 16-bit-equivalent byte stream into 14-bit-payload
containers with the wiki's "sign bit extension" rule applied to the
upper 2 bits of each container. The returned `payload_bit_count`
lets callers recover the exact pre-pack bit length when the input
does not divide evenly into 14-bit chunks. Together with the
existing `unpack_14bit_to_16bit` it completes the bidirectional
14↔16-bit container conversion the wiki snapshot prescribes; the
two encoder variants plus the 14↔16-bit primitives put all four
documented sync encodings within reach of a future
`encode_frame_header_14bit_{be,le}` round.
Round 148 (2026-05-26) closes the encoder surface across all four
documented sync encodings. Two new primitives,
`encode_frame_header_14bit_be(&DtsFrameHeader)` and
`encode_frame_header_14bit_le(&DtsFrameHeader)`, compose
`encode_frame_header_be` with `pack_16bit_to_14bit`: the raw-BE
header bytes are padded to 15 bytes (= 120 bits = the worst-case
`crc_present == 1` header window) and re-packed into nine 14-bit
containers in the requested byte order. Both encoders emit
**exactly 18 bytes** regardless of `crc_present` — matching the
parser's minimum 14-bit input length, so the
`parse_frame_header_14bit(encode_frame_header_14bit_{be|le}(hdr))`
round-trip is exact on every field except `sync_word_encoding`
(which the parser reports as the variant it detected at the
input). The 14-bit-LE output is the pairwise byte-swap of the
14-bit-BE output (each container swapped independently), matching
the wiki's `1F FF E8 00 …` (BE) vs `FF 1F 00 E8 …` (LE)
sync-prefix relationship. With these two additions the crate now
exposes a parse↔encode round-trip on the frame-sync header window
for every one of the four sync encodings the wiki snapshot
enumerates (`RawBigEndian`, `RawLittleEndian`,
`FourteenBitBigEndian`, `FourteenBitLittleEndian`).

The parser surfaces a typed `DtsFrameHeader`:

| Field                     | Source                              |
| ------------------------- | ----------------------------------- |
| `sync_word_encoding`      | first 4 bytes                       |
| `frame_type`              | FTYPE (1 bit) — termination vs normal |
| `sample_count_per_block`  | SHORT (5 bits) + 1                  |
| `crc_present`             | CRC_PRESENT (1 bit)                 |
| `blocks_per_frame`        | NBLKS (7 bits, 5..=127)             |
| `frame_size_bytes`        | FSIZE-1 (14 bits) + 1, 95..=16384   |
| `amode`                   | AMODE (6 bits)                      |
| `sfreq_index`             | SFREQ (4 bits)                      |
| `rate_index`              | RATE (5 bits)                       |
| `downmix`                 | DOWNMIX (1 bit)                     |
| `dynamic_range`           | DYNRANGE (1 bit)                    |
| `time_stamp`              | TIMSTP (1 bit)                      |
| `aux_data`                | AUXDATA (1 bit)                     |
| `hdcd`                    | HDCD (1 bit)                        |
| `ext_descr`               | EXT_DESCR (3 bits)                  |
| `ext_coding`              | EXT_CODING (1 bit)                  |
| `aspf`                    | ASPF (1 bit)                        |
| `lfe`                     | LFE (2 bits) → `LfeMode` enum       |
| `predictor_history`       | PRED_HISTORY (1 bit)                |
| `header_crc`              | `Option<u16>` — `Some` iff `crc_present` |
| `multirate_inter`         | MULTIRATE_INTER (1 bit)             |
| `version`                 | VERSION (4 bits, 0..=15)            |
| `copy_history`            | COPY_HISTORY (2 bits, 0..=3)        |
| `source_pcm_resolution_index` | PCMR (3 bits, 0..=7)            |
| `front_sum`               | FRONT_SUM (1 bit)                   |
| `surround_sum`            | SURROUND_SUM (1 bit)                |
| `dialog_normalization`    | DIALNORM (4 bits, 0..=15)           |

`DtsFrameHeader::verify_header_crc()` currently returns `None`:
the wiki snapshot names the 16-bit `HEADER_CRC` field but does
not document the polynomial, seed, or covered bit range, so
verification waits on a docs follow-up (see "Docs gaps" below).
The raw 16-bit field is still surfaced for pass-through use
cases (re-muxing, equality / hash).

A black-box test against a real `ffmpeg -c:a dca -ar 48000 -ac 2
-b:a 768k` frame is included; ffmpeg is invoked only as an
opaque generator, not consulted as source. Round 2's two companion
fixtures repacked into the 14-bit BE and LE container forms are
extended in round 3 to also check the trailing-flag and CRC
fields. All three encodings recover identical structural plus
trailing-flag fields.

Subband, QMF, Huffman, vector-quantisation, DTS-HD / EXSS / XLL /
X96 / XCH all remain out of scope.

## Multi-frame iteration (round 6)

```rust
use oxideav_dts::{iter_frames, find_next_sync};

let bytes: &[u8] = /* raw .dts stream */ &[];
for frame in iter_frames(bytes) {
    let view = frame?;
    println!(
        "frame at {} ({} B): SFREQ={} RATE={} AMODE={}",
        view.offset, view.len,
        view.header.sfreq_index, view.header.rate_index, view.header.amode,
    );
}

// Resync after lost bytes:
if let Some(m) = find_next_sync(bytes, /*start=*/ 1234) {
    // m.offset, m.encoding — proceed with `iter_frames(&bytes[m.offset..])`.
}
```

The iterator only walks raw-16-bit encodings (`RawBigEndian` /
`RawLittleEndian`) because the wiki snapshot does not enumerate
the byte-advance rule for 14-bit-packed containers; a 14-bit sync
at the iterator's current position yields
`Error::UnsupportedFourteenBit` and the iterator terminates. The
single-frame `parse_frame_header_14bit` entry point remains for
callers that have already partitioned 14-bit input into
frame-sized slices.

## Error-tolerant iteration (round 159)

```rust
use oxideav_dts::{iter_frames_resync, ResyncCause, ResyncEvent};

let bytes: &[u8] = /* possibly-corrupted raw .dts stream */ &[];
let mut recovered = 0usize;
let mut discarded = 0usize;
for step in iter_frames_resync(bytes) {
    match step {
        Ok(view) => {
            recovered += 1;
            println!("frame {} ok ({} B)", view.offset, view.len);
        }
        Err(ResyncEvent { offset, cause, .. }) => {
            discarded += 1;
            match cause {
                ResyncCause::StructuralBoundFailed(_) => {
                    eprintln!("false sync at {offset}: header bounds failed");
                }
                ResyncCause::HeaderEof => {
                    eprintln!("sync at {offset}: header truncated");
                }
                ResyncCause::FrameLengthOverrunsBuffer { declared_len } => {
                    eprintln!("frame at {offset} declares {declared_len} B but overruns");
                }
                ResyncCause::FourteenBitSyncSkipped => {
                    eprintln!("14-bit sync at {offset}: skipped");
                }
            }
        }
    }
}
```

The contract: every yielded step (whether `Ok` or `Err`) advances
the cursor; iteration ends naturally when `find_next_sync` finds no
more syncs. A well-formed stream walks identically to `iter_frames`
— round 159 verifies this against the bundled ffmpeg 5-frame
fixture. Round 159 also exercises the recovery path against a
manually-corrupted variant of the same fixture (one-byte flip in
frame 2's header → `NBLKS == 0`): the resync iterator surfaces one
`StructuralBoundFailed` event at offset 1024 and then recovers
frames 3, 4, and 5 from offsets 2048 / 3072 / 4096.

## Framework integration (round 4, default-on `registry` feature)

The default-on `registry` cargo feature pulls in `oxideav-core` and
exposes:

- `register(ctx: &mut oxideav_core::RuntimeContext)` — registers the
  DTS decoder factory plus FourCC tags `dts` and `dtsc` into the
  runtime's `CodecRegistry`.
- `make_decoder(params)` — boxed `oxideav_core::Decoder` factory.
- `DtsDecoderHandle` — the decoder handle. `send_packet` eagerly
  parses the frame header (so demuxers see structural failures —
  bad sync, NBLKS < 5, frame size < 95, truncated header — at the
  packet boundary); `receive_frame` returns
  `Error::Unsupported` because PCM output remains gated on the
  SFREQ/AMODE value tables (and the subband/QMF decode path) landing
  in `docs/` (see below). The RATE table landed in round 185.
- `probe_dts(&[u8]) -> Confidence` — standalone confidence helper:
  returns `1.0` on a valid frame header at offset 0, `0.5` on a
  truncated buffer (sync present but body short), `0.0` on
  unrelated input.

The `oxideav_core::register!("dts", register)` macro is invoked at
crate root so `oxideav-meta`'s build-time discovery picks the crate
up without manual wiring on the consumer side.

With `--no-default-features` the registry module is excluded, the
`oxideav-core` dep is dropped from the dep tree, and only the
structural parsers (`parse_frame_header`,
`parse_frame_header_14bit`, `unpack_14bit_to_16bit`) plus the
crate-local `Error` / `Result` types remain.

## Docs gaps (filed for the docs collaborator)

`docs/audio/dts/wiki/DTS.wiki` documents the frame-header bit
layout but only says *"See table below"* for the value tables of
three fields. The wiki page itself was mirrored as-is, so some of
those tables are not in `docs/`:

1. **Sample-frequency index → Hz**: SFREQ is a 4-bit code; the
   mapping table (16 entries) is missing. `DtsFrameHeader::sample_rate_hz()`
   returns `None` until it lands.
2. **Transmission-bitrate index → bps**: *Resolved in round 185.*
   ETSI TS 102 114 §5.3.1 Table 5-7 (transcribed in
   `docs/audio/dts/dts-core-extracts.md` §1) gives the 25 fixed
   targeted rates plus the open (`0b11101`) and invalid codes.
   `DtsFrameHeader::bit_rate_bps()` now resolves the fixed codes (e.g.
   code `0b01111` → `Some(768_000)`, cross-validated against the
   768 kb/s ffmpeg black-box fixture); `DtsFrameHeader::targeted_bit_rate()`
   preserves the open/invalid distinction via `TargetedBitRate`.
   (Tables 5-8 `DYNF` / 5-9 `TIMEF` from the same clause are
   present/not-present flags already surfaced as `dynamic_range` /
   `time_stamp`.)
3. **AMODE → channel-count / layout**: AMODE 0..=15 is documented
   as "standard layouts" but the layout descriptions (mono, dual-
   mono, L+R, L+R+C, …) are not in the snapshot.
   `DtsFrameHeader::channel_count()` returns `None`.

A clean-room recipe for filling the remaining SFREQ / AMODE gaps:
cite the ETSI TS 102 114 §5.3.1 sample-frequency and channel-mode
value tables verbatim (the spec is staged at
`docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`) and
transcribe them into `docs/audio/dts/dts-core-extracts.md` alongside
the Table 5-7 / 5-8 / 5-9 entries already there.

### Round-3 docs gaps

4. **`HEADER_CRC` polynomial / coverage**: the wiki snapshot lists
   the 16-bit field as "`Header CRC | if CRC present above is
   set`" without spelling out the generator polynomial, the seed
   value, the byte / bit endianness, or the exact bit range the
   CRC covers. `DtsFrameHeader::verify_header_crc()` therefore
   returns `None` even when the raw field is present. The ETSI
   TS 102 114 main spec is the same external clean-room source
   recommended for gaps 1–3 above — it documents the CRC
   contract in §5.3.

### Round-5 docs gaps

5. **PCMR (source-PCM-resolution) index → bits-per-sample**: the
   wiki snapshot enumerates the field as 3 bits ("Source PCM
   resolution") without listing the eight code values. ETSI TS
   102 114 §5.3.1 documents this as (typically) a 3-bit width
   plus a top "ES" flag mapping to 16 / 20 / 24 bps with
   optional encoder-side ES indication. `DtsFrameHeader::source_pcm_bits_per_sample`
   returns `None` until the table lands.
6. **DIALNORM (dialog-normalization) code → dB**: the wiki
   describes the 4-bit field as "dB of recovery" without
   enumerating the code → dB mapping (the spec also documents a
   version-dependent sign convention).
   `DtsFrameHeader::dialog_normalization_db` returns `None`
   until the table lands.

### Round-195 docs gaps

8. **SA129..SE129 full 129-entry codebooks**: Table 5-24 names the
   five scale-factor codebooks the SHUFF=0..4 entries select but the
   staged Annex D revision (V1.3.1, 2011-08) does not transcribe them
   under those `SA129..SE129` names. Round 195 routes SHUFF=0..4
   through the staged §D.5.3 / §D.5.4 small-Huffman codebooks
   (A5/B5/C5 for SHUFF=0..2, A7/B7 for SHUFF=3..4), which match the
   ±2 (5-level) and ±3 (7-level) difference-symbol ranges Table 5-28
   expects of difference-encoded SCALES. Confirming the full
   129-level mapping (or transcribing the explicit SA129..SE129
   tables from a different revision of TS 102 114) is a
   docs-completeness follow-up. For now,
   `ScalesCodebook::is_huffman_encoded()` partitions the SHUFF=0..4
   set as the difference-encoded path per §5.4.1's
   `if (nQSelect < 5)  nScaleSum += nScale;`.

### Round-6 docs gaps

7. **14-bit container-byte advance rule**: *Resolved in round 192.*
   The analytical half landed in round 189 as
   `DtsFrameHeader::frame_size_container_bytes(SyncWordEncoding)`
   (`frame_size_bytes` for the raw encodings per ETSI §5.3.1's
   `FSIZE+1` byte definition; `2 * ceil(frame_size_bytes * 8 / 14)`
   for the 14-bit encodings per §3.3 of `dts-core-extracts.md`,
   combining §5.3.1's `FSIZE` rule with the §6.1.3.1 / §6.3.x
   28-bit-word-boundary invariant). The empirical half landed in
   round 192 as `iter_frames_14bit(bytes) -> FrameIterator14<'_>`:
   a multi-frame walker that operates directly on 14-bit-packed
   container bytes, calling `parse_frame_header_14bit` at each sync
   to recover the header (the parser internally unpacks just enough
   containers to read the 13/15-byte unpacked header window) and
   advancing by `frame_size_container_bytes(encoding)` container
   bytes per step. The fail-fast `iter_frames` from round 6 still
   refuses 14-bit syncs with `Error::UnsupportedFourteenBit`
   because raw streams and container streams live in distinct
   domains; the symmetric reciprocal — raw 16-bit syncs at the
   cursor of `iter_frames_14bit` — surfaces the new
   `Error::UnsupportedRaw16Bit` variant. Callers route by encoding
   up-front.

## License

MIT — see [LICENSE](./LICENSE).
