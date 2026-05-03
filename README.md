# oxideav-dts

Pure-Rust **DTS Coherent Acoustics** (also known as DCA, Digital
Theater Systems, "DTS Core") audio decoder for the
[oxideav](https://github.com/OxideAV) framework.

## Status

Round-1 scaffold (the DTS Core profile, 16-bit, 1.5 Mbit/s typical
DVD-Video / Blu-ray rate). Decodes the original 1995-vintage
backwards-compatible Core layer — the foundation every DTS-HD frame
also carries.

| Layer / feature                                      | Round-1 |
|------------------------------------------------------|:-------:|
| Sync detection (`0x7FFE_8001` BE 16-bit)             |    Y    |
| Frame header parse (104/120 bits)                    |    Y    |
| AMODE 0..15 (mono → 5.1 + matrix layouts)            |    Y    |
| Sample rates 8 kHz .. 192 kHz (table-driven)         |    Y    |
| Bit rates 32 kbps .. 3.84 Mbps + lossless sentinel   |    Y    |
| Audio coding header (subframes / SUBS / SUBVQ / …)   |    Y    |
| Bit-allocated subbands (BHUFF 5/6/7 = direct-width)  |    Y    |
| Direct 6/7-bit absolute scale factors (SHUFF 5/6)    |    Y    |
| 32-band polyphase synthesis (PQF) — synthesised proto|  Approx |
| VQ codebook lookup (16 of 1024 entries embedded)     |  Approx |
| BHUFF Huffman codebooks 0..4                         |    -    |
| SHUFF Huffman codebooks 0..4                         |    -    |
| THUFF transition modes (full per-sample TM resolve)  |    -    |
| Joint-intensity coding (JCH > 0)                     |    -    |
| ADPCM prediction VQ (4096-entry codebook)            |    -    |
| LFE channel (8-bit ADPCM + 64×/128× FIR)             |    -    |
| Auxiliary data block + dynamic downmix               |    -    |
| 14-bit packed core / LE byte-order variants          |    -    |
| Encoder                                              |    -    |

## Backlog (round 2+)

* **PQF prototype window** — round-1 ships a synthesised
  Kaiser-windowed-sinc prototype (compatible shape, ~96 dB
  stop-band) so PSNR sits ~30-40 dB on bit-allocated content.
  Bit-exact reconstruction needs the full 512-tap
  `ff_dca_fir_32bands_*` tables from
  `docs/audio/dts/data/dts-pqf-window.md`; only the first 32 taps
  are inline today.
* **VQ codebook (1008 of 1024 entries)** — the inline sidecar
  currently lists only entries 0..15 verbatim. The remaining 1008
  are documented as `dcadata.c` lines 4240..6290 but cannot be
  copied here under the workspace's clean-room policy. PSNR on
  high-frequency-rich material (cymbals / consonants) is throttled
  by this gap until docs land the full table.
* **Huffman codebooks** — round-1 supports the encoder fast-path
  (BHUFF/SHUFF = direct fixed-width). The 60+ Huffman codebooks for
  bit-allocation, transition mode, scale factor, and quantization
  index are defined in `dts-huffman-tables.md` (sizes 3..13 inline,
  sizes 17/25/33/65/129 deferred) and will land progressively.
* **Adaptive predictor** — 4096-entry × 4-coef ADPCM codebook
  (`dts-pqf-window.md` §7) is documented at first 4 entries only;
  full table needed before `prediction_mode` bits can be honoured.
* **LFE channel** — separate 8-bit ADPCM path with 64-/128-tap
  upsamplers; tables in `dts-pqf-window.md` §6 (truncated). LFE is
  required for 5.1 → produce a silent LFE today.
* **EXSS framework** — extension-substream container
  (`dts-trace-reverse-engineering.md` §4). Required for **any**
  DTS-HD MA / HRA stream regardless of XLL/XBR.
* **XLL** — DTS-HD Master Audio lossless residual; full chset
  hierarchy + Rice/hybrid-Rice entropy + LPC reconstruction
  (`dts-xll-tables.md` partly drafted, scaffold staged for round
  2+).
* **XCH / XXCH** — 6.1 / 7.1 back-surround additive channels.
* **X96** — 96-kHz core extension with cross-band assembly.
* **XBR** — extended-bit-rate residual companion.
* **LBR / DTS Express** — secondary-audio low-bit-rate codec
  (`dts-lbr-tables.md` partly drafted, blocked on a secondary-audio
  fixture sample).
* **14-bit packed core / LE byte-swap variants** — for CD-DA /
  S/PDIF interop; sync-word table already has the constants.

## Provenance

This crate is a **clean-room** implementation against the public
ETSI TS 102 114 standard. The reference material lives entirely in
`docs/audio/dts/`:

* `docs/audio/dts/dts-trace-reverse-engineering.md` — bitstream
  structure + frame-level walk-through.
* `docs/audio/dts/data/dts-core-tables.md` — AMODE, SFREQ, RATE,
  PCMR, scale-factor LUTs, quantizer step tables, dmix LUT, LFE
  step LUTs.
* `docs/audio/dts/data/dts-huffman-tables.md` — VLC inventory and
  inline sub-tables for sizes 3..13.
* `docs/audio/dts/data/dts-pqf-window.md` — synthesis-bank prototype
  description (first 32 taps inline) + LFE filter outlines.
* `docs/audio/dts/data/dts-vq-codebook.md` — high-frequency VQ
  codebook geometry + first 16 entries inline.

No FFmpeg, libdcadec, or libdca source has been consulted.

## License

MIT — see `LICENSE`.
