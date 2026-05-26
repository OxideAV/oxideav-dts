# oxideav-dts

A pure-Rust DTS audio decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework.

## Status

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
  SFREQ/RATE/AMODE value tables landing in `docs/` (see below).
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
three fields. The wiki page itself was mirrored as-is, so those
tables are not in `docs/`:

1. **Sample-frequency index → Hz**: SFREQ is a 4-bit code; the
   mapping table (16 entries) is missing. `DtsFrameHeader::sample_rate_hz()`
   returns `None` until it lands.
2. **Transmission-bitrate index → bps**: RATE is a 5-bit code; the
   mapping table (32 entries, often including reserved / open-rate
   sentinels) is missing. `DtsFrameHeader::bit_rate_bps()` returns
   `None`.
3. **AMODE → channel-count / layout**: AMODE 0..=15 is documented
   as "standard layouts" but the layout descriptions (mono, dual-
   mono, L+R, L+R+C, …) are not in the snapshot.
   `DtsFrameHeader::channel_count()` returns `None`.

A clean-room recipe for filling the gap: cite ETSI TS 102 114
§5.3.1 tables 5.7 / 5.8 / 5.9 verbatim (the spec is public on the
ETSI portal) and mirror them into `docs/audio/dts/spec/`.

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

### Round-6 docs gaps

7. **14-bit container-byte advance rule**: the wiki snapshot
   documents `frame_size_bytes` as the byte length of the unpacked
   raw-16-bit stream; the corresponding container-byte advance for
   the 14-bit-packed encodings (the byte distance from one
   14-bit-packed sync to the next) is not enumerated. The natural
   `frame_size_bytes * 8 / 14` (rounded up to the next even byte)
   estimate is plausible but unverified, so the round-6
   `iter_frames` helper refuses to walk 14-bit container streams
   and yields `Error::UnsupportedFourteenBit` for them. Once ETSI
   TS 102 114 §5.3 / §6 documents the rule it can be wired into
   `iter_frames` and the gap closes. The single-frame
   `parse_frame_header_14bit` entry point is unaffected — it
   parses one already-sliced 14-bit frame at a time.

## License

MIT — see [LICENSE](./LICENSE).
