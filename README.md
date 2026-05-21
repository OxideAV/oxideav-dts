# oxideav-dts

A pure-Rust DTS audio decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework.

## Status

**Round 4 — `oxideav-core` framework integration.** Round 1 landed
the structural frame-header parser; round 2 added the two 14-bit-packed
sync encodings (`1F FF E8 00 07 Fx` BE and `FF 1F 00 E8 Fx 07` LE) via
`unpack_14bit_to_16bit` plus the dedicated `parse_frame_header_14bit`
entry point. Round 3 (2026-05-21) extended the typed header through
the 13 single-bit and small-field flags that follow RATE in the wiki
layout (downmix, dynamic-range, time-stamp, aux-data, HDCD, 3-bit
extension-audio descriptor, extension-audio coding, ASPF, 2-bit LFE
mode, predictor-history) plus the optional 16-bit `HEADER_CRC` field
that is emitted iff `crc_present` is set. Round 4 (2026-05-22) wires
the crate into `oxideav-core`'s `Decoder` trait surface behind a
default-on `registry` cargo feature, claims the `dts` and `dtsc`
FourCC tags in the codec registry, and exposes a standalone
`probe_dts` confidence helper. With `--no-default-features` the
crate has no `oxideav-core` dep and surfaces only the structural
parsers; an inline `ci-standalone` CI job exercises that path on
every push.

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

## License

MIT — see [LICENSE](./LICENSE).
