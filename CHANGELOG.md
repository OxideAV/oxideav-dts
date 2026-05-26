# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate adheres
to [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Round 159 (2026-05-27) — `iter_frames_resync` error-tolerant
  multi-frame walker. The fail-fast `iter_frames` from round 6
  terminates at the first parse failure; the new
  `FrameIteratorResync` / `iter_frames_resync(bytes)` instead treat
  parse failures at candidate sync positions as false-positive
  syncs and continue scanning from `offset + 1`, recovering real
  frames that follow corrupted-header patches in the middle of a
  `.dts` byte stream.
  - New public types `FrameIteratorResync<'a>` (the iterator),
    `ResyncEvent { offset, encoding, cause }` (Err item), and
    `ResyncCause` (the 4-variant cause enum:
    `StructuralBoundFailed(Error)`, `HeaderEof`,
    `FrameLengthOverrunsBuffer { declared_len }`,
    `FourteenBitSyncSkipped`). All re-exported at the crate root.
  - `iter_frames_resync(bytes)` convenience constructor, mirroring
    the existing `iter_frames(bytes)`.
  - The new iterator does NOT depend on the `oxideav-core`
    integration — both `iter_frames` and `iter_frames_resync` are
    available in the `--no-default-features` build.
  - The well-formed-input contract: on a clean stream the resync
    iterator's yields are byte-for-byte identical to the fail-fast
    iterator's (every step is `Ok`; frame views match). Round 159
    asserts this against the bundled ffmpeg 5-frame fixture.
  - Eleven new unit tests in `src/iter.rs` cover: clean-stream
    parity with `iter_frames`, false-positive sync skipping with
    real-frame recovery, frame-length overrun event surfacing,
    14-bit sync skipping (so a raw stream with stray 14-bit-shaped
    payload patches still walks), cursor advance (one byte on
    event, frame_size_bytes on Ok), empty buffer, no-sync buffer,
    multiple consecutive false positives all reported, truncated
    tail surfaces overrun event then ends, truncated header
    surfaces `HeaderEof`, and `iter_frames_resync` ≡
    `FrameIteratorResync::new`.
  - Two new integration tests in `tests/multi_frame_iter.rs` cover
    the resync iterator against the bundled ffmpeg fixture: a
    clean-fixture equivalence check (resync walks identically to
    fail-fast), and a corrupted-fixture recovery check (frame-2
    header byte flip → resync surfaces one `StructuralBoundFailed`
    event and recovers frames 3, 4, and 5).
- Round 151 (2026-05-26) — `find_all_syncs` bulk-scan helper plus
  raw-LE `iter_frames` test coverage.
  - `find_all_syncs(bytes: &[u8]) -> Vec<SyncMatch>` is the bulk
    counterpart to the round-6 `find_next_sync`: it scans the entire
    input buffer and returns every documented sync occurrence (all
    four encodings) as a vector. Same `O(n)` cost as a
    `find_next_sync` loop from `offset + 1`; the bulk helper just
    materialises the result for stream-integrity tooling that needs
    every resync point up front. The four documented sync prefixes
    start with mutually-distinct first bytes (`7F` / `FE` / `1F` /
    `FF`), so adjacent (non-overlapping) sync occurrences are both
    reported. Includes a doctest plus seven unit tests covering:
    empty buffer, no-sync buffer, single sync, mixed raw-BE / raw-LE,
    all four encodings, consecutive back-to-back syncs, garbage-
    interspersed positions, and parity with the explicit
    `find_next_sync` loop reference.
  - Three new unit tests for `iter_frames` against a hand-built
    multi-frame raw-LE byte stream (constructed by pairwise
    word-swap of a two-frame raw-BE buffer to match the wiki's
    raw-LE-is-word-swapped-raw-BE definition): the walker
    correctly identifies both frames as
    `SyncWordEncoding::RawLittleEndian`, advances by
    `frame_size_bytes` (which the wiki defines as byte length of the
    unpacked raw-16-bit stream — byte-equivalent across both raw
    encodings), and remains robust to leading garbage / resync.
    Closes a coverage hole because the previous test grid only
    exercised the raw-BE path via the bundled ffmpeg fixture.
- Round 148 (2026-05-26) — 14-bit-packed encoder variants that close
  the parse↔encode round-trip across all four documented sync
  encodings. Two new primitives:
  - `encode_frame_header_14bit_be(&DtsFrameHeader) -> Result<Vec<u8>>`
    composes `encode_frame_header_be` with the round-145
    `pack_16bit_to_14bit` primitive: the raw-BE header bytes are
    padded to 15 bytes (= 120 bits = the worst-case `crc_present == 1`
    header window) and re-packed into nine 14-bit-BE containers. The
    output is always exactly 18 bytes (regardless of `crc_present`)
    and always begins with the wiki-documented 14-bit-BE sync prefix
    `1F FF E8 00 …`.
  - `encode_frame_header_14bit_le(&DtsFrameHeader)` is the same
    composition with `FourteenBitByteOrder::LittleEndian`; the output
    is the pairwise byte-swap of the 14-bit-BE output (each container
    swapped independently) and begins with `FF 1F 00 E8 …`.
  - Both encoders inherit the bit-width and structural-bound checks
    from `encode_frame_header_be`
    (`BlockCountOutOfRange` / `FrameSizeOutOfRange` /
    `FieldOutOfRange{header_crc}`).
  - Fourteen new unit tests covering: fixed 18-byte output for both
    `crc_present` states (BE + LE), wiki sync-prefix reproduction
    (BE + LE), pairwise-byte-swap equivalence between the BE and LE
    outputs, parse↔encode round-trip through `parse_frame_header_14bit`
    with and without CRC (BE + LE), NBLKS / FSIZE / CRC-payload bound
    rejection inheritance, an exhaustive 24-case grid
    ({LFE × CRC × {NBLKS, FSIZE}}) covering both variants and
    confirming cross-equivalence on every case, and a cross-check
    that unpacking the 14-bit-BE encoder output through
    `unpack_14bit_to_16bit` recovers the raw-BE header prefix
    byte-for-byte.
- Round 145 (2026-05-26) — raw-LE encoder + bidirectional 14↔16-bit
  container pack/unpack. Two new primitives:
  - `encode_frame_header_le(&DtsFrameHeader) -> Result<Vec<u8>>`
    serialises a parsed header into the raw-LE on-wire byte form
    (canonical sync `FE 7F 01 80`); the output is exactly 16 bytes
    long regardless of `crc_present` (the parser's raw-LE branch
    requires a 16-byte word-swap window). Implemented as
    `encode_frame_header_be` + zero-pad to 16 + 16-bit-word-swap.
    The `parse_frame_header(encode_frame_header_le(hdr))` round-trip
    recovers `hdr` on every field; the parser reports
    `SyncWordEncoding::RawLittleEndian` because that's the sync it
    detected at the input.
  - `pack_16bit_to_14bit(input, order) -> (Vec<u8>, usize)` is the
    inverse of `unpack_14bit_to_16bit`. The input is read as an
    MSB-first bit stream; successive 14-bit chunks are written into
    the lower 14 bits of 16-bit containers, with the upper 2 bits
    filled by a sign-extension of payload bit 13 (per the wiki's
    "sign bit extension" rule). The returned `payload_bit_count`
    lets callers recover the exact pre-pack bit length when the
    input does not divide evenly into 14-bit chunks. Feeding the
    32-bit raw-BE syncword `7F FE 80 01` reproduces the wiki's first
    two 14-bit sync containers byte-for-byte (`1F FF E8 00` BE and
    `FF 1F 00 E8` LE) and the third container's top 12 bits
    (`0x07F`); the lower 4 bits of the third container hold 4 bits
    of the following field rather than the syncword, matching the
    wiki's `07 Fx` notation.
  - Seventeen new unit tests covering: `encode_frame_header_le`
    canonical sync emission, fixed-16-byte output length for both
    `crc_present` states, equivalence with manual
    `swap16(BE.padded_to_16())`, round-trip through the parser with
    and without CRC, NBLKS / CRC-payload bound rejection inheritance,
    an exhaustive {LFE × CRC × {NBLKS, FSIZE}} grid (24 cases), and
    byte-swap reproduction of the real ffmpeg fixture; plus
    `pack_16bit_to_14bit` wiki-sync-prefix reproduction (BE + LE),
    sync-pattern-with-following-bits reproduction of the wiki's
    `0x07 F<x>` third container, round-trip across multiple input
    lengths (BE + LE), the byte-swap equivalence of BE vs LE pack
    output, empty-input contract, and the sign-extension contract
    for positive and negative payloads.
- Round 141 (2026-05-26) — `encode_frame_header_be(&DtsFrameHeader)
  -> Result<Vec<u8>>` serialises a parsed [`DtsFrameHeader`] back
  into the raw-BE on-wire bytes of the frame-sync header window
  (104 or 120 bits, i.e. 13 or 15 bytes depending on
  `crc_present`). The encoder is the inverse of
  `parse_frame_header` against the wiki bit-table — every field
  round-trips bit-exact, and the canonical raw-BE sync
  `7F FE 80 01` is always emitted even if the source header was
  parsed from the raw-LE / 14-bit-BE / 14-bit-LE encoding (the
  caller is expected to repack post-process if a non-raw-BE
  on-wire form is needed). The encoder validates the same
  structural bounds as the parser (`BlockCountOutOfRange`,
  `FrameSizeOutOfRange`) plus per-field bit-width bounds via a
  new `Error::FieldOutOfRange { field, value, max }` variant
  covering AMODE > 63, SFREQ > 15, RATE > 31, EXT_DESCR > 7,
  VERSION > 15, COPY_HISTORY > 3, PCMR > 7, DIALNORM > 15,
  `sample_count_per_block` > 32, and a `header_crc.is_some()`
  vs `crc_present` mismatch (rejected so a silent drop or
  garbage-emit bug cannot defeat the round-trip property).
- Twelve new unit tests covering: non-trivial round-trip with CRC,
  minimal 13-byte termination-frame round-trip without CRC, every
  field-bounds rejection variant, raw-LE input normalised to
  raw-BE output (every field preserved except
  `sync_word_encoding`), an exhaustive grid over the four LFE
  codes × two CRC states × three {NBLKS, FSIZE} pairs (24 cases),
  and a byte-for-byte equality check against the real ffmpeg
  fixture's 13-byte header window
  (`encode_frame_header_be(parse(b))[..] == b[..13]`).
- Round 138 (2026-05-26) — header → SUBFRAMES boundary accessors.
  `DtsFrameHeader::header_bit_length()` returns the total bit-count
  the frame-sync header window occupies (sync + base + trailing +
  optional HEADER_CRC + post-CRC). The value is fully derived from
  the wiki bit-table in `docs/audio/dts/wiki/DTS.wiki`: 32 + 43 + 13
  + 16 + (16 iff `crc_present`) = 104 bits when CRC is absent, 120
  bits when CRC is present. Both totals are exact multiples of 8 by
  construction, so the corresponding
  `DtsFrameHeader::header_byte_length()` is always 13 or 15 and the
  SUBFRAMES region (the wiki's `'''TODO'''` cell) starts on a byte
  boundary. `FrameView::payload()` returns
  `&data[header.header_byte_length()..]` so downstream re-muxers,
  payload-CRC validators, and the future subframe decoder can carve
  out the SUBFRAMES region directly without recomputing the header
  boundary.
- Eight new tests covering the boundary accessor: 104-bit return
  when `crc_present == 0`, 120-bit return when `crc_present == 1`,
  manual wiki-table sum equivalence, exhaustive byte-alignment over
  a grid of structural-field combinations, the 14-bit-packed entry
  point agrees with the raw-BE entry point on the bit-length value,
  and three `FrameView::payload()` integration cases (two synthetic
  95-byte termination frames with crc absent / crc present, and the
  real ffmpeg-generated 5-frame fixture).
- Round 6 (2026-05-25) — multi-frame iterator + resync helper.
  New `iter` module exposes `find_next_sync(bytes, start) -> Option<SyncMatch>`
  and `iter_frames(bytes) -> FrameIterator<'_>` (plus the supporting
  `FrameView<'_>` / `SyncMatch` types) on top of the existing
  single-frame parsers. `find_next_sync` scans for any of the four
  documented DTS sync sequences (raw 16-bit BE / LE, 14-bit packed
  BE / LE) at or after an arbitrary offset, returning the offset
  and matched `SyncWordEncoding`. `iter_frames` walks a raw-16-bit
  DTS Core byte stream frame by frame, using each frame's
  `DtsFrameHeader::frame_size_bytes` to advance to the next sync;
  it tolerates leading garbage by resyncing through
  `find_next_sync`, surfaces parse failures as the next item's
  `Err`, and terminates cleanly after the last frame. The
  iterator refuses 14-bit container streams (yields
  `Error::UnsupportedFourteenBit` and terminates) because the
  container-byte advance rule for 14-bit-packed frames is not
  enumerated in the wiki snapshot (filed as round-6 docs gap #7
  in `README.md`).
- New bundled fixture `tests/fixtures/dts_5_frames.bin` (5 120
  bytes, 5 back-to-back DTS frames at 1 024 B each) generated as
  `ffmpeg -f lavfi -i "sine=frequency=440:duration=0.05" -ac 2
  -ar 48000 -c:a dca -strict experimental -b:a 768k -f dts ...`.
  Used by the new `tests/multi_frame_iter.rs` integration test
  (`include_bytes!` from the fixture path) to exercise the
  iterator end-to-end.
- Seven new tests in `tests/multi_frame_iter.rs` covering:
  iteration over all five fixture frames with per-frame field
  assertions, fixture-size sanity check, `find_next_sync`
  enumeration of every sync offset in the fixture, iteration
  through a stream with 13 bytes of leading garbage,
  iterator-vs-direct `parse_frame_header` equivalence at each
  offset, clean termination after the last frame, and a
  truncated-tail variant that surfaces `Error::UnexpectedEof` at
  the boundary.
- Nine new unit tests in `src/iter.rs` covering `find_next_sync`:
  sync at offset zero, sync after leading garbage, every documented
  sync encoding (raw BE / raw LE / 14-bit BE / 14-bit LE),
  `start` honoured past a prior sync, `None` when no sync exists,
  `None` when `start >= bytes.len()`, and `None` when only a
  partial sync sits at the buffer tail.
- Round 5 (2026-05-25) — 16-bit post-CRC trailing window surfaced
  through `DtsFrameHeader`. After the optional 16-bit `HEADER_CRC`
  slot (or after the predictor-history bit when `crc_present == 0`),
  the parser now consumes seven additional fields the wiki
  snapshot enumerates: `multirate_inter` (1 bit), `version` (4
  bits, raw 0..=15), `copy_history` (2 bits, raw 0..=3),
  `source_pcm_resolution_index` (3 bits, raw 0..=7), `front_sum`
  (1 bit), `surround_sum` (1 bit), and `dialog_normalization` (4
  bits, raw 0..=15). The window is consumed unconditionally
  regardless of `crc_present` because the wiki lists it after
  the HEADER_CRC slot in both code paths. Two new resolver
  stubs (`DtsFrameHeader::source_pcm_bits_per_sample` and
  `DtsFrameHeader::dialog_normalization_db`) return `None`
  pending the index → value tables landing in `docs/` (filed as
  round-5 docs gaps #5 and #6 in `README.md`).
- Twelve new unit tests covering: full post-CRC window
  decomposition for a non-trivial bit pattern (`0xD2EC`),
  all-zero and all-one post-CRC windows, exhaustive round-trip
  for every PCMR / DIALNORM / VERSION / COPY_HISTORY code, the
  `crc_present == 0` vs `crc_present == 1` equivalence of
  post-CRC sub-fields, and updated assertions on every
  pre-existing parser test (raw-BE, raw-LE, 14-bit-BE,
  14-bit-LE, value-resolver, NBLKS-bounds, FSIZE-bounds,
  short-buffer EOF, trailing-bit edge cases) to also verify the
  new fields.
- Black-box ffmpeg fixture asserts (raw-BE + both 14-bit
  variants + cross-encoding equivalence) extended to verify the
  post-CRC fields recovered from the real `ffmpeg -c:a dca`
  frame: `multirate_inter == false`, `version == 7`,
  `copy_history == 0`, `source_pcm_resolution_index == 0`,
  `front_sum == false`, `surround_sum == false`,
  `dialog_normalization == 0`. The same values must come out
  through all three sync encodings. Registry's
  `send_packet_eagerly_parses_header` test additionally checks
  the cached header carries the post-CRC fields after the
  decoder handle's `send_packet` call.
- Round 4 (2026-05-22) — `oxideav-core` framework integration. A
  new default-on `registry` cargo feature gates the
  `oxideav-core` dep, the `Decoder` trait impl, and the
  `oxideav_core::register!("dts", register)` macro invocation.
  With the feature off, the crate retains the standalone
  `parse_frame_header` / `parse_frame_header_14bit` /
  `unpack_14bit_to_16bit` APIs plus the crate-local `Error` /
  `Result` types and pulls no `oxideav-core` dep.
- `make_decoder(params) -> Box<dyn Decoder>` factory and the
  `DtsDecoderHandle` it returns. `Decoder::send_packet` parses
  the frame header eagerly through `detect_sync` and routes to
  `parse_frame_header` (raw 16-bit syncs) or
  `parse_frame_header_14bit` (14-bit packed syncs), surfacing
  structural failures (`NoSync`, `BlockCountOutOfRange`,
  `FrameSizeOutOfRange`) as `Error::InvalidData` and short
  buffers as `Error::NeedMore`. `Decoder::receive_frame` returns
  `Error::Unsupported` because PCM output is gated on the
  SFREQ/RATE/AMODE value tables landing in `docs/` (see README
  docs gaps #1-#3). `Decoder::reset` clears the cached header.
- `register_codecs(reg)` / `register(ctx)` install a `CodecInfo`
  for `CodecId::new("dts")` carrying the FourCC tags
  `CodecTag::fourcc(b"dts ")` and `CodecTag::fourcc(b"dtsc")` so
  the codec resolver routes both QuickTime sample-entry types to
  the DTS decoder factory.
- `probe_dts(&[u8]) -> Confidence` — standalone confidence helper:
  `1.0` on a valid frame at offset 0, `0.5` on a truncated buffer
  (sync detected but body short), `0.0` on unrelated input. The
  registry's per-codec probe function (`probe_dts_tag`) wraps
  this for the `ProbeContext`-driven path: when the demuxer
  supplies a packet sample it forwards to `probe_dts`; when not,
  it returns `1.0` so the FourCC claim is treated as unambiguous.
- Inline `ci-standalone` job in `.github/workflows/ci.yml` running
  `cargo build --no-default-features --lib` and
  `cargo test --no-default-features --lib` on every push, beside
  the existing `OxideAV/.github` reusable-workflow `ci` job that
  exercises the default-feature (registry) path.
- 14 new unit tests in `src/registry.rs` covering: `probe_dts`
  return-value bands for valid / truncated / invalid input, FourCC
  tag resolution for both `dts` and `dtsc`, the eager
  header-parse path on `send_packet`, `Error::Unsupported` on
  `receive_frame` after a parsed header, `Error::NeedMore` and
  `Error::Eof` boundary cases, and `reset` clearing cached state.
- Round 3 (2026-05-21) — trailing-13-bit field + optional
  16-bit header-CRC field surfaced through `DtsFrameHeader`.
  After RATE the parser now consumes (in MSB-first order, per
  `docs/audio/dts/wiki/DTS.wiki`): `downmix` (1 bit),
  `dynamic_range` (1 bit), `time_stamp` (1 bit),
  `aux_data` (1 bit), `hdcd` (1 bit), `ext_descr` (3 bits),
  `ext_coding` (1 bit), `aspf` (1 bit), `lfe` (2-bit `LfeMode`
  enum: `None | Mode1 | Mode2 | Mode3`), and `predictor_history`
  (1 bit). When `crc_present` is set, the trailing 16-bit
  `HEADER_CRC` field is captured into `header_crc: Option<u16>`.
  `DtsFrameHeader::verify_header_crc()` returns `None` (polynomial
  undocumented; see README docs gap #4). The black-box ffmpeg
  fixture's new-field assertions confirm `LfeMode::None`,
  `predictor_history == true`, `header_crc == None`, and every
  other trailing-flag false for the captured frame; the same
  values are observed through the 14-bit BE and LE repacked
  fixtures, so all three documented sync encodings now agree on
  the full 56-bit header window plus optional CRC.
- New `LfeMode` enum re-exported from the crate root; `code()`
  and `is_present()` accessors.
- Twelve new unit tests covering: all four LFE codes, CRC-field
  present / absent paths, all-zero and all-one trailing windows,
  and round-3 fields equivalence across raw-BE / raw-LE /
  14-bit-BE / 14-bit-LE encodings.
- Round 2 (2026-05-21) — 14-bit sync unpacking. New
  `unpack14` module exports `unpack_14bit_to_16bit` plus
  `FourteenBitByteOrder` for the two documented 14-bit packings
  (`1F FF E8 00 07 Fx` BE and `FF 1F 00 E8 Fx 07` LE). The
  unpacker masks each 16-bit container to its lower 14 bits and
  concatenates payloads MSB-first into the raw-BE byte stream
  the round-1 parser already understands. New
  `parse_frame_header_14bit` entry point accepts a 14-bit-packed
  buffer directly; the round-1 `parse_frame_header` continues to
  reject 14-bit inputs with `Error::UnsupportedFourteenBit` so
  the two entry points have disjoint accepted-input sets. Three
  additional black-box fixtures (the round-1 ffmpeg frame
  repacked into BE-14 and LE-14, plus an explicit round-trip
  through `unpack_14bit_to_16bit`) confirm structural-field
  equivalence across all four documented sync encodings.
- `detect_sync` widened for 14-bit variants: matches on the
  lower-14-bit payloads of the first three containers
  (`0x1FFF`, `0x2800`, top-4-of-`0x07F?`) rather than the
  literal wiki byte sequence. The previous narrow match
  incidentally only accepted frames whose
  FTYPE/deficit/CRC/NBLKS_high bits in container 2 happened to
  match the wiki's chosen example; the wider check accepts all
  syntactically valid 14-bit DTS frames.
- Round 1 (2026-05-21) — structural frame-sync header parser per
  ETSI TS 102 114 §5.3 (via the mirrored
  `docs/audio/dts/wiki/DTS.wiki` snapshot). Exports
  `parse_frame_header`, `DtsFrameHeader`, `SyncWordEncoding`,
  `FrameType`, and `Error`. Handles `RawBigEndian` and
  `RawLittleEndian` 16-bit sync sequences; detects but does not
  yet unpack the 14-bit variants.
- `bitreader` module: minimal MSB-first bit reader used by the
  header parser.
- Black-box integration test against a real DTS frame produced by
  `ffmpeg -c:a dca -ar 48000 -ac 2 -b:a 768k` (ffmpeg invoked as an
  opaque generator only).

### Docs gaps (filed in `README.md`)

- SFREQ → Hz, RATE → bps, AMODE → channel-layout tables are not in
  `docs/audio/dts/`. The corresponding `DtsFrameHeader::sample_rate_hz` /
  `bit_rate_bps` / `channel_count` resolvers return `None` until
  the tables are mirrored from ETSI TS 102 114 §5.3.
- Header-CRC polynomial / coverage / seed / endianness: the wiki
  snapshot lists the 16-bit field but does not specify its CRC
  contract. `DtsFrameHeader::verify_header_crc()` returns `None`
  until the contract lands in `docs/`. Filed in `README.md` as
  round-3 gap #4.

### Erased

- Prior master history was force-erased on **2026-05-18** under
  Hat-3 cold enforcement of the workspace clean-room policy
  (`docs/IMPLEMENTOR_ROUND.md`).

### Reset

- Crate reduced to a minimal `oxideav_core::register!` stub. Every
  public API returns `Error::NotImplemented`. The crates.io version
  (`0.0.1`) is preserved on the new master to avoid breaking
  downstream version pins; the published versions on crates.io will
  be yanked by the maintainer.

### Next

- Clean-room re-implementation against the published DTS
  specifications in a future round.
