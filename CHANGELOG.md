# Changelog

All notable changes to this crate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the crate adheres
to [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Round 314 (2026-06-15) — Annex D §D.5 audio-data quantization-index
  Huffman code books for the four lowest `ABITS` families, feeding the
  §5.5 Table 5-29 `nQType == 1` ("Huffman code") `AUDIO[m]` extraction
  path (staged ETSI TS 102 114 V1.3.1 Annex D §D.5.1/§D.5.3/§D.5.4/
  §D.5.5 PDF p.198-201, Table 5-26 PDF p.27). New module
  `src/audio_huff.rs`.
  - Ten signed-level audio-data books transcribed verbatim: `A3`
    (§D.5.1, 3 levels), `A5`/`B5`/`C5` (§D.5.3, 5 levels),
    `A7`/`B7`/`C7` (§D.5.4, 7 levels), `A9`/`B9`/`C9` (§D.5.5,
    9 levels). Unlike the §5.4.1 side-info books (unsigned indices),
    these decode to **signed** mid-tread quantizer levels
    (`0, 1, -1, 2, -2, …`) — the `AUDIO[m]` value §5.5 scales by
    `rScale`.
  - `AudioHuffCodebook` typed code book with `abits()` / `levels()`
    accessors and `from_abits_sel(abits, sel)` resolving a
    `(ABITS, SEL)` pair through the Table 5-26 `SEL`-column order
    (terminal `V…` block-code entry and out-of-family pairs return
    `None`).
  - `decode_audio_huff_at(bytes, bit_offset, codebook)` — single
    `AUDIO[m]` symbol decode returning `(signed_level, bits_consumed)`,
    reusing the existing `Error::{HuffmanDecodeFailed, UnexpectedEof}`
    variants (no new error added).
  - 12 in-module tests: per-book printed-code round trip over every
    symbol, Kraft-equality + prefix-freeness of all ten complete codes,
    `from_abits_sel` Table 5-26 resolution (including terminal-SEL
    `None`), level/family accessors, unaligned-offset decode, EOF on a
    truncated read, and an exhaustive 6-bit-prefix resolve check.
  - Higher §D.5 audio-data families (13/17/25/33/65/129-level) and the
    §5.5 per-subsubframe `Audio Data` walker that dispatches into this
    decoder remain follow-ups.

- Round 309 (2026-06-15) — Annex D §D.6 Block Code Books + the §C.2.1
  table-look-up block-code decoder variant (staged ETSI TS 102 114
  V1.3.1 Annex D §D.6 PDF p.231-236, Annex C §C.2.1 PDF p.182-183).
  New module `src/d6_block_book.rs` closes the round-232 follow-up: the
  §C.2.1 table-look-up decoder that round 232 left blocked on the §D.6
  code-book rows (enumerated as the §C.2.1 Table C-1).
  - Seven §D.6 4-element block code books transcribed verbatim:
    `D6_BOOK_3` / `D6_BOOK_5` / `D6_BOOK_7` / `D6_BOOK_9` /
    `D6_BOOK_13` / `D6_BOOK_17` / `D6_BOOK_25` (`3/5/7/9/13/17/25`
    levels, §D.6.1-§D.6.7). Each is the closed form
    `code(element e, level L) = L · nNumLevel^(e-1)` the printed tables
    tabulate, with every printed anchor cell asserted against the
    constructed row. The §D.6.3 (7-level) 3rd-element level-0 print
    cell reads "47" in the PDF where the table's own `L·49` arithmetic
    and the §C.2.1 modulus decoder both give 147; the constructed book
    carries 147 and a test records the print erratum.
  - `D6BlockBook` typed code book with `levels()` and
    `code_value(element, level_index)` accessors; `d6_book_for_levels`
    resolves a level count to the matching §D.6 book; `D6_BLOCK_ELEMENTS`
    (= 4) names the fixed §D.6 block width.
  - `decode_block_code_table(code, book, output)` — the §C.2.1
    table-look-up decoder: walks the book last-element-first,
    subtracting the largest code value ≤ the residual and recording its
    quantisation index, with the same `nCode == 0` success criterion
    surfaced as `Error::BlockCodeResidual`. Reuses the round-232
    `Error::{BlockCodeResidual, BlockCodeLevelsOutOfRange}` variants
    (no new error added).
  - 21 in-module tests: per-book printed-anchor transcription checks
    (all seven §D.6 sub-clauses, including the 147 erratum), the
    `code_value` bounds, `d6_book_for_levels` resolution, the §C.2.1
    worked example (`code 64 → [0, -1, 0, +1]`), all-zero / max-code
    edge cases, residual + too-many-elements + empty-output rejections,
    full-domain table-vs-modulus cross-validation for 3/5/7-level books
    (3^4 / 5^4 / 7^4 codes) and strided cross-validation for
    9/13/17/25-level books, the in-alphabet index invariant, and the
    `D6_BLOCK_ELEMENTS` constant (490 → 511 lib tests).
  - New crate-root re-exports: `oxideav_dts::{decode_block_code_table,
    d6_book_for_levels, D6BlockBook, D6_BLOCK_ELEMENTS, D6_BOOK_3,
    D6_BOOK_5, D6_BOOK_7, D6_BOOK_9, D6_BOOK_13, D6_BOOK_17,
    D6_BOOK_25}`. The `--no-default-features --lib` standalone build
    still passes (the module has no `oxideav-core` dependency).

  No external library source consulted. No web search. Wall respected
  per IMPLEMENTOR_ROUND.md guardrails. Trace material read: ETSI TS 102
  114 V1.3.1 Annex D §D.6 (staged PDF p.231-236) and Annex C §C.2.1
  (staged PDF p.182-183); no other section, no other docs file.

- Round 306 (2026-06-15) — §5.5 Table 5-29 `DSYNC` subsubframe
  synchronization check word (staged ETSI TS 102 114 V1.3.1 Table 5-29
  pseudocode PDF p.32, prose PDF p.33). New module `src/dsync.rs` lands
  the trailer step of the per-subsubframe `Audio Data` walk.
  - `dsync_present(n_subsubframe, n_ssc, aspf)` — the verbatim §5.5
    gating predicate `(nSubSubFrame == nSSC-1) || (ASPF == 1)`: a DSYNC
    word follows the last subsubframe of every subframe (regardless of
    `ASPF`) and follows every subsubframe when `ASPF == 1`. Composes the
    round-281 `aspf` header field with the round-249
    `SubsubframeCount::n_ssc` count.
  - `decode_dsync_at(bytes, bit_offset, n_subsubframe)` — reads the
    16-bit `ExtractBits(16)` field MSB-first and verifies it against
    `0xffff`, returning the bits consumed (16) on success.
  - `DSYNC_WORD` (= `0xffff`) and `DSYNC_WIRE_BITS` (= 16) constants.
  - New `Error::DsyncMismatch { found, n_subsubframe }` variant
    surfacing the spec's `"DSYNC error at end of subsubframe #%d"`
    condition as a recoverable typed error (the registry layer maps it
    to `CoreError::InvalidData`).
  - 14 in-module tests: ASPF-clear last-subsubframe-only gating,
    ASPF-set every-subsubframe gating, single-subsubframe always-present,
    an exhaustive `(n_subsubframe, n_ssc, aspf)` gating matrix, the
    degenerate `n_ssc == 0` no-underflow guard, byte-aligned /
    non-aligned / byte-boundary-crossing valid reads, mismatch +
    zero-word rejections carrying the bad word and index, two EOF paths,
    and an exact-16-bits-consumed check (476 → 490 lib tests).
  - New crate-root re-exports: `oxideav_dts::{decode_dsync_at,
    dsync_present, DSYNC_WIRE_BITS, DSYNC_WORD}`. The
    `--no-default-features --lib` standalone build still passes.

- Round 300 (2026-06-14) — §5.5 Table 5-29 `Audio Data` quantization-
  type dispatch + Table 5-26 `(ABITS, SEL)` codebook-group geometry
  (staged ETSI TS 102 114 V1.3.1 Table 5-26 PDF p.27, §5.5 Table 5-29
  PDF p.31-32). New module `src/audio_data.rs` lands the decision core
  of the per-subsubframe audio-data array decode: the `nQType`
  resolver that routes each subband's eight `AUDIO[m]` indices into
  one of the four already-landed extraction paths.
  - `QUANT_LEVELS` (Table 5-26 "Number of Index Quantization Levels",
    `ABITS 0..=11`) and `CODEBOOK_GROUP_SIZE` (the per-`ABITS` `nNumQ`
    code-book group size). `ABITS_TABLE_LEN` (= 12), `ABITS_MAX_SEL`
    (= 11), `ABITS_MAX_BLOCK_CODE` (= 7).
  - `AudioQuantType` enum (`NoBits` / `Huffman` / `NoEncoding` /
    `BlockCode` for `nQType` 0/1/2/3) and `audio_quant_type(abits,
    sel)`, the verbatim §5.5 resolver: Huffman by default;
    `sel == nNumQ - 1` selects the group's terminal entry → block code
    (`ABITS <= 7`) or no-further-encoding (`ABITS >= 8`); `ABITS == 0`
    → no bits; `ABITS > 11` → no further encoding (no SEL transmitted).
  - `terminal_sel_index(abits)` exposes the §5.5 `nNumQ - 1` top valid
    `SEL` index per group.
  - 13 in-module tests: Table 5-26 levels + group sizes row-by-row, the
    exhaustive `(ABITS, SEL)` dispatch matrix against the spec
    pseudocode, the terminal-SEL block/NFE split, the `ABITS > 11`
    no-encoding tail, and the constant bounds. 463 → 476 lib tests.

- Round 293 (2026-06-14) — §D.2 quantization step-size tables + §5.5
  inverse-quantization scale composition (staged ETSI TS 102 114
  V1.3.1 Annex D §D.2.1 / §D.2.2, PDF p.193-194, and §5.5 Table 5-29
  `Audio Data`, PDF p.31-32). New module `src/step_size.rs` is the
  dequantization bridge from the §C.2.1 / Annex D Huffman `AUDIO[m]`
  quantization indices to the §C.2.2 / §C.2.5 reconstruction inputs.
  - `STEP_SIZE_LOSSY` (§D.2.1) and `STEP_SIZE_LOSSLESS` (§D.2.2),
    32-entry `Step-size × 2²²` integer tables indexed by `ABITS`
    (indices `0..=26` defined; `27..=31` reserved). `StepSizeTable`
    enum with `for_rate(rate)` (the §5.5 `RATE == 0x1f` lossless
    selector) and `step_size(abits)` (undoes the `× 2²²` scaling).
  - `transient_scale_index(tmode, n_ssc, subsubframe)` — the §5.5
    `nTmode == 0 → nSSC` / pre-vs-post-transient scale-factor split.
  - `dequant_scale(table, abits, scale, adj)` — the §5.5
    `rScale = rStepSize · SCALES · arADJ` composition (`adj` reuses
    the round-241 `ScaleFactorAdjustment`).
  - `scale_subsubframe_samples(audio, r_scale, out)` — the §5.5
    eight-sample `aSample[m] = rScale · AUDIO[m]` scaling.
  - `dequant_subsubframe(side, n, n_ssc, subsubframe, table, adj,
    audio, out)` — fused end-to-end one-subsubframe driver reading
    `abits` / `tmode` / `scales` off the round-281 `ChannelSideInfo`.
  - New `Error::InvalidStepSize { abits }` (reserved/out-of-range
    `ABITS`) and `Error::SampleCountMismatch { expected, found }`
    (non-eight-sample subwindow), both mapped to `CoreError::InvalidData`.
  - 14 in-module tests cross-check the §D.2 nominal column, the
    transient split, the zero-step `ABITS 0` path, and the
    end-to-end driver (449 → 463 lib tests).

- Round 286 (2026-06-13) — fused 32-band synthesis QMF driver
  (§C.2.5 `QMFInterpolation()` per-channel outer loop, staged ETSI
  TS 102 114 V1.3.1 Annex C §C.2.5, PDF p.185 / `dts-core-extracts.md`
  §2.4). New module `src/qmf_synth.rs` composes the previously-landed
  FIR-independent per-sample primitives — `assemble_xin` (step a),
  `cos_mod_stage` (step b), `fir_step` (step c), `write_pcm_output`
  (step d), and `shift_x_history` / `shift_z_output` (step e) — into
  the complete §C.2.5 outer loop.
  - New `QmfSynthesis` per-channel filter object owns the persistent
    §C.2.5 `raX[]` (512-tap shift register) and `raZ[]` (64-entry
    output accumulator) state plus a precomputed 544-entry
    `raCosMod[]` matrix; `QmfSynthesis::new` clears the history
    (matching the per-channel filter's pre-first-subframe state).
  - `QmfSynthesis::synthesize(subband_samples, n_subs, filter,
    r_scale, output)` runs the per-sample loop over one block of
    subband sample rows (one `f64`-per-subband vector per
    `nSubIndex`), appending exactly 32 reconstructed PCM samples per
    row to `output`, and persists the updated `raX[]` / `raZ[]` for
    the channel's next subframe. The §D.8 `prCoeff` table is selected
    once from the resolved `FILTS` branch
    (`FilterBankSelection::coefficients`) and threaded into every
    per-sample FIR step, exactly as the spec hoists the
    `prCoeff = …` assignment out of the loop. `n_subs > 32` surfaces
    as `QmfAssembleError::SubsOutOfRange` before any sample runs.
  - `x_history()` / `z_accumulator()` borrow the live `raX[]` /
    `raZ[]` state for checkpoint/inspection.
  - New crate-root re-export `oxideav_dts::QmfSynthesis`.
  - Eight in-module tests: cleared-history construction; 32-PCM-per-
    row count; all-zero-input → all-zero-PCM for both filter
    selections; fused driver byte-identical to a hand-composed
    per-sample loop over an impulse input (pins the composition as
    faithful with no hidden reordering); split-call equals single
    concatenated call (inter-subframe `raX[]` tail carries across
    `synthesize` calls); `n_subs > 32` rejection with untouched
    state; empty-input no-op; and `n_subs = 0` full-silence.
  - The §C.2.5 `rScale` output multiplier stays a caller-supplied
    parameter — the staged clause uses `rScale` in the PCM step
    without assigning it inside `QMFInterpolation()`, so its
    derivation is a documented docs gap (carried from the round-255
    `write_pcm_output` note).
- Round 281 (2026-06-12) — §5.4.1 Primary Audio Coding Side
  Information subframe walker + TMODE codebooks: composes the
  round-249 SSC/PSC prefix, the round-195 ABITS / SCALES decoders,
  and the new TMODE decoder into the §5.4.1 Table 5-28 decode walk
  (staged ETSI TS 102 114 V1.3.1 PDF p.28-29).
  - New module `src/subframe.rs` with
    `decode_primary_side_info_at(bytes, bit_offset, params)`
    walking, in Table 5-28's exact field order: the SSC/PSC prefix,
    the PMODE plane (1 bit per `(ch, n)`), the PVQ plane (12-bit
    `nVQIndex` per PMODE-active subband; the clause D.10.1
    coefficient lookup is deferred and the raw index is captured),
    the ABITS plane (`BHUFF[ch]` codebook, `n < nVQSUB[ch]`), the
    TMODE plane (decoded only when `nSSC > 1` and `ABITS > 0`), and
    the SCALES plane (per-channel `nScaleSum = 0` reset, transient
    second factor where `TMODE > 0`, high-frequency-VQ tail loop on
    the same running accumulator). Returns a typed
    `PrimarySideInfo` / `ChannelSideInfo` (fixed 32-slot planes)
    plus the consumed-bit count; the cursor lands on the un-walked
    `JOIN_SHUFF`-onward tail (blocked on the clause D.4 table).
  - New `ChannelSideInfoParams` input struct (`nSUBS` / `nVQSUB`
    bounds + codebook selectors, resolved by the caller from the
    §5.3.2 Table 5-21 header) and `MAX_PRIMARY_CHANNELS = 5`
    (§5.3.2 `nPCHS = PCHS + 1 ≤ 5`, PDF p.25); bound violations
    surface as `Error::InvalidSideInfo` with fields `"nPCHS"` /
    `"nSUBS"` / `"VQSUB"` before any bit is read.
  - New `TmodeCodebook` selector (§5.3.2 Table 5-23, PDF p.26;
    total over the 2-bit `THUFF[ch]` wire field) and
    `decode_tmode_at` single-field decoder backed by the Annex D
    §D.5.2 "4 Levels (For TMODE)" Huffman codebooks A4/B4/C4/D4
    transcribed verbatim from the staged PDF p.198.
  - Nineteen new in-module tests (six TMODE-side, thirteen
    walker-side); total lib test count 422 → 441.
- Round 278 (2026-06-11) — §D.8 32-band interpolation FIR
  coefficient tables + `fir_step()`: closes round-208 docs gap #9
  by transcribing the two 512-tap `prCoeff` sets from the staged
  ETSI TS 102 114 V1.3.1 Annex D §D.8 "32-Band Interpolation and
  LFE Interpolation FIR" table
  (`docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`
  p.238-246) and landing the §C.2.5 `QMFInterpolation()` FIR
  convolution step that consumes them
  (`docs/audio/dts/dts-core-extracts.md` §2.4).
  - New module `src/fir_coeff.rs` with
    `RA_COEFF_LOSSLESS: [f64; 512]` (the §D.8 "Perfect
    Reconstruction" column — the pseudocode's `raCoeffLossLess`,
    selected by `FILTS != 0`) and `RA_COEFF_LOSSY: [f64; 512]`
    (the "Non-Perfect Reconstruction" column — `raCoeffLossy`,
    `FILTS == 0`), both transcribed verbatim (the spec table's
    decimal commas rendered as decimal points), plus
    `FIR_COEFF_LEN = 512`. The §D.8 LFE columns (64x / 128x
    interpolation) drive the LFE reconstruction path and stay out
    of scope until that path lands.
  - `FilterBankSelection::coefficients() -> &'static [f64; 512]`
    resolves the round-263 typed selector to the matching §D.8
    table, completing the spec's two-line `prCoeff` assignment.
  - `fir_step(&ra_x, pr_coeff, &mut ra_z)` executes the §C.2.5
    "Multiply by filter coefficients" step: both
    `for (k=31,i=0; i<32; i++,k--) for (j=0; j<512; j+=64)`
    loops, accumulating `prCoeff[i+j]*(raX[i+j]-raX[j+k])` into
    `raZ[0..32]` and `prCoeff[32+i+j]*(-raX[i+j]-raX[j+k])` into
    `raZ[32..64]` (each of the 512 coefficients consumed exactly
    once per call, 8 taps per output slot).
  - 16 new in-module tests: verbatim anchor rows read
    independently off both sides of every staged-PDF page seam
    (p.238/239/.../246) plus the first / centre / last rows; exact
    whole-table antisymmetry (`coeff[i] == -coeff[511-i]`, both
    sets); finite/bounded values with the magnitude peak at the
    255/256 centre; distinct-set check; the `FIR_COEFF_LEN` bound;
    pointer-identity + `from_filts`-composition checks for
    `coefficients()`; and for `fir_step` a bit-exact
    line-for-line §C.2.5 reference comparison (ramp / alternating
    / two pseudo-random registers × both §D.8 sets),
    silent-register no-op, accumulate-not-overwrite
    (reference-matched from a preloaded accumulator plus an exact
    dyadic single-tap check), low-/high-half single-tap index
    mapping, the 8-taps-per-output count, and exact power-of-two
    linearity in the shift register.
  - New re-exports at the crate root: `oxideav_dts::{fir_step,
    FIR_COEFF_LEN, RA_COEFF_LOSSLESS, RA_COEFF_LOSSY}`. Total
    in-module test count: 406 → 422 (`cargo test -p oxideav-dts
    --lib`).
  - With this round every per-sample step of the §C.2.5 loop body
    exists as a public primitive (assemble → cos-mod → FIR → PCM
    out → raX/raZ shifts); the remaining blockers for the fused
    `QMFInterpolation()` driver are the output `rScale` value and
    the `multirate_inter ↔ FILTS` polarity (both still docs
    gaps).

  No external library source consulted. No web search. Wall
  respected per IMPLEMENTOR_ROUND.md guardrails. Trace material
  read: ETSI TS 102 114 V1.3.1 Annex D §D.8 (staged PDF p.236-247
  window, §D.8 table rows 0-511) and Annex C §C.2.5 as transcribed
  in `docs/audio/dts/dts-core-extracts.md` §2.4; no other docs
  file.

- Round 274 (2026-06-11) — `write_pcm_output()`: the
  FIR-independent PCM-output step of the §C.2.5
  `QMFInterpolation()` per-sample loop body (ETSI TS 102 114 V1.3.1
  Annex C §C.2.5, staged PDF p.185, per
  `docs/audio/dts/dts-core-extracts.md` §2.4 lines 213-214). Lands
  the loop-body step that turns the synthesis filter's accumulated
  output into integer PCM, without depending on the §D.8 FIR
  coefficient tables (still pending docs staging, round-208 docs
  gap #9 / OxideAV-docs issue #1357).
  - `write_pcm_output(&[f64; 64], r_scale, &mut [i32], n_ch_index)`
    executes the spec's `for (i=0; i<32; i++) naCh[nChIndex++] =
    int(rScale*raZ[i]);`: it consumes the 32 low entries
    `raZ[0..32]` (the FIR step accumulated them for the current
    per-sample iteration), scales each by the per-channel `rScale`
    multiplier, applies the spec's `int()` truncate-toward-zero
    cast (`f64::trunc` then `as i32`), and writes the 32 integer
    samples into the channel buffer at the running `nChIndex`
    cursor. Returns the advanced cursor (`n_ch_index + 32`),
    mirroring the spec's `naCh[nChIndex++]` post-increment. Reads
    only `raZ[0..32]`; the high block `raZ[32..64]` (next
    iteration's pre-rotate partials) is never read. Reads no §D.8
    coefficients.
  - `rScale` is taken as a caller-supplied parameter: the §C.2.5
    pseudocode uses `rScale` in the PCM-output step without
    assigning it inside the `QMFInterpolation()` block, and the
    staged §C.2.5 clause does not fix the value/derivation of the
    QMF-output `rScale` (distinct from the per-subband
    reconstruction `rScale` defined elsewhere in the spec). This is
    a docs gap for the *value* of the output multiplier; the
    *structure* of the step (scale → `int()` → write 32 → advance
    cursor) is fully defined, so the step ships parametrically. The
    §C.2.5 clause states no per-width saturation in this step;
    clamping to the transmitted source-PCM resolution
    (`SourcePcmResolution`) is a separate output-format step the
    clause does not define here.
  - New `QmfAssembleError::OutputSliceTooShort { n_ch_index,
    available }` for a channel buffer with no room for the 32
    samples at the cursor.
  - New public re-exports at the crate root:
    `oxideav_dts::{write_pcm_output, PCM_OUTPUT_PER_SAMPLE}`.
    `PCM_OUTPUT_PER_SAMPLE = 32` (= `NUM_SUBBAND`) names the
    per-iteration output count the §2.4 line 213-214 `i < 32` bound
    fixes.
  - 11 new unit tests covering: the 32-sample emit + cursor
    advance; scale-before-cast; truncate-toward-zero for both signs
    and sub-unit magnitudes; scale-then-truncate ordering;
    write-at-running-cursor with untouched neighbour blocks;
    low-block-only read (no high-accumulator leak); buffer-too-short
    and cursor-past-room rejections; negative-scale sign flip; the
    `PCM_OUTPUT_PER_SAMPLE` constant; and the error Display message.
    Total in-module test count: 395 → 406 (`cargo test -p
    oxideav-dts --lib`).

  No external library source consulted. No web search. Wall
  respected per IMPLEMENTOR_ROUND.md guardrails. Trace material
  read: ETSI TS 102 114 V1.3.1 Annex C §C.2.5 only
  (`docs/audio/dts/dts-core-extracts.md` §2.4 lines 213-214, staged
  PDF p.185); no other section, no other docs file.

- Round 271 (2026-06-10) — `shift_z_output()`: the FIR-independent
  post-PCM rotate of the 64-entry `raZ[]` output accumulator, the
  last index-only step of the §C.2.5 `QMFInterpolation()` per-sample
  loop body (ETSI TS 102 114 V1.3.1 Annex C §C.2.5, staged PDF
  p.185, per `docs/audio/dts/dts-core-extracts.md` §2.4 lines
  218-219). Completes the three FIR-independent loop-body steps
  (raXin assembly, raX shift, raZ rotate) without depending on the
  §D.8 FIR coefficient tables (still pending docs staging, round-208
  docs gap #9 / OxideAV-docs issue #1357).
  - `shift_z_output(&mut [f64; 64])` executes the spec's two
    sequential loops `for (i=0; i<NumSubband; i++) raZ[i] =
    raZ[i+32];` then `for (i=0; i<NumSubband; i++) raZ[i+32] = 0.0;`:
    it slides the high block `raZ[32..64]` (the FIR step accumulated
    it for the next sample's PCM output) down into `raZ[0..32]` and
    zero-fills the freed high block. The down-shift iterates forward
    (unlike `shift_x_history()`'s reverse walk) because the source
    range `[32, 64)` and destination range `[0, 32)` are disjoint.
    Reads no §D.8 coefficients — pure index manipulation.
  - New public re-exports at the crate root:
    `oxideav_dts::{shift_z_output, Z_OUTPUT_LEN}`. `Z_OUTPUT_LEN = 64`
    (= `2 * NUM_SUBBAND`) is the accumulator length the §2.4 line
    218-219 `raZ[i]` / `raZ[i+32]` indexing implicitly fixes.
  - 8 new unit tests covering: high-to-low block move; high-block
    zero-fill; +0.0 (not -0.0) bit-pattern of the cleared block;
    no leak-through of the prior low-block content; all-zero no-op;
    bit-identical pass-through of signed and subnormal high-block
    values; a two-rotate composition exposing a simulated
    inter-sample FIR refill; and the length constants
    (`Z_OUTPUT_LEN = 64`, `Z_OUTPUT_LEN = 2 * NUM_SUBBAND`). Total
    in-module test count: 387 → 395 (`cargo test -p oxideav-dts
    --lib`).

  No external library source consulted. No web search. Wall
  respected per IMPLEMENTOR_ROUND.md guardrails. Trace material
  read: ETSI TS 102 114 V1.3.1 Annex C §C.2.5 only
  (`docs/audio/dts/dts-core-extracts.md` §2.4 lines 218-219, staged
  PDF p.185); no other section, no other docs file.

- Round 263 (2026-06-09) — `FilterBankSelection`: typed selector
  for the §C.2.5 `QMFInterpolation()` 512-tap FIR coefficient set
  (ETSI TS 102 114 V1.3.1 Annex C §C.2.5, staged PDF p.185, per
  `docs/audio/dts/dts-core-extracts.md` §2.4 lines 175-178).
  Lands the receiving side for the §C.2.5 `FILTS` parameter
  without consuming any §D.8 coefficient values (round-208 docs
  gap #9 / OxideAV-docs issue #1357 still open), so the selection
  ships ahead of the FIR step it parametrises.
  - `FilterBankSelection::NonPerfectReconstruction` names the
    §D.8 `raCoeffLossy` 512-tap set; `FilterBankSelection::PerfectReconstruction`
    names the §D.8 `raCoeffLossLess` 512-tap set. Marked
    `#[non_exhaustive]` so future additional reconstruction modes
    extend without breaking match arms.
  - `FilterBankSelection::from_filts(filts: u8) -> Self` mirrors
    the spec's `if (FILTS==0) prCoeff = raCoeffLossy; else
    prCoeff = raCoeffLossLess;` branch — `0` picks the
    non-perfect variant, every non-zero `u8` picks the perfect
    variant (matching the spec's collapsed `else` semantics).
  - `FilterBankSelection::filts(self) -> u8` returns the
    canonical inverse (`0` / `1`).
  - `FilterBankSelection::spec_table_name(self) -> &'static str`
    returns the verbatim §C.2.5 pseudocode identifier
    (`"raCoeffLossy"` / `"raCoeffLossLess"`) for diagnostics.
  - The `DtsFrameHeader::multirate_inter` docstring is updated to
    point callers at the new enum and to record the still-open
    polarity gap (`multirate_inter ↔ FILTS` mapping not yet
    documented under `docs/audio/dts/`). No `DtsFrameHeader`
    accessor is added until that polarity lands.
  - New public re-export at the crate root:
    `oxideav_dts::FilterBankSelection`.
  - 11 new unit tests covering: `from_filts(0)` →
    `NonPerfectReconstruction`; `from_filts(1)` →
    `PerfectReconstruction`; every non-zero `u8` value picks the
    lossless variant (255-iteration sweep); round-trips of both
    variants through `filts()`; spec-table-name verbatim match;
    table-name distinctness; copy / equality / hash behaviour;
    stable Debug output. Total in-module test count:
    375 → 386 (`cargo test -p oxideav-dts --lib`).

- Round 259 (2026-06-08) — `assemble_xin()` + `shift_x_history()`:
  the FIR-independent per-sample raXin assembly and raX
  shift-register update steps of `QMFInterpolation()` (ETSI TS 102
  114 V1.3.1 Annex C §C.2.5, staged PDF p.185, per
  `docs/audio/dts/dts-core-extracts.md` §2.4 lines 182-183 and 217).
  Bracket round-255's `cos_mod_stage()` inside the synthesis QMF's
  per-sample loop body without depending on the §D.8 FIR coefficient
  tables (still pending docs staging, round-208 docs gap #9), so
  they ship ahead of the full driver.
  - `assemble_xin(subband_samples, n_subs) -> Result<[f64; 32], _>`
    builds the per-sample input vector `cos_mod_stage()` consumes
    by copying `subband_samples[0..n_subs]` into the leading active
    slots and leaving the inactive tail `n_subs..32` at +0.0. Validates
    `n_subs <= 32` (the spec's `NumSubband = 32` cap) and that the
    caller supplied at least `n_subs` per-subband scalars; returns a
    new `QmfAssembleError` enum (`SubsOutOfRange` / `SampleSliceTooShort`)
    on either violation.
  - `shift_x_history(&mut [f64; 512])` rotates the 512-entry `raX[]`
    register by 32 entries toward the high end, matching the spec's
    reverse-iteration shift `for (i=511; i>=32; i--) raX[i] = raX[i-32];`.
    Leaves `raX[0..32]` untouched (the driver overwrites that range
    from the next per-sample `cos_mod_stage()` output before the FIR
    step reads it).
  - New public re-exports at the crate root:
    `oxideav_dts::{assemble_xin, shift_x_history, QmfAssembleError,
    X_HISTORY_LEN}`. `X_HISTORY_LEN = 512` is the spec's implicit
    `raX[]` length (the upper bound of the §2.4 line-217 shift loop,
    matching the 512-tap §D.8 FIR set).
  - 20 new unit tests covering: full-active / zero-active /
    partial-active assembly, ignoring trailing samples past `nSUBS`,
    out-of-range rejection, short-slice rejection, exact-length
    boundary, bit-identical pass-through for signed and subnormal
    f64s, positive-zero invariant in the inactive tail, the shift's
    move-by-32 semantics, the untouched low block, identity on
    uniform / zero registers, top-block contents verification, the
    reverse-iteration anti-pattern check (forward iteration would
    collapse `raX[k*32]` slots to `raX[0]`), repeated-shift
    composition, the length constants (`X_HISTORY_LEN = 512`,
    `X_HISTORY_LEN % NUM_SUBBAND == 0`), and human-readable error
    rendering. All run on stable IEEE-754 `f64`; the assembly is a
    `copy_from_slice` so it inherits memcpy semantics directly.

- Round 255 (2026-06-08) — `cos_mod_stage()` cosine-modulation stage
  of `QMFInterpolation()` (ETSI TS 102 114 V1.3.1 Annex C §C.2.5,
  staged PDF p.185, per `docs/audio/dts/dts-core-extracts.md` §2.4).
  Lands the FIR-independent first half of the 32-band synthesis QMF's
  per-sample loop body: given the per-sample subband vector
  `raXin[0..32]` and the round-208 `precal_cos_mod()` matrix, returns
  the 32 leading entries `raX[0..32]` the spec writes into the
  synthesis filter's shift register before the 512-tap FIR
  convolution. The function consumes only the cosine-modulation
  matrix — no §D.8 `raCoeffLossy` / `raCoeffLossLess` tables (still
  pending docs staging, round-208 docs gap #9) — so it ships ahead of
  the full `QMFInterpolation()` driver.
  - Substep 1 builds the 16-entry `A[k]` and `B[k]` accumulators
    from `raCosMod` Block 1 (`cos((2i+1)(2k+1)π/64)`, indices
    `0..256`) and Block 2 (`cos(i(2k+1)π/32)`, indices `256..512`),
    using the spec's asymmetric `B[k]` accumulation that pairs
    `raXin[2i] + raXin[2i-1]` for `i > 0` and falls back to
    `raXin[0]` at `i = 0`.
  - Substep 2 forms `SUM[k] = A[k] + B[k]` and `DIFF[k] = A[k] -
    B[k]` (fused with substep 3 in the live implementation to avoid
    materialising the intermediates).
  - Substep 3 places `raX[k] = raCosMod[Block3 + k] * SUM[k]` for
    `k = 0..16` (Block 3 scaling, indices `512..528`) and
    `raX[32 - k - 1] = raCosMod[Block4 + k] * DIFF[k]` (Block 4
    scaling, indices `528..544`). The spec's running `j`-counter
    walks `0..544` across the whole stage, matching the j value
    handed to the FIR step that follows.
  - New public re-exports at the crate root: `oxideav_dts::{cos_mod_stage,
    NUM_SUBBAND}`. `NUM_SUBBAND = 32` is the spec's `NumSubband`
    constant for the 32-band synthesis QMF (§C.2.5).
  - Nine new in-module tests in `src/cos_mod.rs` exercising the
    stage: a zero-input zero-output check; a bit-exact match against
    a verbatim line-for-line reference implementation on zero,
    32-impulse, ramp, and alternating-sign inputs; a finite-output
    check on a `sin`-driven input; a linearity check
    (`cos_mod_stage(2x) == 2 * cos_mod_stage(x)`); a determinism
    check; and a `NUM_SUBBAND == 32` constant check. Total
    cos_mod-module test count: 20 → 29.

- Round 249 (2026-06-07) — SSC / nSSC / PSC → Subsubframe-Count
  prefix at the head of §5.4.1 Table 5-28 (ETSI TS 102 114 V1.3.1,
  staged PDF p.28, with field descriptions on p.29 and p.30). Wires
  the first two `ExtractBits` reads of the Primary Audio Side
  Information pseudocode (`SSC = ExtractBits(2); nSSC = SSC + 1;
  PSC = ExtractBits(3);`) to a typed 5-bit prefix decoder.
  - New `SubsubframeCount` struct (`#[non_exhaustive]`) carrying
    the raw `ssc` (2 bits, `0..=3`) and `psc` (3 bits, `0..=7`)
    fields. Accessors: `n_ssc(self) -> u8` (= `ssc + 1`, `1..=4`,
    per PDF p.29), `samples_per_subsubframe_normal(self) -> usize`
    (= `8 * nSSC`, the per-subband sample stride consumed by the
    §C.2.3 / §C.2.4 / §C.2.5 loops in `sum_diff.rs` and
    `joint_subband.rs`), `partial_sample_count(self) -> Option<u8>`
    (`Some(psc)` when `psc > 0`, `None` otherwise), and
    `is_termination_tail(self) -> bool` (returns `true` when
    `psc != 0`, the termination-frame signal per PDF p.30).
    Associated constants `MAX_SSC = 0b11`, `MAX_PSC = 0b111`, and
    `WIRE_BITS = 5`. Constructor `new(ssc, psc)` masks inputs to
    their 2-bit / 3-bit wire widths to match the `ExtractBits`
    semantics.
  - New `decode_subsubframe_count_at(bytes: &[u8], bit_offset:
    usize) -> Result<(SubsubframeCount, usize)>` bit-stream entry
    point that reads the 5-bit prefix at an arbitrary MSB-first
    bit offset and returns `(prefix, bits_consumed)`. Returns
    `Error::UnexpectedEof` when fewer than 5 bits remain after
    `bit_offset`.
  - New re-exports at the crate root:
    `oxideav_dts::{SubsubframeCount, decode_subsubframe_count_at}`.

  Ten new in-module unit tests in `src/side_info.rs`: a four-row
  sweep verifying `nSSC = SSC + 1`; a four-row sweep verifying the
  `8 * nSSC` accessor; a high-bit-masking check (`0xFF`,
  `0b1111_1101`, `0b1111_1010`); a `psc = 0..=7` sweep covering
  both `partial_sample_count` arms and `is_termination_tail`; a
  `WIRE_BITS == 5` constant assertion; a byte-aligned
  `decode_subsubframe_count_at` walk at bit-offset 0; a
  non-byte-aligned walk at bit-offset 3; a byte-boundary-crossing
  walk at bit-offset 5 splitting the 5-bit prefix across two
  bytes; an `UnexpectedEof` check when only 4 bits remain; and an
  exhaustive `4 × 8 = 32` `(SSC, PSC)`-pair walk asserting every
  accessor for every combination. Total in-module test count:
  336 → 346.

- Round 244 (2026-06-07) — ADJ → Scale Factor Adjustment multiplier
  (ETSI TS 102 114 V1.3.1 §5.4.1 Table 5-27, staged PDF p.27). Wires
  the Core Audio Coding Header pseudocode `ADJ = ExtractBits(2);`
  field (Table 5-21, PDF p.25) to its four-row multiplier table.
  - New `ScaleFactorAdjustment` enum (`Adj0..=Adj3`, `#[non_exhaustive]`)
    with `from_index(adj: u8) -> Self` (2-bit-masking, total over
    `0..=3`), `code(self) -> u8` (inverse of `from_index`),
    `multiplier(self) -> f32` returning the Table 5-27 values
    `1.0000`, `1.1250`, `1.2500`, `1.4375`,
    `multiplier_f64(self) -> f64` (same values, exactly
    representable), and `multiplier_rational(self) -> (u8, u8)`
    returning the numerator-over-16 exact rational form
    (`(16, 16)`, `(18, 16)`, `(20, 16)`, `(23, 16)`).
  - New `decode_adj_at(bytes: &[u8], bit_offset: usize) ->
    Result<(ScaleFactorAdjustment, usize)>` bit-stream entry
    point that reads the 2-bit `ADJ` field at an arbitrary
    MSB-first bit offset and returns `(adjustment, bits_consumed)`.
    Returns `Error::UnexpectedEof` when fewer than 2 bits remain
    after `bit_offset`.
  - New re-exports at the crate root:
    `oxideav_dts::{ScaleFactorAdjustment, decode_adj_at}`.

  Eight new in-module unit tests in `src/side_info.rs` lock the
  table down: a row-by-row sweep across all four `(ADJ, variant,
  value)` rows asserting `from_index`, `code` round-trip,
  `multiplier` (`f32`), and `multiplier_f64`; a high-bit-masking
  check (`0xFF`, `0xFC`, `0b1111`, `0b1100`); a rational-accessor
  check confirming all four `(numerator, 16)` pairs equal the
  `f32` multiplier exactly; a byte-aligned `decode_adj_at` walk
  reading four ADJ pairs packed in one byte (`0x1B`); a
  bit-offset=5 walk inside a single byte; a byte-boundary-crossing
  walk that splits the 2-bit field across two consecutive bytes;
  an `UnexpectedEof` check when only 1 bit remains; and a `code`
  round-trip check across every `0..=3` wire value. Total
  in-module test count: 328 → 336.

- Round 241 (2026-06-06) — DIALNORM / UNSPEC → Dialog Normalization
  Gain in dB (ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-20, staged PDF
  p.24). Closes the round-5 DIALNORM docs gap. The 4-bit field that
  follows SURROUND_SUM in the post-CRC header window is named
  `DIALNORM` when `VERNUM ∈ {6, 7}` and `UNSPEC` otherwise; the
  resolver routes through the parsed `version` field per §5.3.1.
  - New `DialogNormalization` enum with `Fixed(i8)` (the Table 5-20
    `VERNUM ∈ {6, 7}` rows: codes `0..=15` mapping to 0 dB down to
    −15 dB for `VERNUM == 7`; codes `0..=15` mapping to −16 dB down
    to −31 dB for `VERNUM == 6`) and `Unspecified` (every other
    `VERNUM`, where the spec sets DNG = 0 dB).
  - `DialogNormalization::gain_db() -> i8` returns the dB value (the
    contained `i8` for `Fixed`, `0` for `Unspecified`).
  - `DtsFrameHeader::dialog_normalization_gain() -> DialogNormalization`
    is the typed counterpart to the existing
    `dialog_normalization_db()`.
  - `DtsFrameHeader::dialog_normalization_db()` (which had returned
    `None` since round 5) now returns `Some(db)` across every
    reachable `(VERNUM, DIALNORM)` pair, including the UNSPEC
    branch where it resolves to `Some(0)`.

  Five new in-module tests in `src/header.rs` lock the table down:
  exhaustive sweeps of both Table 5-20 named-VERNUM rows
  (32 pairs); exhaustive sweeps of the fourteen UNSPEC-branch
  `VERNUM` values (224 pairs); boundary-corner check on the
  pure-function `dialog_normalization_from_codes` helper
  (`(7, 0)` → 0, `(7, 15)` → −15, `(6, 0)` → −16, `(6, 15)` → −31,
  plus 4-bit input-masking on both arguments); `gain_db` projection
  check across both variants; and a range-coverage cross-check
  confirming the resolver's range over all 256 `(VERNUM, DIALNORM)`
  pairs is exactly the spec's `{0, −1, …, −31}` dB set. The
  existing `dialnorm_code_round_trips_for_every_4bit_value` test
  (round 5) is updated to assert `Some(0)` in the UNSPEC branch.
  The `tests/black_box_ffmpeg.rs` fixture
  (`ffmpeg -c:a dca -ar 48000 -ac 2 -b:a 768k`,
  VERNUM=7 + DIALNORM=0) now asserts
  `dialog_normalization_db() == Some(0)`.

- Round 232 (2026-06-04) — §C.2.1 Block Code (ETSI TS 102 114
  V1.3.1 Annex C §C.2.1, PDF p.182–183). New `src/block_code.rs`
  lands the §C.2.1 modulus / integer-division block-code decoder,
  the mixed-radix unpacking step that turns one code word into the
  array of quantisation indices the §C.2.x decode chain consumes.
  Three new public entry points plus two new `Error` variants:
  - `decode_block_code(code, n_levels, &mut output)` — in-place
    decode of one block-code word. Walks the spec's
    `for n in 0..nNumElement { pnValue[n] = (nCode % nNumLevel) -
    nOffset; nCode /= nNumLevel; }` recurrence and produces the
    first-element-first quantisation-index array, returning
    `BlockCodeResidual` when the §C.2.1 success criterion
    `nCode == 0` after the last extraction is unmet.
  - `block_code_offset(n_levels)` — the spec's
    `nOffset = (nNumLevel - 1) >> 1` mid-range offset (1 for a
    3-level alphabet, 2 for 5-level, ..., 12 for 25-level). Exposed
    so callers can size index buffers and validate alphabet ranges
    by the same invariant the spec writes against.
  - `block_code_max_code(n_elements, n_levels)` — largest valid
    code word for the declared block dimensions
    (`n_levels.pow(n_elements) - 1`); returns `None` on overflow.
  - `Error::BlockCodeLevelsOutOfRange { n_levels }` — `n_levels < 2`
    rejected (the recurrence is undefined for single-element or
    zero-element alphabets).
  - `Error::BlockCodeResidual { residual, n_elements, n_levels }` —
    the spec's "ERROR: block code look-up fail" condition surfaced
    as a recoverable error.

  Seventeen new in-module unit tests in `src/block_code.rs` plus
  three doc-tests lock the decoder down: the spec's worked example
  (PDF p.182: `code=64`, `n_levels=3`, four elements →
  `[0, -1, 0, +1]`) end-to-end, the same worked example walked
  step-by-step against the spec's recorded `(rem, next_q)` table,
  the 3-level 4-element domain round-tripped exhaustively (all 81
  valid code words), the 5-level 3-element domain round-tripped
  exhaustively (all 125 code words), the alphabet-bound invariant
  (every decoded index in `[-(n_levels-1)/2, (n_levels-1)/2]`),
  the all-zero-code-word edge case (decodes to all-bottom-of-
  alphabet), the max-valid-code edge case (decodes to all-top-of-
  alphabet), the one-past-max residual error, the `n_levels < 2`
  rejection (`0` and `1`), the empty-output success / residual
  paths, the smallest 3-level 1-element alphabet, the binary
  (`n_levels = 2`) decode reading bits LSB-first, and the largest
  §D.6 25-level 1-element alphabet (offset = 12).

  Scope: this round lands the modulus / integer-division decoder
  primitive only. The §C.2.1 table look-up variant requires the
  §D.6 "rearranged" code-book rows enumerated as Table C-1 (3-level
  4-element, with parallel rearranged tables for 5/7/9/13/17/25
  levels) which are not yet transcribed into this crate. The
  dispatch wiring into a subframe walker — extracting the
  `(n_elements, n_levels)` parameters per subband from the §5.4.1
  side info — is also left to a follow-up round.

  No external library source consulted. No web search. Wall
  respected per IMPLEMENTOR_ROUND.md guardrails. Trace material
  read: ETSI TS 102 114 V1.3.1 Annex C §C.2.1 only (staged PDF
  p.182–183); no other section, no other docs file.

- Round 228 (2026-06-04) — §C.2.2 Inverse ADPCM (ETSI TS 102 114
  V1.3.1 Annex C §C.2.2, PDF p.183). New `src/inverse_adpcm.rs`
  lands the per-subband inverse-ADPCM predictor, the fixed-order
  4-tap FIR over the reconstructed signal that the §5.4.1 `PMODE`
  side-info flag gates per subband. Six new public entry points plus
  the `NumADPCMCoeff = 4` spec invariant exposed as a constant:
  - `inverse_adpcm_decode_i32(history, coeffs, samples)` /
    `inverse_adpcm_decode_f64(...)` — the §C.2.2 predictor itself.
    On entry `samples` carries the dequantised residuals; on return
    each `samples[m]` has been overwritten with
    `samples[m] + Σ_{n=0..3} coeffs[n] * past[n]` where `past[n]`
    is the sample at logical index `m - n - 1`, sourced from the
    `history` buffer for the negative-index range and from earlier
    `samples` slots for the non-negative range. The i32 variant
    uses `wrapping_add` / `wrapping_mul` to mirror the spec's C
    `int` semantics. The predictor walks strictly left-to-right
    because each freshly-written `samples[m]` is the `n = 0`
    history slot consumed at step `m + 1`.
  - `update_history_i32(history, samples)` /
    `update_history_f64(...)` — slide the rolling four-sample
    history buffer forward by the just-reconstructed block: long
    blocks (`samples.len() >= 4`) overwrite `history` with the
    final four samples; short blocks shift `history` left by
    `samples.len()` slots and append the residual. This convenience
    helper makes the spec's "It must updated each time before
    reverse ADPCM is run for a block of samples for each subband"
    sentence concrete.
  - `inverse_adpcm_required(pmode) -> bool` — the dispatch predicate
    (`pmode == 1`); the §C.2.2 narrative gates the predictor on a
    per-subband `PMODE == 1`.
  - `NUM_ADPCM_COEFF: usize = 4` — the spec's `NumADPCMCoeff`
    constant exposed for callers that want to size history /
    coefficient buffers by the same invariant the spec writes
    against.
  - One new `Error` variant: `Error::InverseAdpcmShapeMismatch
    { history_len, coeffs_len }`, fired when either the history or
    coefficient slice has any length other than 4.

  Twenty-six new in-module unit tests in `src/inverse_adpcm.rs` plus
  two doc-tests lock the predictor down: zero-coeff identity, the
  four-tap history-slot mapping (`coeff[0]` taps `raSample[-1]`,
  `coeff[3]` taps `raSample[-4]`), the freshly-written-sample-feeds-
  next-step ordering, the four-tap walk-off at `m == 4` (no
  history is consulted from sample index 4 onwards), wrapping
  arithmetic at `i32::MAX * 2` and `i32::MIN * -1`, sign correctness
  for negative coefficients, empty-block no-op, the four
  history-vs-coeffs length-mismatch error paths, the floating-point
  variant's exact-arithmetic property at `coeff = 0.5`, the
  history-update helper's three regimes (long block / exact-four /
  short block), the `pmode == 1` dispatch predicate across all 256
  possible byte values, and a two-block continuation property test
  that confirms decoding a residual stream as two consecutive
  blocks (with history slid between them) is identical to decoding
  it as a single long block.

  No external library source consulted. No web search. Wall
  respected per IMPLEMENTOR_ROUND.md guardrails. Trace material
  read: ETSI TS 102 114 V1.3.1 Annex C §C.2.2 only (PDF p.183);
  no other section, no other docs file.

- Round 223 (2026-06-03) — §C.2.3 Joint Subband Coding (ETSI
  TS 102 114 V1.3.1 Annex C §C.2.3, PDF p.184). Four new public
  entry points implement the spec's normative per-channel
  joint-subband copy-and-scale, the destination channel's
  high-end-subband reconstruction step that imports subbands
  `[nSUBS[ch], nSUBS[nSourceCh])` from source channel `nSourceCh =
  JOINX[ch] - 1` and scales each sample by the per-subband
  `JOIN_SCALES[ch][n]` factor:
  - `joint_subband_decode_range_i32(dst_subs, src_subs, scales,
    n_subs_dst, n_subs_src)` /
    `joint_subband_decode_range_f64(...)` — the §C.2.3 inner loop
    over slice-of-slices. For each `n ∈ [n_subs_dst, n_subs_src)`
    the destination subband is overwritten with `scales[n -
    n_subs_dst] * src_subbands[n]`, sample-by-sample. The i32
    variant uses `wrapping_mul` to mirror the spec's C `int`
    overflow semantics.
  - `joint_source_channel(joinx) -> Option<u8>` — resolves the
    one-based `JOINX[ch]` field to the zero-based source-channel
    index per the spec's inline comment ("counts channels as
    1,2,3,4,5, so minus 1"): `0 → None` (joint-subband disabled);
    `joinx > 0 → Some(joinx - 1)`.
  - `joint_subband_required(joinx) -> bool` — the dispatch
    predicate (`joinx > 0`).
  - One new `Error` variant: `Error::JointSubbandShapeMismatch
    { dst_len, src_len }`, fired when any §C.2.3 structural
    invariant is violated (`n_subs_dst > n_subs_src`, dst/src outer
    too short for `n_subs_src`, scales-length disagreement with
    `n_subs_src - n_subs_dst`, or a per-subband sample-length
    mismatch).

  Twenty new in-module unit tests in `src/joint_subband.rs` plus
  three new doc-tests lock the decode behaviour down: one-based →
  zero-based source-channel resolution across `joinx ∈ 1..=u8::MAX`,
  the `joinx == 0` disabled case, copy-and-scale on a two-subband ×
  three-sample fixture, leave-untouched property below
  `n_subs_dst`, empty-range no-op (`n_subs_dst == n_subs_src`),
  zero-scale zeroing, negative-scale sign-inversion, wrapping-
  multiply at `i32::MIN × -1`, write-only-inside-range invariant at
  `(n_dst, n_src) = (2, 4)`, and the error paths
  (`n_subs_dst > n_subs_src`, dst outer too short, src outer too
  short, scales-length disagreement, inner sample-length
  disagreement) for both i32 and f64. A `(n_dst, n_src, n_samples)
  = (2, 5, 8)` end-to-end sweep cross-checks the helper against a
  hand-computed expected.

  Scope: this round lands the per-channel copy + scale only. Wiring
  it into a complete subframe walker (which still needs the AUDIO
  CODING HEADER's per-channel `JOINX[ch]`, `nSUBS[ch]`, and
  `JOIN_SCALES[ch][n]` decoders, plus the §5.4-onwards subframe
  walk and the §C.2.5 QMF-synthesis path that remains gated on the
  §D.8 FIR coefficient tables) is a follow-up.

- Round 214 (2026-06-03) — §C.2.4 Sum/Difference Decoding (ETSI
  TS 102 114 V1.3.1 Annex C §C.2.4, PDF p.184). Six new public entry
  points implement the spec's normative sum/difference matrix decoder
  for the `FRONT_SUM` (`SUMF`) / `SURROUND_SUM` (`SUMS`) /
  `AMODE == 3` joint-channel-coding paths:
  - `sum_difference_decode_i32(left, right)` /
    `sum_difference_decode_f64(left, right)` — single-pair in-place
    decode through the matrix `(L', R') = (L + R, L − R)`. The
    pre-update value of `L` is consumed for both outputs, matching the
    §C.2.4 pseudocode's read-old / write-new ordering. Integer
    arithmetic is wrapping (`i32::wrapping_add` / `wrapping_sub`) to
    mirror the spec's C `int` semantics.
  - `sum_difference_decode_subband_pair_i32(left_subs, right_subs)` /
    `sum_difference_decode_subband_pair_f64(...)` — full §C.2.4 outer
    loop (`for n=0; n<nSUBS; n++`) across slice-of-slices, one inner
    slice per active subband.
  - `front_sum_difference_required(front_sum, amode) -> bool` — the
    dispatch predicate that returns `true` when `SUMF` is set OR
    `amode == 3` (Sum/Difference channel arrangement, per the §C.2.4
    narrative: "This decoding is also required when AMODE = 3.").
  - `surround_sum_difference_required(surround_sum) -> bool` — the
    surround-pair counterpart; reduces to a pass-through of `SUMS`
    because the spec does not name an `AMODE` code that forces the
    surround decode independent of the flag.
  - One new `Error` variant: `Error::SumDiffLengthMismatch
    { left_len, right_len }`, fired when the two slice arguments
    disagree in length (the §C.2.4 pseudocode requires a one-to-one
    sample pairing).

  Twenty-four new in-module unit tests in `src/sum_diff.rs` plus two
  new doc-tests lock the matrix behaviour down: the encoder-decoder
  round-trip property `decode(encode(L, R)) = (2L, 2R)`, the matrix
  self-product `M² = 2 I`, the read-old / write-new ordering check,
  wrapping arithmetic at `i32::MAX`, empty-slice no-ops,
  slice-length-mismatch error reporting (with the offending lengths
  surfaced), subband-pair walks across `nSUBS = 3..=4` with
  `8 * nSSC ∈ {2, 8}`, outer-slice-count mismatch detection,
  per-subband length-mismatch detection (first-mismatch position
  reported), front-pair dispatch-predicate truth tables across the
  full 64-code `AMODE` range (the eight user-defined codes
  `16..=63` do not force the front decode), and the surround-pair
  dispatch-predicate behaviour. The full §C.2.4 sweep at `nSUBS = 4`,
  `8 * nSSC = 8` cross-checks the subband-pair helper against an
  independent hand-computed expected result.

  Scope: this round lands the matrix decode and dispatch predicates
  only. Wiring it into a complete subframe walker (which needs the
  AUDIO CODING HEADER `SUBFS` / `nPCHS` / `nSUBS[ch]` / `JOINX[ch]`
  fields plus the §5.4-onwards subframe / subband / QMF-synthesis
  decode path that remains gated on the §D.8 FIR coefficient tables)
  is a follow-up.

- Round 208 (2026-06-02) — `PreCalCosMod()` 544-entry
  cosine-modulation coefficient array `raCosMod` for the §C.2.5
  32-band synthesis QMF, transliterated verbatim from the spec
  pseudocode transcribed in `docs/audio/dts/dts-core-extracts.md`
  §2.3 (which quotes ETSI TS 102 114 V1.3.1 Annex C §C.2.5, PDF
  p.184). New public surface:
  - `precal_cos_mod() -> [f64; COS_MOD_LEN]` — builder that returns
    the populated 544-entry array. Deterministic: every byte-
    identical run produces the same array (verified by a
    `to_bits()` cross-check between two independent invocations).
  - `COS_MOD_LEN: usize = 544` — total length.
  - `COS_MOD_BLOCK1_START: usize = 0` /
    `COS_MOD_BLOCK2_START: usize = 256` /
    `COS_MOD_BLOCK3_START: usize = 512` /
    `COS_MOD_BLOCK4_START: usize = 528` — start indices of the four
    blocks of the spec's `PreCalCosMod()` pseudocode (`j`-counter
    transitions). Decomposes as 256 + 256 + 16 + 16.
  - The four blocks: Block 1 (256 entries) is
    `cos((2i+1)(2k+1) π/64)` for `k, i ∈ 0..16` (the `(2k+1)`
    half-band cosine-modulation kernel); Block 2 (256 entries) is
    `cos(i(2k+1) π/32)` (the dual second-kind cosine block);
    Block 3 (16 entries) is `+0.25 / (2·cos((2k+1) π/128))` (history
    `SUM`-side scaling); Block 4 (16 entries) is
    `−0.25 / (2·sin((2k+1) π/128))` (history `DIFF`-side scaling).

  Twenty new in-module unit tests in `src/cos_mod.rs` cover: length
  matches 544, block-boundary constants reproduce the four-block
  decomposition, Block 1 anchor `ra[0] = cos(π/64)`, Block 2 anchor
  `ra[256] = 1.0`, Block 3 anchor
  `ra[512] = 0.25 / (2·cos(π/128))`, Block 4 anchor
  `ra[528] = −0.25 / (2·sin(π/128))`, exhaustive 256-entry walks of
  Block 1 + Block 2 against the closed-form, exhaustive 16-entry
  walks of Block 3 + Block 4, Block 3 strict positivity, Block 4
  strict negativity, Block 3 monotone-increasing in k, Block 4
  monotone-decreasing magnitude in k, Block 2 row-zero column
  always 1 (since `cos(0) = 1`), Block 1's last row-zero entry
  matches `cos(31 π / 64)`, every entry finite (no NaN / ±∞),
  Block 1 + Block 2 entries bounded by `[-1, +1]`, the
  packing-density round-trip (16 rows × 16 cols = 256 per Block 1),
  and bit-identical determinism across two independent
  `precal_cos_mod()` calls.

  Scope: this round lands the cosine-modulation matrix builder
  only. The §C.2.5 `QMFInterpolation` synthesis loop (which
  consumes `raCosMod` plus the §D.8 512-tap `raCoeffLossy` /
  `raCoeffLossLess` FIR coefficient tables selected by the frame
  `FILTS` flag) is a follow-up — the §D.8 tables are referenced in
  the staged ETSI PDF (p.238) but not yet transcribed under
  `docs/audio/dts/`. The full filterbank reconstruction is blocked
  on that docs-staging pass, filed as round-208 docs gap #9 in
  `README.md`.

- Round 202 (2026-06-01) — `SFREQ` / `AMODE` / `PCMR` value-table
  resolvers (ETSI TS 102 114 V1.3.1 §5.3.1 Tables 5-5 / 5-4 / 5-17,
  PDF pp.18-23). Closes the three sample-rate / channel-count /
  source-PCM-resolution `Option`-resolver gaps that have been
  documented in README "Docs gaps" since round 1 (#1, #3, #5):
  - `SampleFrequency` enum + `DtsFrameHeader::sample_frequency()` /
    `DtsFrameHeader::sample_rate_hz()` resolve `SFREQ` to one of
    nine fixed source-sampling-frequency values (8/16/32/11.025/
    22.05/44.1/12/24/48 kHz) or `Invalid` for the seven reserved
    codes (`0b0000`, `0b0100`, `0b0101`, `0b1001`, `0b1010`,
    `0b1110`, `0b1111`), per Table 5-5.
  - `AmodeArrangement` enum + `DtsFrameHeader::amode_arrangement()` /
    `DtsFrameHeader::channel_count()` resolve `AMODE` to the
    sixteen standard arrangements at codes `0..=15`
    (`Mono` / `DualMono` / `Stereo` / `SumDifference` / `LtRt` /
    `ClR` / `LrS` / `ClRS` / `LrSlSr` / `ClRSlSr` / `ClCrLRSlSr` /
    `ClRLrRrOv` / `CfCrLfRfLrRr` / `ClCCrLRSlSr` /
    `ClCrLRSl1Sl2Sr1Sr2` / `ClCCrLRSlSSr`) with the CHS column
    surfaced via `AmodeArrangement::channel_count()`, plus
    `UserDefined(u8)` for codes `16..=63` (Table 5-4's user-defined
    band).
  - `SourcePcmResolution` enum + `DtsFrameHeader::source_pcm_resolution()`
    / `DtsFrameHeader::source_pcm_bits_per_sample()` resolve `PCMR`
    to one of six valid `(bits, es)` pairs (16/16/20/20/24/24 bits
    with the auxiliary DTS-ES flag) at codes `{0,1,2,3,5,6}` or
    `Invalid` for the two reserved codes `{4, 7}` per Table 5-17.
  Seven new lib-level tests in `src/header.rs` lock the
  table-row-by-table-row mapping down: exhaustive 16-code SFREQ
  walk, 64-code AMODE walk (sixteen standard + 48 user-defined),
  8-code PCMR walk, plus a Table 5-4 CHS-column reproduction
  test and a `ffmpeg_fixture_resolves_to_48k_stereo_16bit` end-to-
  end check that exercises the new resolvers against the same
  synthetic header geometry as the bundled black-box fixture. The
  black-box integration tests in `tests/black_box_ffmpeg.rs` now
  assert `sample_rate_hz() == Some(48_000)`, `channel_count() == Some(2)`,
  and `source_pcm_bits_per_sample() == Some(16)` across the three
  documented sync encodings (raw-BE, 14-bit-BE, 14-bit-LE) of the
  same ffmpeg-encoded 48 kHz / stereo / 768 kb/s frame. The
  `DIALNORM`-code-to-dB mapping (Table 5-20) remains a docs-
  completeness follow-up — the table's row order in the staged
  PDF straddles the `VERNUM == 6` and `VERNUM == 7` sign-convention
  columns and needs a tighter transcription before
  `dialog_normalization_db()` can resolve.

- Round 195 (2026-05-31) — §5.4.1 ABITS / SCALES (a.k.a. ALLOC /
  SCFAC) Primary Audio Coding Side Information bit-stream decoders,
  with the Annex D §D.5.6 / §D.5.3 / §D.5.4 small-Huffman codebooks
  and §D.1.1 / §D.1.2 RMS square-root tables they dispatch through.
  All tables transcribed verbatim from the locally staged ETSI
  TS 102 114 V1.3.1 PDF (Annex D pp.191-205 + Table 5-28
  pp.28-30 + Tables 5-22..5-27 pp.26-27). New public surface:
  - `AbitsCodebook` enum + `AbitsCodebook::from_bhuff(u8)` —
    resolves the 3-bit `BHUFF[ch]` field to one of the seven
    documented variants per Table 5-25 (`A12`, `B12`, `C12`, `D12`,
    `E12`, `Linear4Bit`, `Linear5Bit`). `BHUFF == 7` is rejected as
    the new `Error::InvalidSideInfo { field: "BHUFF" }`.
  - `ScalesCodebook` enum + `ScalesCodebook::from_shuff(u8)` —
    resolves `SHUFF[ch]` per Table 5-24 (`Sa129..Se129`,
    `Linear6Bit`, `Linear7Bit`). `SHUFF == 7` rejected as
    `Error::InvalidSideInfo { field: "SHUFF" }`. Two predicate
    accessors mirror the §5.4.1 dispatch:
    `is_huffman_encoded()` distinguishes difference-coded vs
    absolute-coded paths, `uses_7bit_rms_table()` distinguishes the
    §D.1.2 vs §D.1.1 square-root lookup.
  - `decode_abits_at(bytes, bit_offset, codebook) -> (u8, usize)`
    — extracts one ABITS field from a byte slice at an arbitrary
    bit offset, returning the decoded index plus
    `bits_consumed` so the caller can chain calls through a
    §5.4.1 inner loop.
  - `decode_scales_at(bytes, bit_offset, codebook, n_scale_sum) ->
    (u32, i32, usize)` — extracts one SCALES field, returning the
    table-looked-up scale-factor value, the updated `n_scale_sum`
    accumulator (so the difference-encoded path's running sum
    chains across calls), and `bits_consumed`.
  - `RMS_6BIT: [u32; 64]` — §D.1.1 6-bit RMS square-root
    quantisation levels (index 63 is the spec-reserved "invalid"
    slot).
  - `RMS_7BIT: [u32; 128]` — §D.1.2 7-bit RMS levels (indexes
    125..=127 are spec-reserved).
  - `Error::InvalidSideInfo { field, value }` — reserved BHUFF /
    SHUFF / SCALES values (selector 7, or a SCALES accumulator
    walking into a spec-invalid table slot). Mapped to
    `oxideav_core::Error::InvalidData` through the registry's
    `From<DtsError>` impl.
  - `Error::HuffmanDecodeFailed { table }` — bit stream did not
    match any entry in the named Annex D codebook within the
    maximum documented code length (defensive bound; the Annex D
    codebooks are all complete prefix codes by Kraft's inequality,
    so this fires only on EOF or stream-format corruption).

  Nineteen new in-module unit tests in `src/side_info.rs` cover:
  BHUFF / SHUFF reserved-value rejection, all-7-codes exhaustive
  dispatch (BHUFF + SHUFF), 7-bit-RMS-table predicate, all 60
  ABITS Huffman symbols round-trip across A12/B12/C12/D12/E12,
  linear-4-bit and linear-5-bit raw-field decode, EOF surface,
  Kraft-equality completeness check across every codebook
  (A12..E12 + A5/B5/C5 + A7/B7), RMS table lengths + anchor-value
  cross-check against the staged PDF, linear-6-bit and
  linear-7-bit absolute lookups, SA129 difference accumulation
  across a (+1, +1, -1) sequence, SD129 7-level table with ±3
  range, negative-accumulator rejection, and reserved-index
  rejection in both 6-bit and 7-bit RMS tables. Three new
  integration tests in `tests/side_info_decode.rs` exercise the
  public `decode_abits_at` / `decode_scales_at` surface
  end-to-end: a 5-subband ABITS block walked through BHUFF=A12 (24
  total bits, [1, 5, 12, 1, 8]), a 5-subband SCALES block walked
  through SHUFF=SA129 with a hand-built difference sequence
  (+2, +1, 0, -1, -2) starting from `n_scale_sum=10` and
  cross-checked against the §D.1.1 lookups, and a linear-7-bit
  block that demonstrates the absolute-overwrite contract
  (`n_scale_sum` is overwritten by each call, not accumulated).

  Scope: this round only lands the **single-field** decode
  primitives plus their backing tables. Wiring them into a
  complete subframe walker (which also requires the AUDIO CODING
  HEADER §5.3.x fields SUBFS, PCHS, SUBS, VQSUB, JOINX,
  BHUFF/THUFF/SHUFF, plus the side-info loop over
  `nPCHS × nSUBS[ch]`) is a follow-up. The 129-entry full
  SA129..SE129 mappings (referenced by Table 5-24 but not
  transcribed under that name in the staged Annex D revision)
  remain a docs-completeness follow-up; round 195 routes
  SHUFF=0..4 through the small-Huffman §D.5.3 / §D.5.4 codebooks
  the staged PDF does enumerate, treating their symbols as
  scale-factor index differences per the §5.4.1 pseudocode.


## [0.0.1](https://github.com/OxideAV/oxideav-dts/releases/tag/v0.0.1) - 2026-05-30

### Other

- round 192: iter_frames_14bit — 14-bit container-stream frame walker
- round 189: frame_size_container_bytes() — 14-bit advance per ETSI §5.3.1 / §6.1.3.1
- RATE → targeted bit-rate via ETSI §5.3.1 Table 5-7
- round 179: iter_syncs lazy iterator + SyncWordEncoding/SyncMatch accessors
- round 165: find_next_sync first-byte gate (252/256 short-circuit)
- round 159: iter_frames_resync error-tolerant frame walker
- round 151: find_all_syncs bulk-scan helper + raw-LE iter_frames coverage
- round 148: encode_frame_header_14bit_{be,le} — all 4 sync encodings round-trip
- round 145: raw-LE encoder + bidirectional 14<->16-bit container pack/unpack
- round 141: encode_frame_header_be — parse↔encode round-trip on header window
- surface header->SUBFRAMES boundary as bit/byte length
- round 6: multi-frame iterator + resync helper
- round 5: post-CRC 16-bit trailing window (multirate / version / copy / PCMR / sum / dialnorm)
- round 4: oxideav-core Decoder integration + ci-standalone job
- round 3: trailing-13-bit fields + optional header CRC
- round 2: 14-bit sync unpacking + parse_frame_header_14bit
- round 1: frame-header parser per ETSI TS 102 114 §5.3
- orphan rebuild: clean-room scaffold post 2026-05-18 audit

### Added

- Round 192 (2026-05-30) — 14-bit container-byte frame iterator
  (`iter_frames_14bit`). Closes the empirical half of round-6 docs
  gap #7 by wiring the round-189
  `DtsFrameHeader::frame_size_container_bytes` accessor into a
  multi-frame walker that operates directly on 14-bit-packed
  container bytes (no caller-side unpack step required). New public
  surface:
  - `iter_frames_14bit(bytes) -> FrameIterator14<'_>` — convenience
    constructor mirroring `iter_frames`.
  - `FrameIterator14<'a>` — `Iterator<Item = Result<FrameView14<'a>>>`.
    Each step calls `find_next_sync`, accepts only 14-bit syncs
    (raw 16-bit syncs at the cursor yield `Error::UnsupportedRaw16Bit`
    and terminate — the symmetric counterpart to the round-6
    `Error::UnsupportedFourteenBit` behaviour on `iter_frames`), calls
    `parse_frame_header_14bit` at the matched offset to surface the
    header, and advances the cursor by
    `header.frame_size_container_bytes(encoding)` container bytes.
    Truncated tails surface `Error::UnexpectedEof` at the boundary
    (mirroring `iter_frames`'s contract).
  - `FrameView14<'a>` — per-step container-domain view. Fields
    differ in semantics from `FrameView` to avoid an overloaded
    `len`: here `len` is the container-byte advance (= the
    round-189 formula result) and `data` is the container-byte
    window of the frame. The unpacked-domain logical byte count is
    still available as `header.frame_size_bytes`.
  - `Error::UnsupportedRaw16Bit` — new variant, symmetric
    counterpart to `Error::UnsupportedFourteenBit`. Surfaced by
    `iter_frames_14bit` when a raw 16-bit sync is encountered at
    the cursor. Mapped to `oxideav_core::Error::Unsupported`
    through the registry's `From<DtsError>` impl.

  Ten new unit tests cover: single-frame BE / LE walks; back-to-back
  BE frames with cursor + length cross-check; leading garbage before
  the first sync; raw-16-bit sync rejection; empty buffer; no-sync
  buffer; truncated tail reporting `UnexpectedEof`; `view.data`
  round-trips through `parse_frame_header_14bit`; `cursor()`
  advances by exactly `frame_size_container_bytes` per step. Two
  new integration tests in `tests/multi_frame_iter.rs` repackage
  the bundled ffmpeg 5-frame fixture (5 × 1024 raw-BE bytes) into
  14-bit-packed BE and LE streams (5 × 1172 container bytes each)
  and verify the iterator walks all five frames with the expected
  header fields and container-byte length.

- Round 189 (2026-05-30) — 14-bit container-byte frame-advance
  accessor derived from
  `docs/audio/dts/dts-core-extracts.md` §3.3 (ETSI TS 102 114 V1.3.1
  §5.3.1 `FSIZE+1` definition combined with the §6.1.3.1 / §6.3.x
  28-bit-word-boundary invariant). New public surface:
  - `DtsFrameHeader::frame_size_container_bytes(SyncWordEncoding) -> u32`
    — returns the container-byte distance from this frame's
    syncword to the next frame's syncword for each of the four wire
    encodings. For `RawBigEndian` / `RawLittleEndian` the answer is
    just `frame_size_bytes` (FSIZE+1 already counts on-wire
    container bytes of the 16-bit-per-word stream). For
    `FourteenBitBigEndian` / `FourteenBitLittleEndian` the answer is
    `2 * ceil(frame_size_bytes * 8 / 14)` container bytes (one
    16-bit container word carries 14 logical bits per §3.2 /
    §6.1.3.1; the partial final word is padded out to the next
    two-container-word boundary per the ETSI alignment invariant).

  Seven new unit tests lock the formula down:
  raw-equals-`frame_size_bytes` for both raw encodings;
  1024-logical → 1172-container; minimum 95 → 110 / maximum
  16384 → 18726 container-byte advance; strict-greater-than-raw
  + closed-form `16/14` scaling upper bound on a spread of frame
  sizes; BE/LE equivalence on both pairs (the 14-bit-LE byte count
  matches 14-bit-BE because LE is the pairwise byte-swap of BE per
  the wiki); the 14-bit advance is always even (the §3.3 / §6.1.3.1
  28-bit-word-boundary invariant forces the per-frame step to land
  on a two-container-word boundary); and a closed-form cross-check
  `2 * ceil(frame_size_bytes * 8 / 14)`. This closes the
  analytical half of round-6 docs gap #7; the empirical half (wiring
  the advance + a streaming per-frame 14↔16-bit unpacker into
  `iter_frames`) is now a focused follow-up rather than blocked.

- Round 185 (2026-05-29) — transmission bit-rate resolution from
  ETSI TS 102 114 V1.3.1 §5.3.1 Table 5-7 (transcribed in
  `docs/audio/dts/dts-core-extracts.md` §1). New public surface:
  - `TargetedBitRate` — `Fixed(u32)` (one of the 25 documented
    targeted rates, in bits per second), `Open` (`RATE == 0b11101`),
    or `Invalid` (any reserved code). `#[non_exhaustive]`.
  - `DtsFrameHeader::targeted_bit_rate() -> TargetedBitRate` —
    resolves the 5-bit `RATE` index per Table 5-7.

  Behavioural change (not a signature change): `DtsFrameHeader::bit_rate_bps()`
  now returns `Some(bps)` for the 25 fixed codes (e.g. code `0b01111`
  → `Some(768_000)`) instead of the round-1 placeholder `None`; it
  still returns `None` for the open and invalid codes (use
  `targeted_bit_rate()` to distinguish). The `dynamic_range` (`DYNF`,
  Table 5-8) and `time_stamp` (`TIMEF`, Table 5-9) field docs now
  cite the same clause. One new exhaustive unit test walks all 32
  `RATE` codes; the ffmpeg black-box tests now assert `768_000` bps
  across the raw-BE / 14-bit-BE / 14-bit-LE encodings,
  cross-validated against ffprobe's external read of the same frame.
  The SFREQ (sample-rate) and AMODE (channel) value tables remain a
  docs gap, so `sample_rate_hz()` / `channel_count()` still return
  `None`.

- Round 179 (2026-05-29) — lazy `iter_syncs` / `SyncIterator`
  streaming counterpart to `find_all_syncs`, plus a small accessor
  surface on `SyncWordEncoding` and `SyncMatch` derived from the
  wiki sync-sequence table (`docs/audio/dts/wiki/DTS.wiki`). New
  public surface:
  - `iter_syncs(bytes) -> SyncIterator<'_>` and `SyncIterator` —
    `Iterator<Item = SyncMatch>` over every sync sequence in a
    byte buffer. Same matching rules, walk order, and `O(n)` cost
    as `find_all_syncs`, but without the upfront `Vec<SyncMatch>`
    allocation. Useful for `take(N)`, `filter`, and `map`-style
    consumption (e.g. streaming sniffers that act on each match
    as it appears rather than after the full scan completes).
  - `SyncWordEncoding::sync_byte_length() -> usize` — wiki-table-
    derived byte length of the on-wire sync sequence (4 for the
    two raw encodings, 6 for the two 14-bit-packed encodings).
  - `SyncWordEncoding::is_raw_16bit()` / `is_14bit_packed()` —
    mutually-exclusive predicates that partition the enum into
    the raw vs container forms documented in the wiki.
  - `SyncMatch::sync_byte_length() -> usize` and
    `sync_byte_range() -> Range<usize>` — delegate to the
    encoding's wiki-derived length so the common "advance past the
    matched sync" / "highlight the matched bytes" patterns read as
    `cursor = m.offset + m.sync_byte_length()` /
    `&bytes[m.sync_byte_range()]`.

  Eleven new tests (plus one new doc-test) lock down: the four
  wiki-table sync byte counts (4 / 4 / 6 / 6), the raw-vs-packed
  predicate partition, `sync_byte_range` slicing back the original
  sync bytes for raw-BE and 14-bit-BE inputs, streaming vs bulk
  equivalence (`iter_syncs(...).collect() == find_all_syncs(...)`)
  on a mixed-encoding buffer, an empty-result buffer, `take(N)`
  window correctness, the `is_raw_16bit` filter combinator, the
  `SyncIterator::cursor()` progression contract, and a 4 KB
  pseudo-random buffer cross-check against the existing
  `reference_find_all_syncs`. No new docs gap is introduced; the
  byte-length values are read verbatim from the wiki snapshot.

- Round 165 (2026-05-27) — `find_next_sync` first-byte gate
  (constant-factor speedup, no API change). The inner loop of
  `find_next_sync(bytes, start)` now skips positions whose first
  byte cannot start any of the four documented sync sequences
  (`0x7F` raw-BE / `0xFE` raw-LE / `0x1F` 14-bit-BE / `0xFF`
  14-bit-LE) with a single compare-and-branch, rather than calling
  the multi-byte `detect_sync` helper on every position. On
  uniform-random payload 252 of 256 first bytes short-circuit
  through the cheap path (4-byte raw-sync equality check + two
  6-byte 14-bit container unpacks elided). The walk order,
  returned `SyncMatch { offset, encoding }`, and end-of-buffer
  bookkeeping are identical to round 6 — downstream walkers
  (`iter_frames`, `iter_frames_resync`, `find_all_syncs`) inherit
  the speedup transparently because they all dispatch through
  `find_next_sync`.
- Round 165 (2026-05-27) — eight new tests covering the optimised
  scanner:
  - `first_byte_candidate_accepts_exactly_four_bytes` —
    exhaustive 256-input check that the filter accepts exactly
    `{0x1F, 0x7F, 0xFE, 0xFF}` and rejects the other 252.
  - `first_byte_candidate_accepts_documented_sync_prefixes` —
    spot-checks each documented sync's first byte individually
    plus four adjacent non-sync bytes.
  - `find_next_sync_matches_pre_optimization_reference_on_candidate_dense_payload`
    — packs every fourth byte with a first-byte sync candidate
    but a non-sync continuation; the optimised scanner must
    return the same `None` (then the same embedded sync at offset
    100) as the pre-round-165 brute-force reference.
  - `find_next_sync_matches_reference_on_pseudo_random_buffer` —
    4 KB LCG-seeded random buffer; sweeps every possible `start`
    offset and asserts per-call agreement with the reference.
  - `find_all_syncs_matches_reference_on_random_buffer_with_embedded_syncs`
    — bulk-scan parity test embedding one sync of each of the
    four encodings at known positions; verifies the optimised
    `find_all_syncs` recovers every `(offset, encoding)` pair the
    reference recovers.
  - `find_next_sync_handles_all_ones_payload_with_one_embedded_sync`
    — all-`0xFF` payload (every position is a first-byte
    candidate, the negative filter's degenerate case) with one
    real raw-LE sync embedded at offset 50.
  - `find_next_sync_handles_all_zero_payload` — all-zero buffer
    (no first-byte candidates anywhere) returns `None`; confirms
    the early-exit path doesn't infinite-loop or skip
    end-of-buffer bookkeeping.
  - `find_next_sync_start_sweep_matches_reference_with_two_real_syncs`
    — sweeps `start` across every offset of a 200 B buffer
    holding two real syncs; per-call agreement with the
    reference.
- Round 159 (2026-05-27) — `iter_frames_resync` error-tolerant
  multi-frame walker. The fail-fast `iter_frames` from round 6
  terminates at the first parse failure; the new
  `FrameIteratorResync` / `iter_frames_resync(bytes)` instead treat
  parse failures at candidate sync positions as false-positive
  syncs and continue scanning from `offset + 1`, recovering real
  frames that follow corrupted-header patches in the middle of a
  `.dts` byte stream.
  - New public types `FrameIteratorResync<'a>` (the iterator),
    `ResyncEvent { offset, encoding, cause }` (Err item), and
    `ResyncCause` (the 4-variant cause enum:
    `StructuralBoundFailed(Error)`, `HeaderEof`,
    `FrameLengthOverrunsBuffer { declared_len }`,
    `FourteenBitSyncSkipped`). All re-exported at the crate root.
  - `iter_frames_resync(bytes)` convenience constructor, mirroring
    the existing `iter_frames(bytes)`.
  - The new iterator does NOT depend on the `oxideav-core`
    integration — both `iter_frames` and `iter_frames_resync` are
    available in the `--no-default-features` build.
  - The well-formed-input contract: on a clean stream the resync
    iterator's yields are byte-for-byte identical to the fail-fast
    iterator's (every step is `Ok`; frame views match). Round 159
    asserts this against the bundled ffmpeg 5-frame fixture.
  - Eleven new unit tests in `src/iter.rs` cover: clean-stream
    parity with `iter_frames`, false-positive sync skipping with
    real-frame recovery, frame-length overrun event surfacing,
    14-bit sync skipping (so a raw stream with stray 14-bit-shaped
    payload patches still walks), cursor advance (one byte on
    event, frame_size_bytes on Ok), empty buffer, no-sync buffer,
    multiple consecutive false positives all reported, truncated
    tail surfaces overrun event then ends, truncated header
    surfaces `HeaderEof`, and `iter_frames_resync` ≡
    `FrameIteratorResync::new`.
  - Two new integration tests in `tests/multi_frame_iter.rs` cover
    the resync iterator against the bundled ffmpeg fixture: a
    clean-fixture equivalence check (resync walks identically to
    fail-fast), and a corrupted-fixture recovery check (frame-2
    header byte flip → resync surfaces one `StructuralBoundFailed`
    event and recovers frames 3, 4, and 5).
- Round 151 (2026-05-26) — `find_all_syncs` bulk-scan helper plus
  raw-LE `iter_frames` test coverage.
  - `find_all_syncs(bytes: &[u8]) -> Vec<SyncMatch>` is the bulk
    counterpart to the round-6 `find_next_sync`: it scans the entire
    input buffer and returns every documented sync occurrence (all
    four encodings) as a vector. Same `O(n)` cost as a
    `find_next_sync` loop from `offset + 1`; the bulk helper just
    materialises the result for stream-integrity tooling that needs
    every resync point up front. The four documented sync prefixes
    start with mutually-distinct first bytes (`7F` / `FE` / `1F` /
    `FF`), so adjacent (non-overlapping) sync occurrences are both
    reported. Includes a doctest plus seven unit tests covering:
    empty buffer, no-sync buffer, single sync, mixed raw-BE / raw-LE,
    all four encodings, consecutive back-to-back syncs, garbage-
    interspersed positions, and parity with the explicit
    `find_next_sync` loop reference.
  - Three new unit tests for `iter_frames` against a hand-built
    multi-frame raw-LE byte stream (constructed by pairwise
    word-swap of a two-frame raw-BE buffer to match the wiki's
    raw-LE-is-word-swapped-raw-BE definition): the walker
    correctly identifies both frames as
    `SyncWordEncoding::RawLittleEndian`, advances by
    `frame_size_bytes` (which the wiki defines as byte length of the
    unpacked raw-16-bit stream — byte-equivalent across both raw
    encodings), and remains robust to leading garbage / resync.
    Closes a coverage hole because the previous test grid only
    exercised the raw-BE path via the bundled ffmpeg fixture.
- Round 148 (2026-05-26) — 14-bit-packed encoder variants that close
  the parse↔encode round-trip across all four documented sync
  encodings. Two new primitives:
  - `encode_frame_header_14bit_be(&DtsFrameHeader) -> Result<Vec<u8>>`
    composes `encode_frame_header_be` with the round-145
    `pack_16bit_to_14bit` primitive: the raw-BE header bytes are
    padded to 15 bytes (= 120 bits = the worst-case `crc_present == 1`
    header window) and re-packed into nine 14-bit-BE containers. The
    output is always exactly 18 bytes (regardless of `crc_present`)
    and always begins with the wiki-documented 14-bit-BE sync prefix
    `1F FF E8 00 …`.
  - `encode_frame_header_14bit_le(&DtsFrameHeader)` is the same
    composition with `FourteenBitByteOrder::LittleEndian`; the output
    is the pairwise byte-swap of the 14-bit-BE output (each container
    swapped independently) and begins with `FF 1F 00 E8 …`.
  - Both encoders inherit the bit-width and structural-bound checks
    from `encode_frame_header_be`
    (`BlockCountOutOfRange` / `FrameSizeOutOfRange` /
    `FieldOutOfRange{header_crc}`).
  - Fourteen new unit tests covering: fixed 18-byte output for both
    `crc_present` states (BE + LE), wiki sync-prefix reproduction
    (BE + LE), pairwise-byte-swap equivalence between the BE and LE
    outputs, parse↔encode round-trip through `parse_frame_header_14bit`
    with and without CRC (BE + LE), NBLKS / FSIZE / CRC-payload bound
    rejection inheritance, an exhaustive 24-case grid
    ({LFE × CRC × {NBLKS, FSIZE}}) covering both variants and
    confirming cross-equivalence on every case, and a cross-check
    that unpacking the 14-bit-BE encoder output through
    `unpack_14bit_to_16bit` recovers the raw-BE header prefix
    byte-for-byte.
- Round 145 (2026-05-26) — raw-LE encoder + bidirectional 14↔16-bit
  container pack/unpack. Two new primitives:
  - `encode_frame_header_le(&DtsFrameHeader) -> Result<Vec<u8>>`
    serialises a parsed header into the raw-LE on-wire byte form
    (canonical sync `FE 7F 01 80`); the output is exactly 16 bytes
    long regardless of `crc_present` (the parser's raw-LE branch
    requires a 16-byte word-swap window). Implemented as
    `encode_frame_header_be` + zero-pad to 16 + 16-bit-word-swap.
    The `parse_frame_header(encode_frame_header_le(hdr))` round-trip
    recovers `hdr` on every field; the parser reports
    `SyncWordEncoding::RawLittleEndian` because that's the sync it
    detected at the input.
  - `pack_16bit_to_14bit(input, order) -> (Vec<u8>, usize)` is the
    inverse of `unpack_14bit_to_16bit`. The input is read as an
    MSB-first bit stream; successive 14-bit chunks are written into
    the lower 14 bits of 16-bit containers, with the upper 2 bits
    filled by a sign-extension of payload bit 13 (per the wiki's
    "sign bit extension" rule). The returned `payload_bit_count`
    lets callers recover the exact pre-pack bit length when the
    input does not divide evenly into 14-bit chunks. Feeding the
    32-bit raw-BE syncword `7F FE 80 01` reproduces the wiki's first
    two 14-bit sync containers byte-for-byte (`1F FF E8 00` BE and
    `FF 1F 00 E8` LE) and the third container's top 12 bits
    (`0x07F`); the lower 4 bits of the third container hold 4 bits
    of the following field rather than the syncword, matching the
    wiki's `07 Fx` notation.
  - Seventeen new unit tests covering: `encode_frame_header_le`
    canonical sync emission, fixed-16-byte output length for both
    `crc_present` states, equivalence with manual
    `swap16(BE.padded_to_16())`, round-trip through the parser with
    and without CRC, NBLKS / CRC-payload bound rejection inheritance,
    an exhaustive {LFE × CRC × {NBLKS, FSIZE}} grid (24 cases), and
    byte-swap reproduction of the real ffmpeg fixture; plus
    `pack_16bit_to_14bit` wiki-sync-prefix reproduction (BE + LE),
    sync-pattern-with-following-bits reproduction of the wiki's
    `0x07 F<x>` third container, round-trip across multiple input
    lengths (BE + LE), the byte-swap equivalence of BE vs LE pack
    output, empty-input contract, and the sign-extension contract
    for positive and negative payloads.
- Round 141 (2026-05-26) — `encode_frame_header_be(&DtsFrameHeader)
  -> Result<Vec<u8>>` serialises a parsed [`DtsFrameHeader`] back
  into the raw-BE on-wire bytes of the frame-sync header window
  (104 or 120 bits, i.e. 13 or 15 bytes depending on
  `crc_present`). The encoder is the inverse of
  `parse_frame_header` against the wiki bit-table — every field
  round-trips bit-exact, and the canonical raw-BE sync
  `7F FE 80 01` is always emitted even if the source header was
  parsed from the raw-LE / 14-bit-BE / 14-bit-LE encoding (the
  caller is expected to repack post-process if a non-raw-BE
  on-wire form is needed). The encoder validates the same
  structural bounds as the parser (`BlockCountOutOfRange`,
  `FrameSizeOutOfRange`) plus per-field bit-width bounds via a
  new `Error::FieldOutOfRange { field, value, max }` variant
  covering AMODE > 63, SFREQ > 15, RATE > 31, EXT_DESCR > 7,
  VERSION > 15, COPY_HISTORY > 3, PCMR > 7, DIALNORM > 15,
  `sample_count_per_block` > 32, and a `header_crc.is_some()`
  vs `crc_present` mismatch (rejected so a silent drop or
  garbage-emit bug cannot defeat the round-trip property).
- Twelve new unit tests covering: non-trivial round-trip with CRC,
  minimal 13-byte termination-frame round-trip without CRC, every
  field-bounds rejection variant, raw-LE input normalised to
  raw-BE output (every field preserved except
  `sync_word_encoding`), an exhaustive grid over the four LFE
  codes × two CRC states × three {NBLKS, FSIZE} pairs (24 cases),
  and a byte-for-byte equality check against the real ffmpeg
  fixture's 13-byte header window
  (`encode_frame_header_be(parse(b))[..] == b[..13]`).
- Round 138 (2026-05-26) — header → SUBFRAMES boundary accessors.
  `DtsFrameHeader::header_bit_length()` returns the total bit-count
  the frame-sync header window occupies (sync + base + trailing +
  optional HEADER_CRC + post-CRC). The value is fully derived from
  the wiki bit-table in `docs/audio/dts/wiki/DTS.wiki`: 32 + 43 + 13
  + 16 + (16 iff `crc_present`) = 104 bits when CRC is absent, 120
  bits when CRC is present. Both totals are exact multiples of 8 by
  construction, so the corresponding
  `DtsFrameHeader::header_byte_length()` is always 13 or 15 and the
  SUBFRAMES region (the wiki's `'''TODO'''` cell) starts on a byte
  boundary. `FrameView::payload()` returns
  `&data[header.header_byte_length()..]` so downstream re-muxers,
  payload-CRC validators, and the future subframe decoder can carve
  out the SUBFRAMES region directly without recomputing the header
  boundary.
- Eight new tests covering the boundary accessor: 104-bit return
  when `crc_present == 0`, 120-bit return when `crc_present == 1`,
  manual wiki-table sum equivalence, exhaustive byte-alignment over
  a grid of structural-field combinations, the 14-bit-packed entry
  point agrees with the raw-BE entry point on the bit-length value,
  and three `FrameView::payload()` integration cases (two synthetic
  95-byte termination frames with crc absent / crc present, and the
  real ffmpeg-generated 5-frame fixture).
- Round 6 (2026-05-25) — multi-frame iterator + resync helper.
  New `iter` module exposes `find_next_sync(bytes, start) -> Option<SyncMatch>`
  and `iter_frames(bytes) -> FrameIterator<'_>` (plus the supporting
  `FrameView<'_>` / `SyncMatch` types) on top of the existing
  single-frame parsers. `find_next_sync` scans for any of the four
  documented DTS sync sequences (raw 16-bit BE / LE, 14-bit packed
  BE / LE) at or after an arbitrary offset, returning the offset
  and matched `SyncWordEncoding`. `iter_frames` walks a raw-16-bit
  DTS Core byte stream frame by frame, using each frame's
  `DtsFrameHeader::frame_size_bytes` to advance to the next sync;
  it tolerates leading garbage by resyncing through
  `find_next_sync`, surfaces parse failures as the next item's
  `Err`, and terminates cleanly after the last frame. The
  iterator refuses 14-bit container streams (yields
  `Error::UnsupportedFourteenBit` and terminates) because the
  container-byte advance rule for 14-bit-packed frames is not
  enumerated in the wiki snapshot (filed as round-6 docs gap #7
  in `README.md`).
- New bundled fixture `tests/fixtures/dts_5_frames.bin` (5 120
  bytes, 5 back-to-back DTS frames at 1 024 B each) generated as
  `ffmpeg -f lavfi -i "sine=frequency=440:duration=0.05" -ac 2
  -ar 48000 -c:a dca -strict experimental -b:a 768k -f dts ...`.
  Used by the new `tests/multi_frame_iter.rs` integration test
  (`include_bytes!` from the fixture path) to exercise the
  iterator end-to-end.
- Seven new tests in `tests/multi_frame_iter.rs` covering:
  iteration over all five fixture frames with per-frame field
  assertions, fixture-size sanity check, `find_next_sync`
  enumeration of every sync offset in the fixture, iteration
  through a stream with 13 bytes of leading garbage,
  iterator-vs-direct `parse_frame_header` equivalence at each
  offset, clean termination after the last frame, and a
  truncated-tail variant that surfaces `Error::UnexpectedEof` at
  the boundary.
- Nine new unit tests in `src/iter.rs` covering `find_next_sync`:
  sync at offset zero, sync after leading garbage, every documented
  sync encoding (raw BE / raw LE / 14-bit BE / 14-bit LE),
  `start` honoured past a prior sync, `None` when no sync exists,
  `None` when `start >= bytes.len()`, and `None` when only a
  partial sync sits at the buffer tail.
- Round 5 (2026-05-25) — 16-bit post-CRC trailing window surfaced
  through `DtsFrameHeader`. After the optional 16-bit `HEADER_CRC`
  slot (or after the predictor-history bit when `crc_present == 0`),
  the parser now consumes seven additional fields the wiki
  snapshot enumerates: `multirate_inter` (1 bit), `version` (4
  bits, raw 0..=15), `copy_history` (2 bits, raw 0..=3),
  `source_pcm_resolution_index` (3 bits, raw 0..=7), `front_sum`
  (1 bit), `surround_sum` (1 bit), and `dialog_normalization` (4
  bits, raw 0..=15). The window is consumed unconditionally
  regardless of `crc_present` because the wiki lists it after
  the HEADER_CRC slot in both code paths. Two new resolver
  stubs (`DtsFrameHeader::source_pcm_bits_per_sample` and
  `DtsFrameHeader::dialog_normalization_db`) return `None`
  pending the index → value tables landing in `docs/` (filed as
  round-5 docs gaps #5 and #6 in `README.md`).
- Twelve new unit tests covering: full post-CRC window
  decomposition for a non-trivial bit pattern (`0xD2EC`),
  all-zero and all-one post-CRC windows, exhaustive round-trip
  for every PCMR / DIALNORM / VERSION / COPY_HISTORY code, the
  `crc_present == 0` vs `crc_present == 1` equivalence of
  post-CRC sub-fields, and updated assertions on every
  pre-existing parser test (raw-BE, raw-LE, 14-bit-BE,
  14-bit-LE, value-resolver, NBLKS-bounds, FSIZE-bounds,
  short-buffer EOF, trailing-bit edge cases) to also verify the
  new fields.
- Black-box ffmpeg fixture asserts (raw-BE + both 14-bit
  variants + cross-encoding equivalence) extended to verify the
  post-CRC fields recovered from the real `ffmpeg -c:a dca`
  frame: `multirate_inter == false`, `version == 7`,
  `copy_history == 0`, `source_pcm_resolution_index == 0`,
  `front_sum == false`, `surround_sum == false`,
  `dialog_normalization == 0`. The same values must come out
  through all three sync encodings. Registry's
  `send_packet_eagerly_parses_header` test additionally checks
  the cached header carries the post-CRC fields after the
  decoder handle's `send_packet` call.
- Round 4 (2026-05-22) — `oxideav-core` framework integration. A
  new default-on `registry` cargo feature gates the
  `oxideav-core` dep, the `Decoder` trait impl, and the
  `oxideav_core::register!("dts", register)` macro invocation.
  With the feature off, the crate retains the standalone
  `parse_frame_header` / `parse_frame_header_14bit` /
  `unpack_14bit_to_16bit` APIs plus the crate-local `Error` /
  `Result` types and pulls no `oxideav-core` dep.
- `make_decoder(params) -> Box<dyn Decoder>` factory and the
  `DtsDecoderHandle` it returns. `Decoder::send_packet` parses
  the frame header eagerly through `detect_sync` and routes to
  `parse_frame_header` (raw 16-bit syncs) or
  `parse_frame_header_14bit` (14-bit packed syncs), surfacing
  structural failures (`NoSync`, `BlockCountOutOfRange`,
  `FrameSizeOutOfRange`) as `Error::InvalidData` and short
  buffers as `Error::NeedMore`. `Decoder::receive_frame` returns
  `Error::Unsupported` because PCM output is gated on the
  SFREQ/RATE/AMODE value tables landing in `docs/` (see README
  docs gaps #1-#3). `Decoder::reset` clears the cached header.
- `register_codecs(reg)` / `register(ctx)` install a `CodecInfo`
  for `CodecId::new("dts")` carrying the FourCC tags
  `CodecTag::fourcc(b"dts ")` and `CodecTag::fourcc(b"dtsc")` so
  the codec resolver routes both QuickTime sample-entry types to
  the DTS decoder factory.
- `probe_dts(&[u8]) -> Confidence` — standalone confidence helper:
  `1.0` on a valid frame at offset 0, `0.5` on a truncated buffer
  (sync detected but body short), `0.0` on unrelated input. The
  registry's per-codec probe function (`probe_dts_tag`) wraps
  this for the `ProbeContext`-driven path: when the demuxer
  supplies a packet sample it forwards to `probe_dts`; when not,
  it returns `1.0` so the FourCC claim is treated as unambiguous.
- Inline `ci-standalone` job in `.github/workflows/ci.yml` running
  `cargo build --no-default-features --lib` and
  `cargo test --no-default-features --lib` on every push, beside
  the existing `OxideAV/.github` reusable-workflow `ci` job that
  exercises the default-feature (registry) path.
- 14 new unit tests in `src/registry.rs` covering: `probe_dts`
  return-value bands for valid / truncated / invalid input, FourCC
  tag resolution for both `dts` and `dtsc`, the eager
  header-parse path on `send_packet`, `Error::Unsupported` on
  `receive_frame` after a parsed header, `Error::NeedMore` and
  `Error::Eof` boundary cases, and `reset` clearing cached state.
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
