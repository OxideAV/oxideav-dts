# oxideav-dts

A pure-Rust DTS audio decoder for the
[oxideav](https://github.com/OxideAV/oxideav) framework.

## Status

**Round 241 — DIALNORM / UNSPEC → Dialog Normalization Gain in dB
(ETSI §5.3.1 Table 5-20).**
Round 241 (2026-06-06) closes the round-5 DIALNORM docs gap (the
last open `Option`-resolver gap on the post-CRC trailing window) by
wiring ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-20 ("Dialog
Normalization Parameter", staged PDF p.24) into the header. The
4-bit `DIALNORM`/`UNSPEC` field is routed through `version` (the
4-bit `VERNUM` field that precedes it in the post-CRC window) per
the §5.3.1 narrative: `VERNUM == 7` ⇒ the field is `DIALNORM` and
codes 0..=15 resolve to 0 dB through −15 dB; `VERNUM == 6` ⇒ the
field is `DIALNORM` and codes 0..=15 resolve to −16 dB through
−31 dB; every other `VERNUM` ⇒ the field is `UNSPEC` and DNG is
fixed at 0 dB per the spec's "DNG=0 indicates No Dialog
Normalization" sentence (PDF p.23). New public surface: the
`DialogNormalization` enum with `Fixed(i8)` and `Unspecified`
variants plus a `gain_db() -> i8` accessor;
`DtsFrameHeader::dialog_normalization_gain()` returns the typed
variant. The existing `DtsFrameHeader::dialog_normalization_db()`
— which had returned `None` since round 5 — now returns
`Some(db)` across every reachable `(VERNUM, DIALNORM)` pair, with
the `Unspecified` branch resolving to `Some(0)` to surface the
spec-prescribed playback semantics. Five new in-module tests in
`src/header.rs` lock the table down: an exhaustive 32-row sweep of
the two Table 5-20 named-VERNUM rows
(`VERNUM ∈ {6, 7} × DIALNORM ∈ 0..=15` → DNG = 0, −1, …, −31);
an exhaustive sweep of the fourteen UNSPEC-branch VERNUM values
(`VERNUM ∈ {0,1,2,3,4,5,8,9,10,11,12,13,14,15} × DIALNORM ∈
0..=15` → DNG = 0); a boundary-row check on the pure-function
`dialog_normalization_from_codes` helper (`(7, 0)` → 0;
`(7, 15)` → −15; `(6, 0)` → −16; `(6, 15)` → −31; high bits of
both inputs masked off so the resolver consults only the
documented 4-bit wire widths); a `gain_db` projection check
across both variants; and a range-coverage cross-check that
confirms the resolver's range across all 256
`(VERNUM, DIALNORM)` pairs is exactly the spec's `{0, −1, …, −31}`
dB set. The existing `dialnorm_code_round_trips_for_every_4bit_value`
test is updated to assert `Some(0)` in the UNSPEC branch instead
of the previous `None`. The black-box `ffmpeg` 48 kHz / stereo /
768 kb/s fixture (VERNUM=7, DIALNORM=0 → DNG = 0 dB) now asserts
`dialog_normalization_db() == Some(0)` and the resolver returns
`DialogNormalization::Fixed(0)`. With this round the post-CRC
trailing window's three `Option`-resolver gaps (DIALNORM, PCMR,
CHIST) are all closed; the remaining open header gap is the
HEADER_CRC polynomial (round-3 gap #4). Scope: this round only
lands the Table 5-20 resolver; the §D.8 32-band FIR coefficient
tables (round-208 gap #9) remain pending docs staging.

**Round 232 — §C.2.1 Block Code (ETSI Annex C §C.2.1).**
Round 232 (2026-06-04) lands the §C.2.1 block-code decoder, the
mixed-radix unpacking step that turns one code word into the array of
quantisation indices the rest of the §C.2 chain (inverse ADPCM, joint
subband, sum/difference, downstream stages) consumes. Source: ETSI TS
102 114 V1.3.1 (2011-08) Annex C (informative) §C.2.1 (staged PDF
p.182–183). The spec gives two decoder variants (a §D.6-table
look-up walker and a modulus / integer-division walker); this round
implements the modulus / integer-division variant because it is fully
specified by the spec text alone (the table look-up variant requires
the §D.6 "rearranged" code-book rows enumerated as Table C-1, which
are a follow-up). The new entry points are
`decode_block_code(code, n_levels, &mut output)` for the in-place
decode, `block_code_offset(n_levels)` for the spec's
`(nNumLevel - 1) >> 1` mid-range offset, and
`block_code_max_code(n_elements, n_levels)` for the largest valid
code word `n_levels.pow(n_elements) - 1`. Two new `Error` variants:
`BlockCodeLevelsOutOfRange { n_levels }` fires when
`n_levels < 2` (the mixed-radix recurrence is undefined for
single-element or zero-element alphabets), and
`BlockCodeResidual { residual, n_elements, n_levels }` surfaces the
§C.2.1 spec text's "ERROR: block code look-up fail" condition (a
non-zero residual after the last element). The spec's worked
example reproduces verbatim: `code = 64`, `n_levels = 3`, four
elements decode to `[0, -1, 0, +1]` matching the spec's
quotient/remainder trace at each step. Seventeen new in-module unit
tests plus three doc-tests lock the decoder down: the spec's worked
example end-to-end, the spec's worked example intermediate
quotients (step-by-step `(rem, next_q)` table from PDF p.182), the
3-level 4-element domain round-tripped exhaustively (all 81 valid
code words), the 5-level 3-element domain round-tripped
exhaustively (all 125 code words), the alphabet-bound invariant
(every decoded index lands in
`[-(n_levels-1)/2, (n_levels-1)/2]`), the all-zero-code-word edge
case (decodes to the all-bottom-of-alphabet index array), the
max-code-word edge case (decodes to the all-top-of-alphabet index
array), the one-past-max residual error, the `n_levels < 2`
rejection (both `0` and `1`), the empty-output success path
(`code == 0`) and residual path (`code != 0`), the smallest
non-trivial 3-level 1-element alphabet, the binary (`n_levels = 2`,
offset 0) decode reading the code's bits LSB-first, and the
largest §D.6 25-level 1-element alphabet (`offset = 12`). Scope:
this round lands the modulus / integer-division decoder primitive
only; the table look-up variant and the §C.2.1 dispatch wiring
into a subframe walker remain follow-ups.

**Round 228 — §C.2.2 Inverse ADPCM (ETSI Annex C §C.2.2).**
Round 228 (2026-06-04) lands the §C.2.2 inverse-ADPCM predictor, the
per-subband reconstruction step that runs whenever the §5.4.1 `PMODE`
side-info flag is set on a subband. Source: ETSI TS 102 114 V1.3.1
(2011-08) Annex C (informative) §C.2.2 (staged PDF p.183). The spec's
normative pseudocode walks `m ∈ [0, nNumSample)` per output sample;
each iteration accumulates the residual with a four-tap dot product of
the ADPCM coefficients (`raADPCMCoeff[0..4]`) against the four
preceding reconstructed samples (`raSample[m-1..m-4]`), where the
negative-index slots `raSample[-1..-4]` are seeded from the prior
decode block's tail. Six new public entry points plus a constant: the
predictor variants `inverse_adpcm_decode_i32` / `inverse_adpcm_decode_f64`
take `(history, coeffs, samples)` and overwrite the residuals in
`samples` with the reconstructed signal in place; the rolling-history
helpers `update_history_i32` / `update_history_f64` slide the last four
reconstructed samples into the history buffer for the next block (with
short-block fallback that shifts existing history left by
`samples.len()`); `inverse_adpcm_required(pmode)` is the dispatch
predicate (`pmode == 1`); and `NUM_ADPCM_COEFF: usize = 4` exposes the
spec's `NumADPCMCoeff` invariant for buffer sizing. One new error
variant, `Error::InverseAdpcmShapeMismatch { history_len, coeffs_len }`,
fires when either argument's length disagrees with the spec's fixed
four-tap shape. The i32 variant uses `wrapping_add` / `wrapping_mul`
to mirror the spec's C `int` semantics. The predictor walks
strictly left-to-right: each freshly-written `samples[m]` is the
`n = 0` history slot consumed at step `m + 1`. Twenty-six new
in-module unit tests in `src/inverse_adpcm.rs` plus two doc-tests lock
the predictor down: zero-coefficient identity, the four-tap
history-slot mapping (`coeff[0]` taps `raSample[-1]`, `coeff[3]` taps
`raSample[-4]`, confirmed at `m = 0` with the four-decimal-digit
`(1, 10, 100, 1000)` coefficients against the four-decimal-digit
`(1, 2, 3, 4)` history seeded to produce `1234`), the
freshly-written-sample-feeds-next-step ordering (geometric-sequence
property: residual `1`, coeff `(2, 0, 0, 0)` → `(1, 2, 4, 8)`), the
four-tap walk-off at `m == 4` (the all-zero block remains all-zero
when no positive samples can flow in from the residuals), wrapping
arithmetic at `i32::MAX * 2` and `i32::MIN * -1`, sign correctness for
negative coefficients, empty-block no-op, the four history-vs-coeffs
length-mismatch error paths (short / long history, short / long coeffs),
the floating-point variant's exact-arithmetic property at
`coeff = 0.5`, the history-update helper's three regimes (long block
takes the four-sample tail, exact-four-sample block replaces history
wholesale, short block shifts history left by `samples.len()` and
appends the residual), the `pmode == 1` dispatch predicate across all
256 byte values, and a two-block continuation property that confirms
decoding a 12-sample residual stream as two blocks `(0..7) + (7..12)`
with history slid between them is identical to decoding it as a single
12-sample block. Scope: this round lands the per-subband predictor
primitive and the dispatch predicate / rolling-history helpers only;
wiring it into a complete subframe walker (which needs the per-subband
`PMODE` decoder and the ADPCM-coefficient extractor from §5.4.1
Primary Audio Coding Side Information that remain in the side-info
docs gap) is a follow-up. The §C.2.5 32-band synthesis QMF entry point
is also unblocked but still needs the §D.8 FIR coefficient tables.

**Round 223 — §C.2.3 Joint Subband Coding (ETSI Annex C §C.2.3).**
Round 223 (2026-06-03) lands the §C.2.3 joint-subband decode, the
per-channel reconstruction step that copies the high-end subband
samples of a source channel into a destination channel and scales
them by the destination channel's per-subband `JOIN_SCALES[ch][n]`
factor. Encoder side: when joint-subband coding is active for channel
`ch`, the encoder drops the destination channel's high subbands from
the wire (only the source channel's high subbands are coded); the
decoder re-synthesises the destination's high subbands at unpack
time. Source: ETSI TS 102 114 V1.3.1 (2011-08) Annex C (informative)
§C.2.3 (staged PDF p.184). The spec's normative pseudocode walks
`ch ∈ [0, nPCHS)`; when `JOINX[ch] > 0` the destination's subband
range `n ∈ [nSUBS[ch], nSUBS[nSourceCh])` (with `nSourceCh = JOINX[ch]
- 1`) is overwritten by `JOIN_SCALES[ch][n] *
aPrmCh[nSourceCh].aSubband[n].aSample[nSample]` across every
`nSample ∈ [0, 8*nSSC)`. Four new public entry points:
`joint_subband_decode_range_i32` / `joint_subband_decode_range_f64`
are slice-of-slices copy + scale primitives that walk the §C.2.3
inner loop across the imported subband range and overwrite the
destination samples per the spec; `joint_source_channel(joinx)`
resolves the one-based `JOINX[ch]` field to the zero-based source-
channel index (`0` → `None` per the `JOINX[ch] > 0` gate; `joinx
> 0` → `Some(joinx - 1)` per the spec's inline comment); and
`joint_subband_required(joinx)` is the dispatch predicate that
returns `true` when `joinx > 0`. One new error variant,
`Error::JointSubbandShapeMismatch { dst_len, src_len }`, fires when
any §C.2.3 structural invariant is violated (`n_subs_dst >
n_subs_src`, dst/src per-channel array shorter than `n_subs_src`,
`scales.len() != n_subs_src - n_subs_dst`, or a per-subband
destination/source sample-length disagreement). Twenty new lib-
level tests in `src/joint_subband.rs` plus three doc-tests lock the
decode behaviour down: one-based → zero-based source-channel
resolution across `joinx ∈ 1..=u8::MAX`, the `joinx == 0`
disabled-channel case, copy-and-scale on a two-subband × three-sample
fixture, the leave-untouched property below `n_subs_dst`, the
empty-range no-op (`n_subs_dst == n_subs_src`), zero-scale zeroing,
negative-scale sign-inversion, the wrapping-multiply property at
`i32::MIN × -1` (mirroring the spec's C `int` semantics),
write-only-inside-range at `(n_dst, n_src) = (2, 4)`, and each error
path (`n_subs_dst > n_subs_src`, dst outer too short, src outer too
short, scales-length disagreement, inner sample-length disagreement)
for both i32 and f64. A `(n_dst, n_src, n_samples) = (2, 5, 8)`
end-to-end sweep cross-checks the helper against an independent
hand-computed expected. Scope: this round lands the per-channel
copy + scale and the dispatch predicate / source-channel resolver
only; wiring it into a complete subframe walker (which also needs
the `JOINX[ch]` / `nSUBS[ch]` / `JOIN_SCALES[ch][n]` decoders from
the AUDIO CODING HEADER plus the §5.4-onwards subband / QMF-
synthesis decode path that remains gated on the §D.8 FIR coefficient
tables) is a follow-up.

**Round 214 — §C.2.4 Sum/Difference Decoding (ETSI Annex C §C.2.4).**
Round 214 (2026-06-03) lands the §C.2.4 sum/difference matrix decoder,
the inverse of the encoder-side joint sum/difference coding that the
`FRONT_SUM` (`SUMF`) and `SURROUND_SUM` (`SUMS`) header flags signal,
and that `AMODE == 3` (Sum/Difference channel arrangement) implies for
the front pair regardless of the `SUMF` bit. Source: ETSI TS 102 114
V1.3.1 (2011-08) Annex C (informative) §C.2.4 (staged PDF p.184). The
spec's normative pseudocode is two parallel loops over all active
subbands × all sub-sub-frame samples, applying the matrix
`(L', R') = (L + R, L − R)` with the pre-update value of `L` consumed
for both outputs. Six new public entry points: `sum_difference_decode_i32`
/ `sum_difference_decode_f64` are single-pair primitives that
in-place decode one `(left, right)` sample slice through the matrix;
`sum_difference_decode_subband_pair_i32` /
`sum_difference_decode_subband_pair_f64` walk the same matrix across
the §C.2.4 outer subband loop (slice-of-slices, one inner slice per
active subband); `front_sum_difference_required(front_sum, amode)` is
the dispatch predicate that returns `true` when `SUMF` is set OR
`AMODE == 3` (per the §C.2.4 narrative); and
`surround_sum_difference_required(surround_sum)` is the surround-pair
counterpart, which reduces to the `SUMS` flag because the spec does
not name an `AMODE` code that forces the surround decode. One new
error variant, `Error::SumDiffLengthMismatch { left_len, right_len }`,
fires when the left and right slices passed to any of the four
decoders have different lengths (the §C.2.4 pseudocode requires a
one-to-one sample pairing). Twenty-four new lib-level tests in
`src/sum_diff.rs` plus two doc-tests lock the matrix behaviour down:
the encoder-decoder round-trip property
`decode(encode(L, R)) = (2L, 2R)` (cross-checked over a 256-element
sweep in i32 and a dyadic-rational pair in f64), the matrix
self-product `M² = 2 I` (applying decode twice doubles both inputs),
the read-old / write-new ordering check (writing `*l` first would
yield `(L+R, L)` instead of the spec-correct `(L+R, L−R)`), wrapping
arithmetic at `i32::MAX`, empty-slice no-ops, slice-length-mismatch
error reporting, subband-pair walks across `nSUBS = 3..=4` with
`8 * nSSC ∈ {2, 8}`, outer-slice-count mismatch detection and
per-subband length-mismatch detection (with the first-mismatch
position reported), front-pair dispatch-predicate truth tables across
the full 64-code `AMODE` range (including the eight user-defined codes
that do not force the front decode), and the surround-pair
dispatch-predicate behaviour. The full §C.2.4 sweep at `nSUBS = 4`,
`8 * nSSC = 8` cross-checks the subband-pair helper against an
independent hand-computed expected result. Scope: this round lands
the matrix decode and the dispatch predicates only; wiring it into a
complete subframe walker (which needs the §5.4.x AUDIO CODING HEADER
fields plus the §5.4-onwards subband / QMF-synthesis decode path that
remains gated on the §D.8 FIR coefficient tables) is a follow-up.

**Round 208 — `PreCalCosMod()` 544-entry cosine-modulation matrix (ETSI Annex C §C.2.5).**
Round 208 (2026-06-02) lands the first §C.2.5 synthesis-QMF building
block — the 544-entry cosine-modulation coefficient array `raCosMod`
that drives the §C.2.5 `QMFInterpolation` 32-band synthesis filter
bank. The matrix is allocated once per decoder instance (per the
spec's "computed once" remark) and reused on every per-channel
synthesis call; the new `precal_cos_mod()` function returns the
populated `[f64; COS_MOD_LEN]` directly, with `COS_MOD_LEN = 544`
plus per-block start constants `COS_MOD_BLOCK{1..4}_START`
(`0 / 256 / 512 / 528`) surfacing the spec's four-block packing
(Block 1: `cos((2i+1)(2k+1) π/64)` 16×16; Block 2:
`cos(i(2k+1) π/32)` 16×16; Block 3: `+0.25 / (2·cos((2k+1) π/128))`
16; Block 4: `−0.25 / (2·sin((2k+1) π/128))` 16). The transliteration
is verbatim from the spec's `PreCalCosMod()` pseudocode as
transcribed in `docs/audio/dts/dts-core-extracts.md` §2.3 (which
quotes ETSI TS 102 114 V1.3.1 Annex C §C.2.5, PDF p.184). Twenty
new lib-level tests in `src/cos_mod.rs` lock the matrix down:
length + block-boundary constants, anchor values
(`ra[0] == cos(π/64)`, `ra[256] == 1.0`,
`ra[512] == 0.25 / (2·cos(π/128))`, `ra[528] == −0.25 / (2·sin(π/128))`),
exhaustive 256-entry walks of Block 1 and Block 2 against the
closed-form, 16-entry walks of Block 3 and Block 4, the
sign-and-monotonicity properties (Block 3 strictly positive and
monotone-increasing in k, Block 4 strictly negative with
monotone-decreasing magnitude), Block 2's i=0 column equals 1 for
every k, Block 1's last row-zero entry matches the closed form,
all 544 entries finite, Block 1 + Block 2 bounded by `[-1, +1]`,
and bit-identical determinism across two independent invocations.
Scope: this round only lands the cosine-modulation matrix; the
downstream `QMFInterpolation` synthesis loop (and the §D.8 512-tap
`raCoeffLossy` / `raCoeffLossLess` FIR coefficient tables it
multiplies in) remains a follow-up — the §D.8 tables are
referenced in the staged PDF (p.238) but not yet transcribed
under `docs/audio/dts/`, so the synthesis loop awaits that
docs-staging pass.

**Round 202 — `SFREQ` / `AMODE` / `PCMR` value-table resolvers (ETSI §5.3.1 Tables 5-5 / 5-4 / 5-17).**
Round 202 (2026-06-01) closes the three sample-rate / channel-count /
source-PCM-resolution `Option`-resolver gaps that have been documented
in README "Docs gaps" since round 1: `DtsFrameHeader::sample_rate_hz()`
now resolves the nine valid `SFREQ` codes to their `Source Sampling
Frequency` from ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-5 (8/16/32/
11.025/22.05/44.1/12/24/48 kHz) and returns `None` for the seven
reserved codes (`0b0000`, `0b0100`, `0b0101`, `0b1001`, `0b1010`,
`0b1110`, `0b1111`); `DtsFrameHeader::channel_count()` resolves the
sixteen standard `AMODE` codes to the CHS column of Table 5-4
(`1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8`) and returns `None`
for the user-defined codes `16..=63`; and
`DtsFrameHeader::source_pcm_bits_per_sample()` resolves the six valid
`PCMR` codes to 16/16/20/20/24/24 bps per Table 5-17 and returns
`None` for the two reserved codes (`0b100`, `0b111`). Three new typed
accessors (`sample_frequency()` / `amode_arrangement()` /
`source_pcm_resolution()`) carry the richer `Fixed` / `Invalid` and
`Valid { bits, es }` / `Invalid` and `Mono` / `DualMono` / `Stereo` / …
/ `UserDefined(u8)` variants the `Option` accessors collapse. Backing
tables transcribed verbatim from the staged ETSI PDF (Tables 5-4 /
5-5 / 5-17, PDF pp.18-23) into `src/header.rs`. Seven new lib-level
tests (exhaustive 16-code SFREQ walk, 64-code AMODE walk, 8-code PCMR
walk, plus a CHS-column reproduction check and a synthetic-header
parser round-trip that mirrors the bundled black-box fixture
geometry) lock the table-row-by-table-row mapping down; the
integration tests in `tests/black_box_ffmpeg.rs` now assert
`sample_rate_hz() == Some(48_000)`, `channel_count() == Some(2)`,
`source_pcm_bits_per_sample() == Some(16)` for the bundled ffmpeg
48 kHz / stereo / 16-bit / 768 kb/s frame across the three documented
sync encodings (raw-BE, 14-bit-BE, 14-bit-LE). The `DIALNORM`-code-
to-dB mapping (Table 5-20) remains a docs-completeness follow-up — the
staged PDF interleaves the `VERNUM == 6` and `VERNUM == 7` sign
conventions, so the row-by-row code→dB columns need a tighter
transcription pass before `dialog_normalization_db()` can resolve.

**Round 195 — §5.4.1 ABITS / SCALES (a.k.a. ALLOC / SCFAC) side-info decoders.**
Round 195 (2026-05-31) lands the side-information half of the core
subframe decode path: extracting the per-channel × per-subband
ABITS bit-allocation index field and the per-channel × per-subband
SCALES scale-factor field from a packed bit stream, given the
channel-wide `BHUFF[ch]` / `SHUFF[ch]` codebook selectors read
earlier from the AUDIO CODING HEADER. Three new public entry points:
`AbitsCodebook::from_bhuff(u8)` / `ScalesCodebook::from_shuff(u8)`
(Table 5-25 / Table 5-24 selectors), and the byte-slice + bit-offset
single-field decoders `decode_abits_at(bytes, bit_offset, codebook)`
(returns `(abits, bits_consumed)`) and
`decode_scales_at(bytes, bit_offset, codebook, n_scale_sum)`
(returns `(scale, updated_n_scale_sum, bits_consumed)`). Backing
tables: Annex D §D.5.6 five 12-level Huffman codebooks A12/B12/C12/
D12/E12 (BHUFF=0..4), Annex D §D.5.3 + §D.5.4 small-Huffman
codebooks A5/B5/C5 + A7/B7 routed to SA129..SE129 difference
symbols (SHUFF=0..4), and the §D.1.1 / §D.1.2 RMS square-root
quantisation tables (`RMS_6BIT: [u32; 64]` /
`RMS_7BIT: [u32; 128]`) as `pub const` arrays. Two new error
variants — `Error::InvalidSideInfo { field, value }` (reserved
BHUFF/SHUFF/SCALES values) and `Error::HuffmanDecodeFailed { table }`
(defensive bound — the Annex D codebooks are all complete prefix
codes by Kraft equality, so this fires only on EOF or stream-format
corruption). Nineteen unit tests in `src/side_info.rs` plus three
integration tests in `tests/side_info_decode.rs` lock down the
behavioural contract: BHUFF/SHUFF reserved-value rejection,
exhaustive 7-code dispatch, every ABITS Huffman symbol round-trip
across all five 12-level codebooks, Kraft completeness across all
ten transcribed codebooks, RMS table length + anchor-value
cross-check against the staged PDF, SA129 difference accumulation
across `(+2, +1, 0, -1, -2)` from `n_scale_sum=10`, SD129 7-level
table with ±3 range, negative-accumulator + reserved-index
rejection, and a 5-subband end-to-end block walked through the
public API. Scope: single-field decode + tables only; the full
subframe walker (which also requires the §5.3.x AUDIO CODING
HEADER fields SUBFS/PCHS/SUBS/VQSUB/JOINX and the SCALES loop over
`nPCHS × nSUBS[ch]`) is a follow-up. The 129-entry SA129..SE129
full mappings (Table 5-24's nominal codebook names, not
transcribed under those names in the staged Annex D revision)
remain a docs-completeness gap; this round routes SHUFF=0..4
through the small-Huffman §D.5.3 / §D.5.4 codebooks the staged
PDF does enumerate, treating their symbols as scale-factor index
differences per the §5.4.1 pseudocode.

**Round 192 — 14-bit container-byte frame iterator `iter_frames_14bit`.**
Round 192 (2026-05-30) closes the empirical half of round-6 docs gap
#7 by wiring the round-189 `frame_size_container_bytes` accessor into
a multi-frame walker that operates directly on 14-bit-packed
container bytes. The new `iter_frames_14bit(bytes)` returns a
`FrameIterator14<'_>` whose `Iterator::next` step calls
`find_next_sync` to handle leading garbage, accepts only the two
14-bit syncs (`FourteenBitBigEndian` / `FourteenBitLittleEndian`),
calls the existing `parse_frame_header_14bit` to recover the typed
header from each frame's container window, and advances the cursor
by `header.frame_size_container_bytes(encoding)` container bytes —
the round-189 formula `2 * ceil(frame_size_bytes * 8 / 14)`. The
per-step `FrameView14` is a deliberate separate type (not
`FrameView`) because the `len` and `data` fields carry container-
domain semantics here (container-byte advance + container-byte
window) rather than the unpacked-domain semantics they carry in
`FrameView`. A raw 16-bit sync at the iterator's cursor surfaces the
new `Error::UnsupportedRaw16Bit` variant (symmetric counterpart to
the round-6 `Error::UnsupportedFourteenBit` on `iter_frames`) and
terminates. Twelve new tests lock the iterator's contract down: ten
unit tests (single-frame BE / LE walks; back-to-back BE frames with
cursor + length cross-check; leading garbage before first sync;
raw-16-bit sync rejection; empty buffer; no-sync buffer; truncated
tail reporting `UnexpectedEof`; `view.data` round-trips through
`parse_frame_header_14bit`; `cursor()` advances by exactly
`frame_size_container_bytes` per step) plus two integration tests
that repackage the bundled ffmpeg 5-frame fixture (5 × 1024 raw-BE
bytes) into 14-bit-packed BE and LE streams (5 × 1172 container
bytes each) and verify all five frames walk with the expected
header fields and container-byte length. The fail-fast
`iter_frames` from round 6 is unchanged — it still rejects 14-bit
syncs with `UnsupportedFourteenBit` because raw streams and
container streams live in distinct domains; callers route by sync
encoding up-front.

**Round 189 — 14-bit container-byte frame-advance accessor (ETSI §5.3.1 + §6.1.3.1).**
Round 189 (2026-05-30) adds a single new accessor,
`DtsFrameHeader::frame_size_container_bytes(SyncWordEncoding) -> u32`,
that returns the container-byte distance from this frame's syncword
to the next frame's syncword for each of the four wire encodings.
For the raw 16-bit encodings (`RawBigEndian` / `RawLittleEndian`)
the answer is just `frame_size_bytes`: per ETSI TS 102 114 V1.3.1
§5.3.1 the `FSIZE+1` field already counts on-wire container bytes
of the 16-bit-per-word stream. For the 14-bit-packed encodings
(`FourteenBitBigEndian` / `FourteenBitLittleEndian`) the same
`FSIZE+1` logical bytes are carried at 14 logical bits per 2
container bytes (one 16-bit container word carries 14 payload bits
per §3.2 / §6.1.3.1), so the span occupies
`ceil(frame_size_bytes * 8 / 14)` container words =
`2 * ceil(frame_size_bytes * 8 / 14)` container bytes. The
formula is the analytical half of round-6 docs gap #7,
transcribed verbatim from
`docs/audio/dts/dts-core-extracts.md` §3.3 (which synthesises
ETSI §5.3.1's `FSIZE` definition with the §6.1.3.1 / §6.3.x
"28-bit-word boundary" invariant). Seven new unit tests lock the
formula down: raw-equals-`frame_size_bytes`,
1024-logical→1172-container, minimum 95→110 / maximum 16384→18726
container-byte advance, strict-greater-than-raw + closed-form
`16/14` scaling upper bound, BE/LE equivalence (both raw and
14-bit pairs), the 14-bit advance is always even (the
28-bit-boundary invariant forces a two-container-word step), and a
closed-form cross-check on a spread of frame sizes. No new docs
gap; the formula's empirical half — actually walking a 14-bit
container stream through `iter_frames` — is still pending a
streaming 14↔16-bit per-frame header unpacker (the parser reads
fields from the unpacked stream, so the iterator needs that
conversion step before it can call `parse_frame_header_14bit` on
each frame slice).

**Round 185 — `RATE` → targeted bit-rate (ETSI §5.3.1 Table 5-7).**
Round 185 (2026-05-29) wires ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-7
("RATE parameter versus targeted bit-rate", transcribed in
`docs/audio/dts/dts-core-extracts.md` §1) into the header resolvers.
The new `TargetedBitRate` enum distinguishes the 25 fixed targeted
rates (`Fixed(bps)`), the open-mode code `0b11101` (`Open`), and
every reserved code (`Invalid`); `DtsFrameHeader::targeted_bit_rate()`
returns it, and `DtsFrameHeader::bit_rate_bps()` — which had returned
`None` since round 1 — now resolves the fixed codes to bits per
second (e.g. code `0b01111` → `Some(768_000)`). The mapping is
cross-validated against the existing 768 kb/s ffmpeg black-box
fixture, whose `RATE` index 15 now resolves to exactly the 768 000 bps
ffprobe reports for the same frame. Tables 5-8 (`DYNF`) / 5-9
(`TIMEF`) from the same clause are present/not-present flags already
surfaced as `dynamic_range` / `time_stamp`; their field docs now cite
the tables. One new exhaustive test walks all 32 `RATE` codes
(25 fixed + open + 6 invalid); the black-box tests assert the
768 000 bps result across the raw-BE, 14-bit-BE, and 14-bit-LE input
encodings. This closes the bitrate half of docs gap #928; the
SFREQ (sample-rate) and AMODE (channel-count) value tables remain
open (`sample_rate_hz()` / `channel_count()` still return `None`).

**Round 179 — `iter_syncs` lazy streaming iterator + `SyncWordEncoding` / `SyncMatch` accessor surface.**
Round 179 (2026-05-29) adds a streaming counterpart to the
round-151 `find_all_syncs` bulk helper plus a small accessor surface
on `SyncWordEncoding` and `SyncMatch` derived directly from the wiki
sync-sequence table (`docs/audio/dts/wiki/DTS.wiki`'s
"How to distinguish different versions" enumeration). The new
`iter_syncs(bytes) -> SyncIterator<'_>` returns an
`Iterator<Item = SyncMatch>` that walks the buffer one
`find_next_sync` hop at a time and yields matches as they appear —
same matching rules, same walk order, same `O(n)` cost as
`find_all_syncs`, but no upfront `Vec<SyncMatch>` allocation. Useful
when the caller is fine with element-by-element consumption,
wants to stop early after a `take(N)` window, or routes through
standard `Iterator` combinators (e.g.
`iter_syncs(bytes).filter(|m| m.encoding.is_raw_16bit())`). The new
`SyncWordEncoding::sync_byte_length()` reports the on-wire byte
length of the matched sync sequence (4 for raw-BE / raw-LE per the
wiki's `7F FE 80 01` / `FE 7F 01 80` rows; 6 for the two
14-bit-packed encodings per `1F FF E8 00 07 Fx` / `FF 1F 00 E8 Fx 07`);
`SyncWordEncoding::is_raw_16bit()` / `is_14bit_packed()` are
mutually-exclusive predicates that partition the enum into the
raw-vs-container distinction the wiki documents. `SyncMatch`
forwards both into `sync_byte_length()` / `sync_byte_range()` so
the common "advance the cursor past the matched sync" / "slice the
matched bytes" patterns read naturally
(`cursor = m.offset + m.sync_byte_length()` /
`&bytes[m.sync_byte_range()]`). Eleven new tests (plus one new
doc-test) lock down the byte counts against the wiki table, the
raw-vs-packed partition, the streaming-vs-bulk equivalence
(`iter_syncs(...).collect() == find_all_syncs(...)`) on a
mixed-encoding buffer, an empty-result buffer, `take(N)` window
correctness, `is_raw_16bit` filter combinator usage, and a 4 KB
pseudo-random buffer cross-check against the existing
`reference_find_all_syncs`. No new docs gap is introduced; the
existing #928 / #1055 / #1084 docs gaps remain open.

**Round 165 — `find_next_sync` first-byte gate (`O(n)` constant-factor speedup).**
Round 165 (2026-05-27) gates the multi-byte `detect_sync` call inside
`find_next_sync` behind a one-byte filter
(`is_sync_first_byte_candidate`). The four documented DTS sync
sequences (`7F FE 80 01` raw-BE, `FE 7F 01 80` raw-LE,
`1F FF E8 00 07 Fx` 14-bit-BE, `FF 1F 00 E8 Fx 07` 14-bit-LE) all
begin with distinct first bytes — `0x7F`, `0xFE`, `0x1F`, `0xFF` per
the wiki bit-table — so 252 of 256 possible payload bytes can be
rejected with a single compare-and-branch rather than the previous
4-byte raw-sync equality check + two 6-byte 14-bit container
unpacks. On uniform-random payload the inner loop visits ~98.4% of
positions with the cheap path; the walk order, returned `SyncMatch
{ offset, encoding }`, and end-of-buffer bookkeeping are
**unchanged** from round 6 — round 165 also adds eight new tests
(171 total, up from 163) including:

- a `find_next_sync_matches_pre_optimization_reference_on_candidate_dense_payload`
  harness that packs every fourth byte with a first-byte sync
  candidate but a non-sync continuation, and proves the optimised
  scanner returns the same `None` (and then the same embedded sync
  at offset 100) the pre-round-165 brute-force reference returns;
- a 4 KB pseudo-random-buffer cross-check sweeping every possible
  `start` offset and asserting per-call agreement with the
  reference;
- a `find_all_syncs_matches_reference_on_random_buffer_with_embedded_syncs`
  bulk-scan parity test that embeds one sync of each of the four
  encodings at known positions and verifies the optimised
  `find_all_syncs` recovers every (offset, encoding) pair the
  reference recovers;
- an all-`0xFF` payload stress test (every position is a first-byte
  candidate — the negative filter's degenerate case) with one real
  raw-LE sync embedded mid-buffer;
- an exhaustive 256-input check that the filter accepts exactly the
  four documented first bytes `{0x1F, 0x7F, 0xFE, 0xFF}` and
  rejects the other 252.

The downstream walkers (`iter_frames`, `iter_frames_resync`,
`find_all_syncs`) inherit the speedup transparently because they
all dispatch through `find_next_sync`. No public API surface change;
no docs gap touched (#928 / #1055 / #1084 still open).

**Round 159 — `iter_frames_resync` error-tolerant frame walker.**
Round 159 (2026-05-27) adds an error-tolerant counterpart to the
round-6 `iter_frames`: `iter_frames_resync(bytes) -> FrameIteratorResync<'_>`
walks the same raw-16-bit DTS Core stream as `iter_frames`, but when
a candidate sync turns out to be a false positive (random payload
bytes that happened to match a 4-byte sync sequence and whose
subsequent header bits fail the structural NBLKS / FSIZE bounds, or
whose declared `frame_size_bytes` overruns end-of-buffer), the
iterator yields a `ResyncEvent { offset, encoding, cause }` and
**continues scanning** from `offset + 1` instead of terminating. The
new `ResyncCause` enum documents the four discard reasons:
`StructuralBoundFailed(Error)` (NBLKS &lt; 5 or FSIZE &lt; 95 — the
classic false-positive sync signature), `HeaderEof` (sync too close
to end-of-buffer for the 13–15-byte header window),
`FrameLengthOverrunsBuffer { declared_len }` (header parses but the
declared length runs past the input), and `FourteenBitSyncSkipped`
(a 14-bit sync at the cursor; skipped rather than terminating like
the fail-fast iterator does, so a raw-16-bit stream with stray
14-bit-shaped byte sequences in payload still walks). The fail-fast
`iter_frames` from round 6 is unchanged — well-formed input walks
through both iterators identically and round 159 confirms this via a
fixture-level equivalence test (the bundled ffmpeg 5-frame fixture
yields the same five frames through both). A corrupted-header
variant of the same fixture (header byte flip in frame 2 →
NBLKS=0) demonstrates the recovery contract: the resync iterator
surfaces one `StructuralBoundFailed` event at offset 1024 and then
walks frames 3, 4, and 5 (1024 B each); the fail-fast iterator
terminates at frame 2. Useful for demuxers, stream-integrity
tooling, and forensic walkers that need to survive a corrupted
patch in the middle of a `.dts` byte stream.

**Round 151 — `find_all_syncs` bulk-scan helper + raw-LE `iter_frames` test coverage.**
Round 151 (2026-05-26) adds `find_all_syncs(bytes) -> Vec<SyncMatch>`,
the bulk-scan counterpart to the round-6 `find_next_sync`: instead of
returning the first sync at or after a cursor, it walks the entire
input buffer and returns every documented sync occurrence (all four
encodings) as a vector. Same `O(n)` cost as a `find_next_sync` loop
from `offset + 1`; the bulk helper just materialises the result for
stream-integrity tooling that needs every resync point up front
rather than walking one at a time. The round also closes a missing
coverage gap by exercising `iter_frames` against a hand-built
multi-frame raw-LE stream — the iterator was already raw-LE-capable
because `frame_size_bytes` is byte-equivalent across both raw
encodings (the wiki defines raw-LE as the 16-bit-word-swap of
raw-BE), but the previous test grid only exercised raw-BE via the
bundled ffmpeg fixture.

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

## Error-tolerant iteration (round 159)

```rust
use oxideav_dts::{iter_frames_resync, ResyncCause, ResyncEvent};

let bytes: &[u8] = /* possibly-corrupted raw .dts stream */ &[];
let mut recovered = 0usize;
let mut discarded = 0usize;
for step in iter_frames_resync(bytes) {
    match step {
        Ok(view) => {
            recovered += 1;
            println!("frame {} ok ({} B)", view.offset, view.len);
        }
        Err(ResyncEvent { offset, cause, .. }) => {
            discarded += 1;
            match cause {
                ResyncCause::StructuralBoundFailed(_) => {
                    eprintln!("false sync at {offset}: header bounds failed");
                }
                ResyncCause::HeaderEof => {
                    eprintln!("sync at {offset}: header truncated");
                }
                ResyncCause::FrameLengthOverrunsBuffer { declared_len } => {
                    eprintln!("frame at {offset} declares {declared_len} B but overruns");
                }
                ResyncCause::FourteenBitSyncSkipped => {
                    eprintln!("14-bit sync at {offset}: skipped");
                }
            }
        }
    }
}
```

The contract: every yielded step (whether `Ok` or `Err`) advances
the cursor; iteration ends naturally when `find_next_sync` finds no
more syncs. A well-formed stream walks identically to `iter_frames`
— round 159 verifies this against the bundled ffmpeg 5-frame
fixture. Round 159 also exercises the recovery path against a
manually-corrupted variant of the same fixture (one-byte flip in
frame 2's header → `NBLKS == 0`): the resync iterator surfaces one
`StructuralBoundFailed` event at offset 1024 and then recovers
frames 3, 4, and 5 from offsets 2048 / 3072 / 4096.

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
  §5.4-onwards subframe / subband / QMF-synthesis decode path. The
  RATE / SFREQ / AMODE / PCMR header-level value tables landed in
  rounds 185 / 202.
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
three fields. The wiki page itself was mirrored as-is, so some of
those tables are not in `docs/`:

1. **Sample-frequency index → Hz**: *Resolved in round 202.* ETSI
   TS 102 114 V1.3.1 §5.3.1 Table 5-5 (staged at
   `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf` p.19)
   enumerates the nine valid `SFREQ` codes (8/16/32/11.025/22.05/
   44.1/12/24/48 kHz) and the seven reserved ones.
   `DtsFrameHeader::sample_rate_hz()` now resolves the valid codes
   and returns `None` for the reserved ones;
   `DtsFrameHeader::sample_frequency()` preserves the `Fixed` vs
   `Invalid` distinction.
2. **Transmission-bitrate index → bps**: *Resolved in round 185.*
   ETSI TS 102 114 §5.3.1 Table 5-7 (transcribed in
   `docs/audio/dts/dts-core-extracts.md` §1) gives the 25 fixed
   targeted rates plus the open (`0b11101`) and invalid codes.
   `DtsFrameHeader::bit_rate_bps()` now resolves the fixed codes (e.g.
   code `0b01111` → `Some(768_000)`, cross-validated against the
   768 kb/s ffmpeg black-box fixture); `DtsFrameHeader::targeted_bit_rate()`
   preserves the open/invalid distinction via `TargetedBitRate`.
   (Tables 5-8 `DYNF` / 5-9 `TIMEF` from the same clause are
   present/not-present flags already surfaced as `dynamic_range` /
   `time_stamp`.)
3. **AMODE → channel-count / layout**: *Resolved in round 202.*
   ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-4 (staged at
   `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf` p.18)
   enumerates the sixteen standard arrangements (codes `0..=15`,
   CHS column `1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8`)
   and the user-defined range `16..=63`.
   `DtsFrameHeader::channel_count()` now resolves the sixteen
   standard codes to their CHS values and returns `None` for the
   user-defined codes; `DtsFrameHeader::amode_arrangement()`
   returns the full `AmodeArrangement` variant (per-channel layout
   per Table 5-4's `Arrangement` column).

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

5. **PCMR (source-PCM-resolution) index → bits-per-sample**:
   *Resolved in round 202.* ETSI TS 102 114 V1.3.1 §5.3.1
   Table 5-17 (staged at
   `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf` p.23)
   enumerates the six valid codes — `(16, 0)` / `(16, 1)` /
   `(20, 0)` / `(20, 1)` / `(24, 1)` / `(24, 0)` at codes
   `{0, 1, 2, 3, 5, 6}` — and marks `{4, 7}` Invalid.
   `DtsFrameHeader::source_pcm_bits_per_sample()` now resolves
   the six valid codes and returns `None` for the two reserved
   ones; `DtsFrameHeader::source_pcm_resolution()` preserves
   both the bits-per-sample value and the auxiliary DTS-ES flag.
6. **DIALNORM (dialog-normalization) code → dB**: *Resolved in
   round 241.* ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-20 (staged at
   `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf` p.24)
   enumerates the (`VERNUM`, `DIALNORM`) → Dialog Normalization
   Gain (dB) mapping: `VERNUM == 7` codes 0..=15 → 0 dB down to
   −15 dB; `VERNUM == 6` codes 0..=15 → −16 dB down to −31 dB. For
   every other `VERNUM` the field is named `UNSPEC` (PDF p.23) and
   the spec sets DNG = 0 dB. `DtsFrameHeader::dialog_normalization_db()`
   now returns the resolved dB across the whole `(VERNUM, DIALNORM)`
   product; `DtsFrameHeader::dialog_normalization_gain()`
   distinguishes the [`DialogNormalization::Fixed`] Table 5-20 row
   from the [`DialogNormalization::Unspecified`] zero-gain branch.

### Round-208 docs gaps

9. **Annex §D.8 32-band synthesis FIR coefficient tables
   (`raCoeffLossy` / `raCoeffLossLess`, 512 taps each)**: the staged
   ETSI TS 102 114 V1.3.1 PDF lists these on page 238 but
   `docs/audio/dts/dts-core-extracts.md` only references them by name
   (§2.4 names them as the `FILTS == 0` / `FILTS == 1` selector
   targets inside `QMFInterpolation`); the two 512-coefficient tables
   themselves are not yet transcribed under `docs/audio/dts/`. The
   round-208 `precal_cos_mod()` matrix does not depend on §D.8, but
   wiring the full 32-band synthesis QMF (`QMFInterpolation`) does.
   The same docs collaborator pass that lands the Table 5-20 DIALNORM
   transcription is the natural place for the §D.8 transcription.

### Round-195 docs gaps

8. **SA129..SE129 full 129-entry codebooks**: Table 5-24 names the
   five scale-factor codebooks the SHUFF=0..4 entries select but the
   staged Annex D revision (V1.3.1, 2011-08) does not transcribe them
   under those `SA129..SE129` names. Round 195 routes SHUFF=0..4
   through the staged §D.5.3 / §D.5.4 small-Huffman codebooks
   (A5/B5/C5 for SHUFF=0..2, A7/B7 for SHUFF=3..4), which match the
   ±2 (5-level) and ±3 (7-level) difference-symbol ranges Table 5-28
   expects of difference-encoded SCALES. Confirming the full
   129-level mapping (or transcribing the explicit SA129..SE129
   tables from a different revision of TS 102 114) is a
   docs-completeness follow-up. For now,
   `ScalesCodebook::is_huffman_encoded()` partitions the SHUFF=0..4
   set as the difference-encoded path per §5.4.1's
   `if (nQSelect < 5)  nScaleSum += nScale;`.

### Round-6 docs gaps

7. **14-bit container-byte advance rule**: *Resolved in round 192.*
   The analytical half landed in round 189 as
   `DtsFrameHeader::frame_size_container_bytes(SyncWordEncoding)`
   (`frame_size_bytes` for the raw encodings per ETSI §5.3.1's
   `FSIZE+1` byte definition; `2 * ceil(frame_size_bytes * 8 / 14)`
   for the 14-bit encodings per §3.3 of `dts-core-extracts.md`,
   combining §5.3.1's `FSIZE` rule with the §6.1.3.1 / §6.3.x
   28-bit-word-boundary invariant). The empirical half landed in
   round 192 as `iter_frames_14bit(bytes) -> FrameIterator14<'_>`:
   a multi-frame walker that operates directly on 14-bit-packed
   container bytes, calling `parse_frame_header_14bit` at each sync
   to recover the header (the parser internally unpacks just enough
   containers to read the 13/15-byte unpacked header window) and
   advancing by `frame_size_container_bytes(encoding)` container
   bytes per step. The fail-fast `iter_frames` from round 6 still
   refuses 14-bit syncs with `Error::UnsupportedFourteenBit`
   because raw streams and container streams live in distinct
   domains; the symmetric reciprocal — raw 16-bit syncs at the
   cursor of `iter_frames_14bit` — surfaces the new
   `Error::UnsupportedRaw16Bit` variant. Callers route by encoding
   up-front.

## License

MIT — see [LICENSE](./LICENSE).
