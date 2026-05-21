# oxideav-dts

A pure-Rust DTS audio decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework.

## Status

**Round 2 — 14-bit sync unpacking.** Round 1 (2026-05-21) landed the
structural frame-header parser; round 2 adds support for the two
14-bit-packed sync encodings (`1F FF E8 00 07 Fx` BE and
`FF 1F 00 E8 Fx 07` LE) by way of a small bit-level unpacker
(`unpack_14bit_to_16bit`) that converts 14-bit-packed input into
the equivalent 16-bit raw-BE byte stream the round-1 parser already
understands. A dedicated `parse_frame_header_14bit` entry point
keeps the two accepted-input sets disjoint at the type level.

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

A black-box test against a real `ffmpeg -c:a dca -ar 48000 -ac 2
-b:a 768k` frame is included; ffmpeg is invoked only as an
opaque generator, not consulted as source. Round 2 adds two
companion fixtures repacked into the 14-bit BE and LE container
forms; all three encodings recover the identical structural
fields.

Subband, QMF, Huffman, vector-quantisation, DTS-HD / EXSS / XLL /
X96 / XCH all remain out of scope.

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

## License

MIT — see [LICENSE](./LICENSE).
