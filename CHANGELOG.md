# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate adheres
to [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
