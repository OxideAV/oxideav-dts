# oxideav-dts

A pure-Rust DTS (DTS Coherent Acoustics) decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework, built clean-room
from a locally-staged copy of ETSI TS 102 114 V1.3.1.

## Status

This crate is an **in-progress Core-profile decoder**. The frame
container and structural parsing are complete; the DSP chain that
reconstructs PCM is being built up primitive by primitive and is **not
yet wired into an end-to-end audio decoder** — the registry `Decoder`
impl currently returns `CoreError::Unsupported` for the audio-data
reconstruction step.

### What works today

- **Frame-header parsing** (`parse_frame_header` /
  `parse_frame_header_14bit`, typed `DtsFrameHeader`) — the §5.3 Core
  sync header for all four bitstream forms (16-bit big/little-endian and
  the two 14-bit container forms, via the `unpack14` helpers), including
  the trailing single-bit / small-field flags, the optional 16-bit
  `HEADER_CRC` field, and the post-CRC trailing window (multirate-inter,
  version, copy-history, PCMR, front/surround sum, and the §5.3.1
  Table 5-20 `DIALNORM` dialog-normalization gain).
- **Frame framing** — `iter_frames` / `iter_frames_14bit` /
  `FrameIterator` / `FrameView` plus `find_next_sync` walk and resync a
  multi-frame elementary stream (raw and 14-bit container streams are
  routed by encoding).
- **Side-information decode** — the §5.4.1 Primary Audio Coding Side
  Information walker (`decode_primary_side_info_at`) decodes the
  SSC/PSC prefix, PMODE/PVQ/ABITS/TMODE/SCALES planes, and the TMODE
  codebooks end-to-end through SCALES.
- **DSP primitives** — clean-room transcriptions of the building blocks
  the §5.5 audio-data reconstruction needs: the §C.2.1 block-code
  decoder (both the modulus and table-look-up variants), the §C.2.2
  inverse-ADPCM predictor, the §C.2.3 / §C.2.4 sum-difference and
  joint-subband steps, the §C.2.5 32-band synthesis QMF
  (`QmfSynthesis`), the §D.2 quantization step-size tables and §5.5
  inverse-quantization scale composition, the §D.8 512-tap 32-band
  interpolation FIR coefficient sets plus the two §D.8 512-tap **LFE**
  interpolation FIR sets (`RA_COEFF_LFE64` / `RA_COEFF_LFE128`) with the
  typed §C.2.6 `LfeInterpolationSelection` (`nDecimationSelect`) driver
  selector, the §5.5 `nQType` dispatch, the
  §D.6 block code books, the §D.5.1/§D.5.3/§D.5.4/§D.5.5/§D.5.7
  audio-data quantization-index Huffman code books (the five lowest
  `ABITS` families — 3/5/7/9/13-level — feeding the `nQType == 1` path,
  decoding to signed `AUDIO[m]` levels via `AudioHuffCodebook` /
  `decode_audio_huff_at`), and the §5.5 `DSYNC` subsubframe check word.

### Not yet implemented

- The §5.5 `Audio Data` walker that composes the side-info, dispatch,
  dequantization, ADPCM, and QMF primitives into reconstructed subband
  samples — and thus PCM output (the registry `Decoder` returns
  `Unsupported` for this step).
- The remaining §D.5 audio-data quantization-index Huffman code books
  feeding the `nQType == 1` Huffman path: the higher `ABITS` families
  (§D.5.8 17-level, §D.5.9 25-level, §D.5.10 33-level, §D.5.11 65-level,
  §D.5.12 129-level). The five lowest families
  (§D.5.1/§D.5.3/§D.5.4/§D.5.5/§D.5.7) are landed.
- The Table 5-21 Core Audio Coding Header decoder feeding the §5.4.1
  walker.
- The §C.2.6 `InterpolationFIR()` LFE-reconstruction driver *body* (the
  per-sample 512-tap polyphase convolution loop). The §D.8 LFE
  coefficient tables and the `nDecimationSelect` table selector are
  landed, but the §C.2.6 loop-body pseudocode is not transcribed in the
  staged `docs/audio/dts/` material, so the convolution step awaits that
  staging.
- Extensions (EXSS / XCH / XXCH / X96 / XLL) are out of scope for the
  current Core-profile effort.
- The `HEADER_CRC` polynomial is not documented in the staged spec
  material, so `DtsFrameHeader::verify_header_crc` returns `None`; the
  raw 16-bit field is still surfaced for pass-through callers.

## Usage

```rust
use oxideav_dts::{parse_frame_header, iter_frames};

let bytes: &[u8] = b""; // a DTS Core (raw 16-bit) elementary stream

// Parse a single Core frame header.
if let Ok(_hdr) = parse_frame_header(bytes) {
    // inspect channel layout, sample-rate code, frame size, ...
}

// Walk a multi-frame stream.
for frame in iter_frames(bytes) {
    let _payload = frame.payload();
}
```

The DSP primitives are public crate-root re-exports
(`decode_block_code`, `QmfSynthesis`, `fir_step`, `dequant_subsubframe`,
…) for callers experimenting with the reconstruction chain directly.

## Cargo features

| Feature    | Default | Effect |
|------------|---------|--------|
| `registry` | yes     | Pulls in `oxideav-core` and registers the codec via `register`, exposing the `Decoder` trait surface and `probe_dts`. Disable (`default-features = false`, build `--no-default-features --lib`) for a standalone build that exposes only the header parser, framing, and DSP primitives without the framework dependency. |

## Clean-room provenance

Implemented entirely from a locally-staged copy of ETSI TS 102 114
V1.3.1 under `docs/audio/dts/`. No external decoder or library source
was consulted; binaries are used only as black-box fixture generators
and validators, never as a source of constants or layout.

## License

MIT — see [LICENSE](LICENSE).
