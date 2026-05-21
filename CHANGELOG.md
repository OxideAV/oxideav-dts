# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate adheres
to [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
