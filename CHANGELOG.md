# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate adheres
to [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
