# oxideav-dts

A pure-Rust DTS (DTS Coherent Acoustics) decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework, built clean-room
from a locally-staged copy of ETSI TS 102 114 V1.3.1.

## Status

This crate is an **in-progress Core-profile decoder**. The frame
container, structural parsing, and the full DSP reconstruction chain are
in place: the registry `Decoder` now **decodes raw 16-bit DTS Core
frames to PCM end to end** for the common Core case (§5.3 → §5.4 → §5.5 →
§C.2.5), emitting a planar S32 `AudioFrame`, and **carries the §C.2.5
per-channel QMF filter tail across frames** (`CoreStreamDecoder`) so a
multi-frame elementary stream reconstructs without a per-frame
filter-warmup transient. This full-chain output is **validated against a
black-box `ffmpeg -c:a dca` reference decode** of the bundled 5-frame
fixture: our PCM is shape-identical to the reference (Pearson
correlation 1.0, 100 % sign agreement on both channels), confirming the
reconstruction chain is correct up to the implementation-defined output
`rScale` gain (the spec leaves §C.2.5 `rScale` non-normative). The
§5.4.1 Table 5-28 side-info tail is handled for **dynamic range**
(`DYNF`: the 8-bit `RANGE` index is read and the §D.4 multiplier applied
to the reconstructed PCM after QMF synthesis) and the **side-info CRC**
(`CPF`: the 16-bit `SICRC` is consumed for framing, not verified).
**LFE-bearing frames** (`LFF != 0`) now decode correctly: the §5.5 LFE
phase (`2·LFF·nSSC` 8-bit samples + `LFEscaleIndex`) is consumed before
the audio-data phase so the audio-data cursor stays aligned, and the LFE
samples are dequantised (§D.1.2 `RMS_7BIT` scale + `0.035` step) and
upsampled through the §C.2.6 `InterpolationFIR()` polyphase convolution
(`LfeChannel`); the decoded LFE PCM is surfaced via
`SubframePcmDecoder::take_last_lfe_pcm`. Only **joint-intensity** frames
(`JOINX > 0`) and the §D.10 VQ / ADPCM code books still surface
`CoreError::Unsupported`.

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
  selector **and the §C.2.6 `InterpolationFIR()` polyphase convolution
  driver body** (`LfeInterpolator`, `src/lfe_synth.rs`: each decimated
  LFE sample expands to 64/128 interpolated PCM samples, carrying the
  `taps_per_phase − 1` inter-sub-frame history) and the **§5.5 LFE phase
  dequant** (`LfeChannel`: 8-bit `LFE[n]` → `rLFE[n] = LFE[n]·nScale·
  0.035` with the §D.1.2 `RMS_7BIT` scale, then `InterpolationFIR(LFF)`),
  the §5.5 `nQType` dispatch, the
  §D.6 block code books, the
  §D.5.1/§D.5.3/§D.5.4/§D.5.5/§D.5.7/§D.5.8/§D.5.9 audio-data
  quantization-index Huffman code books (the seven lowest `ABITS`
  families — 3/5/7/9/13/17/25-level; the 17-level group is the seven
  §D.5.8 books `A17`…`G17` and the 25-level group the seven §D.5.9
  books `A25`…`G25` whose deepest codeword reaches 14 bits — feeding
  the `nQType == 1` path, decoding to signed `AUDIO[m]` levels via
  `AudioHuffCodebook` / `decode_audio_huff_at` with a per-book
  `max_code_len` walk bound), and the §5.5 `DSYNC` subsubframe check
  word.
- **Header → §C.2.5 QMF-driver bridge** — `DtsFrameHeader` now resolves
  the two header-sourced parameters of the §C.2.5 `QMFInterpolation()`
  driver directly: `filter_bank_selection()` maps the `MULTIRATE_INTER`
  bit (the spec's `FILTS` "Multirate Interpolator Switch" of §5.3.1
  Table 5-15) to the §D.8 coefficient set (`false`/`FILTS==0` →
  non-perfect `raCoeffLossy`, `true`/`FILTS==1` → perfect
  `raCoeffLossLess`), and `output_r_scale()` derives the post-filterbank
  output gain `rScale = 2^(PCMR_bits−1)` from the §5.3.1 Table 5-17
  source-PCM resolution (`Some(32768/524288/8388608)` for 16/20/24-bit,
  `None` for the two reserved PCMR codes). A parsed header now feeds
  `QmfSynthesis::synthesize` end-to-end with no out-of-band parameters.
- **Per-frame multi-channel synthesis** — `MultiChannelQmf` owns one
  persistent `QmfSynthesis` per channel (the §C.2.5 `aPrmCh[ch]` filter
  objects) and runs the per-channel driving call
  `aPrmCh[ch].QMFInterpolation(FILTS, nSUBS[ch])` for every channel of a
  frame in one step, with the frame-wide `FILTS` and output `rScale`
  shared across channels. It reconstructs a whole frame's PCM either
  **planar** (per-channel `Vec<i32>`) or **interleaved** (sample-major),
  takes per-channel `nSUBS`, persists every channel's inter-frame filter
  tail across calls, and offers a `synthesize_planar_from_header`
  convenience that sources `FILTS`/`rScale` straight from a parsed
  `DtsFrameHeader` (returning `Ok(None)` for the reserved PCMR codes).

- **End-to-end frame decode** — `decode_core_frame(bytes, &header)`
  chains the §5.3.2 Audio Coding Header (Table 5-21), the per-subframe
  §5.4.1 side-info walk (Table 5-28) **including the `RANGE`/`SICRC`
  tail**, and the §5.5 + §C.2.5 reconstruction into one raw-bytes-to-PCM
  call. It decodes every frame whose channels all have `JOINX == 0`,
  including `DYNF != 0` frames (the §D.4 dynamic-range multiplier is
  applied to each subframe's PCM after synthesis) and `CPF == 1` frames
  (the `SICRC` word is consumed). `SubframePcmDecoder` (with
  `decode_subframe` / `decode_frame`) is the lower-level composition of
  the §5.5 `decode_audio_data_subframe_at` walk and the §C.2.5
  `MultiChannelQmf` synthesis, owning a persistent per-channel filter so
  the inter-subframe filter tail carries across subframes.
- **Streaming decode** — `CoreStreamDecoder` wraps a stream-lifetime
  `SubframePcmDecoder` so the §C.2.5 per-channel filter tail (`raX[]` /
  `raZ[]`) carries across **frame** boundaries of a contiguous
  elementary stream — the spec's QMF filter is a continuous per-channel
  object, not reset between frames. `decode_core_frame` (a fresh
  per-call decoder) keeps single-frame semantics; `CoreStreamDecoder` is
  the multi-frame path. The registry `Decoder::receive_frame` holds a
  persistent `CoreStreamDecoder` so multi-packet streams carry the
  filter tail across packets, and emits a planar S32 `AudioFrame`;
  joint-intensity tails and §D.10 VQ/ADPCM blockers return a typed
  `CoreFrameDecodeError` (mapped to `Unsupported`). Carrying the
  inter-frame tail is what makes the decode match the `ffmpeg` reference
  (correlation 1.0 vs 0.73 with a per-frame reset — see
  `tests/black_box_ffmpeg_pcm.rs`).
- **§5.4.1 side-info tail** — `decode_primary_side_info_tail_at` /
  `SideInfoTail` walk the Table 5-28 tail after the SCALES block: the
  8-bit `RANGE` dynamic-range index (`DYNF`, looked up via the §D.4
  `drc_range` 256-entry multiplier table) and the 16-bit `SICRC`
  (`CPF`). Joint-intensity (`JOINX > 0`) is declined pending the
  joint-scale table.

### Not yet implemented

- The §5.4.1 `JOIN_SHUFF` / `JOIN_SCALES` joint-intensity tail (when
  `JOINX > 0`). The §C.2.3 joint-subband decode itself is landed, but its
  `JOIN_SCALES` Huffman side-info decode needs the joint-scale-factor
  table, which is not transcribed in `docs/audio/dts/`, so
  `decode_core_frame` still declines `JOINX > 0` frames. (The `RANGE` /
  `DYNF` and `SICRC` / `CPF` tail fields *are* handled — see "What works
  today".)
- The §D.10.1 ADPCM-coefficient VQ and §D.10.2 high-frequency-subband VQ
  code books (a `PMODE != 0` or `nVQSUB < nSUBS` subband surfaces a typed
  blocker) — those Annex D VQ tables are not transcribed in
  `docs/audio/dts/`.
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

// Decode one whole Core frame to planar PCM (common Core case).
use oxideav_dts::decode_core_frame;
if let Ok(hdr) = parse_frame_header(bytes) {
    match decode_core_frame(bytes, &hdr) {
        Ok(pcm) => { /* pcm[ch] is a Vec<i32> of reconstructed samples */ }
        Err(_unsupported_tail_or_vq) => { /* not the common Core case */ }
    }
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
