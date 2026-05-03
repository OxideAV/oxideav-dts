# Changelog

All notable changes to `oxideav-dts` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/)
and the project adheres to [Semantic Versioning](https://semver.org/).

## [0.0.1] — 2026-05-02

Initial scaffold (round 1).

### Added

- DTS Core sync-word + frame-header parser (BE 16-bit packed,
  `0x7FFE_8001`).
- AMODE / SFREQ / RATE / PCMR / EXSS-SFREQ codebooks; scale-factor
  6-bit + 7-bit absolute LUTs; ABITS quant-step LUTs (lossy +
  lossless); bit-allocation / quant-index size class metadata.
- `BitReader` (MSB-first, byte-straddling), canonical Huffman builder
  for the bit-allocation 12-entry codebooks (BHUFF), the
  transition-mode codebooks (THUFF), and the quantization-index
  codebooks at sizes 3..13.
- High-frequency VQ codebook lookup (entries 0..15 verbatim from
  `dts-vq-codebook.md` §3, remainder zero-filled).
- 32-band PQF synthesis with a clean-room Kaiser-windowed-sinc
  prototype (round-1 approximation; bit-exact 512-tap prototype is on
  the round-2 backlog).
- `oxideav_core::Decoder` impl (`make_decoder`) handling the
  encoder-fast-path coding (BHUFF=7, SHUFF=6, no transients, no
  joint, no prediction).
- Integration test that round-trips a 440-Hz sine through `ffmpeg
  -c:a dca` + this decoder.

### Backlog

See `README.md` for the round-2+ feature list (full PQF prototype
table, full VQ codebook, BHUFF/SHUFF Huffman codebooks, joint
coding, ADPCM prediction, LFE, EXSS, XLL, XCH/XXCH/X96/XBR/LBR,
14-bit packed core).
