//! Per-sample input assembly + shift-register update for the DTS
//! Core 32-band synthesis QMF.
//!
//! These two FIR-independent primitives bracket round 255's
//! [`crate::cos_mod_stage`] inside the §C.2.5 `QMFInterpolation()`
//! per-sample loop body, transcribed verbatim in
//! `docs/audio/dts/dts-core-extracts.md` §2.4 (ETSI TS 102 114
//! V1.3.1 Annex C §C.2.5, staged PDF p.185).
//!
//! Per the staged §2.4 pseudocode, the body of the outer
//! `for (nSubIndex=nStart; nSubIndex<nEnd; nSubIndex++)` loop is:
//!
//! ```text
//!     // (a) Per-sample raXin assembly — this module's
//!     //     assemble_xin().
//!     for (i=0;     i<nSUBS;       i++) raXin[i] = aSubband[i].raSample[nSubIndex];
//!     for (i=nSUBS; i<NumSubband;  i++) raXin[i] = 0.0;
//!
//!     // (b) Cosine-modulation stage — round 255 cos_mod_stage().
//!     //     Reads raXin[], raCosMod[], writes raX[0..32].
//!
//!     // (c) 512-tap FIR convolution against the §D.8 prCoeff set —
//!     //     deferred until docs gap #9 closes.
//!
//!     // (d) PCM output step — deferred (depends on (c)).
//!
//!     // (e) Per-sample shift-register update — this module's
//!     //     shift_x_history() plus the still-deferred raZ[]
//!     //     rotate (depends on (c)/(d)).
//!     for (i=511; i>=32; i--)        raX[i]    = raX[i-32];
//!     for (i=0; i<NumSubband; i++)   raZ[i]    = raZ[i+32];
//!     for (i=0; i<NumSubband; i++)   raZ[i+32] = (real)0.0;
//! ```
//!
//! Steps (a) and (e)'s `raX` shift only depend on `nSUBS`, the
//! per-subband sample arrays, and the synthesis filter's `raX[]`
//! shift register — they are entirely independent of the §D.8
//! `raCoeffLossy` / `raCoeffLossLess` 512-tap FIR coefficient tables
//! (still pending docs staging, round-208 docs gap #9). They can
//! therefore be landed ahead of the FIR step that fuses them into
//! the full driver.
//!
//! The `raZ[]` rotate at the end of step (e) operates on the
//! output of step (c)'s FIR convolution, so its semantics are
//! entangled with the FIR step and it is *not* implemented here —
//! it will land alongside the FIR coefficient tables.

use crate::cos_mod::NUM_SUBBAND;

/// Length of the synthesis filter's `raX[]` shift register, per
/// §C.2.5 / `dts-core-extracts.md` §2.4.
///
/// The staged pseudocode declares `raX[]` implicitly via its post-
/// PCM-output shift step:
///
/// ```text
///     for (i=511; i>=32; i--) raX[i] = raX[i-32];
/// ```
///
/// — the loop's upper bound (`i = 511`) fixes `raX[]` at 512
/// entries, matching the 512-tap `prCoeff` set (`raCoeffLossy` /
/// `raCoeffLossLess`, §D.8) the FIR step that drives `raX[]`
/// consumes.
pub const X_HISTORY_LEN: usize = 512;

/// Per-sample subband-input assembly step of the §C.2.5
/// `QMFInterpolation()` outer-loop body (per
/// `dts-core-extracts.md` §2.4, PDF p.185, lines 182-183 of the
/// staged pseudocode).
///
/// Builds the per-sample input vector `raXin[0..32]` that
/// [`crate::cos_mod_stage`] consumes by:
///
/// 1. Copying `subband_samples[i]` into `raXin[i]` for every
///    active subband `i ∈ 0..n_subs`, per the spec's
///    `for (i=0; i<nSUBS; i++) raXin[i] = aSubband[i].raSample[nSubIndex];`.
/// 2. Zero-filling `raXin[i]` for every inactive subband
///    `i ∈ n_subs..NumSubband`, per the spec's
///    `for (i=nSUBS; i<NumSubband; i++) raXin[i] = 0.0;`.
///
/// `subband_samples[i]` is the single sample `aSubband[i].raSample[nSubIndex]`
/// the spec reads at sample-index `nSubIndex` — the caller has
/// already selected `nSubIndex` from each active subband's sample
/// array. This narrows the assembly step to a pure vector-build
/// over per-subband scalars (independent of the per-subband sample
/// array layout that future bitstream-decode rounds will land).
///
/// `n_subs` is the spec's per-channel `nSUBS[ch]` — the count of
/// active subbands the channel transmitted, per the §C.2.5
/// `QMFInterpolation(FILTS, int nSUBS)` signature; it must be in
/// `0..=NUM_SUBBAND` (a `n_subs == 0` channel is a fully-silenced
/// pass through `raXin = [0.0; 32]`, valid by spec; `n_subs ==
/// NUM_SUBBAND` is the full 32-band case with no inactive tail).
///
/// Returns `Err(QmfAssembleError::SubsOutOfRange)` if
/// `n_subs > NUM_SUBBAND`, and `Err(QmfAssembleError::SampleSliceTooShort)`
/// if `subband_samples.len() < n_subs` (the caller didn't supply
/// one scalar per active subband). The per-call zero-fill of the
/// inactive tail is guaranteed even when the caller's
/// `subband_samples` slice is longer than `n_subs` — the spec's
/// zero-fill step ignores the high end past `nSUBS`.
pub fn assemble_xin(
    subband_samples: &[f64],
    n_subs: usize,
) -> Result<[f64; NUM_SUBBAND], QmfAssembleError> {
    if n_subs > NUM_SUBBAND {
        return Err(QmfAssembleError::SubsOutOfRange { n_subs });
    }
    if subband_samples.len() < n_subs {
        return Err(QmfAssembleError::SampleSliceTooShort {
            provided: subband_samples.len(),
            required: n_subs,
        });
    }

    // Step (a)(1): active subbands raXin[0..nSUBS]. Bulk-copy is
    // semantically identical to the spec's
    // `for (i=0; i<nSUBS; i++) raXin[i] = aSubband[i].raSample[nSubIndex];`
    // (the runtime-i and runtime-sample-array forms compile to the
    // same memcpy on contiguous f64s).
    let mut ra_xin = [0.0_f64; NUM_SUBBAND];
    ra_xin[..n_subs].copy_from_slice(&subband_samples[..n_subs]);
    // Step (a)(2): inactive subbands raXin[nSUBS..32] = 0.0.
    // The array starts at 0.0 from the [0.0_f64; NUM_SUBBAND]
    // initialiser above; no per-index re-zeroing is needed.
    Ok(ra_xin)
}

/// Per-sample shift-register update for the synthesis filter's
/// `raX[]` register, per `dts-core-extracts.md` §2.4 line 217
/// (`for (i=511; i>=32; i--) raX[i] = raX[i-32];`).
///
/// Rotates the 512-entry `raX[]` register by 32 entries toward the
/// high end: after the call, `raX[32..512]` holds what `raX[0..480]`
/// held on entry, and `raX[0..32]` is left untouched. The §C.2.5
/// driver overwrites `raX[0..32]` with the next per-sample
/// cosine-modulation output ([`crate::cos_mod_stage`]) before the
/// following FIR step reads it.
///
/// The shift runs from `i = 511` down to `i = 32` (inclusive) so
/// each write reads a slot that has not yet been overwritten —
/// directly translating the spec's reverse-iteration pseudocode.
/// `raX[0..32]` is left undefined after the shift (the spec's next
/// per-sample step writes those slots from `cos_mod_stage`'s
/// output before the FIR convolution reads them); callers are
/// expected to immediately overwrite that range before the next
/// FIR step.
///
/// This primitive is independent of the §D.8 `raCoeffLossy` /
/// `raCoeffLossLess` 512-tap FIR coefficient tables: it only
/// rotates the shift register's content, never reads any
/// coefficients.
pub fn shift_x_history(ra_x: &mut [f64; X_HISTORY_LEN]) {
    // Walk from i=511 down to i=32 (inclusive), writing
    // raX[i] = raX[i - 32]. The reverse iteration is essential —
    // forward iteration would overwrite low-index entries before
    // the high-index entries that depend on them are read.
    for i in (NUM_SUBBAND..X_HISTORY_LEN).rev() {
        ra_x[i] = ra_x[i - NUM_SUBBAND];
    }
}

/// Error returned by [`assemble_xin`] when its inputs violate the
/// spec's preconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QmfAssembleError {
    /// `n_subs` exceeds the §C.2.5 `NumSubband = 32` cap, so the
    /// active-subband loop would write past the end of the
    /// `raXin[0..32]` vector.
    SubsOutOfRange {
        /// The out-of-range `n_subs` value the caller passed.
        n_subs: usize,
    },
    /// The caller supplied fewer than `n_subs` per-subband samples,
    /// so the assembly loop would read past the end of
    /// `subband_samples`.
    SampleSliceTooShort {
        /// `subband_samples.len()` — the number of per-subband
        /// scalars the caller supplied.
        provided: usize,
        /// The minimum length the §C.2.5 step requires
        /// (`n_subs`).
        required: usize,
    },
}

impl core::fmt::Display for QmfAssembleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            QmfAssembleError::SubsOutOfRange { n_subs } => {
                write!(
                    f,
                    "n_subs={n_subs} exceeds NumSubband={NUM_SUBBAND} for the §C.2.5 32-band synthesis QMF"
                )
            }
            QmfAssembleError::SampleSliceTooShort { provided, required } => {
                write!(
                    f,
                    "subband_samples.len()={provided} is shorter than the n_subs={required} per-sample scalars required by §C.2.5"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------
    // assemble_xin() — per-sample raXin assembly.
    // -----------------------------------------------------------

    #[test]
    fn assemble_xin_full_active_count_copies_all_thirty_two_entries() {
        // nSUBS = 32: every slot is an active subband, no
        // zero-fill tail.
        let samples: Vec<f64> = (0..NUM_SUBBAND).map(|i| (i as f64) + 0.5).collect();
        let ra_xin = assemble_xin(&samples, NUM_SUBBAND).expect("assemble succeeds at n_subs=32");
        for (i, v) in ra_xin.iter().enumerate() {
            assert_eq!(*v, (i as f64) + 0.5, "raXin[{i}] = {v} mismatch");
        }
    }

    #[test]
    fn assemble_xin_zero_active_count_produces_silent_input() {
        // nSUBS = 0: spec's first loop does nothing, second loop
        // zeros the entire raXin[0..32]. Valid by the §C.2.5
        // signature.
        let samples: Vec<f64> = vec![];
        let ra_xin = assemble_xin(&samples, 0).expect("assemble succeeds at n_subs=0");
        for (i, v) in ra_xin.iter().enumerate() {
            assert_eq!(*v, 0.0, "raXin[{i}] = {v} should be zero");
        }
    }

    #[test]
    fn assemble_xin_partial_active_count_zero_fills_inactive_tail() {
        // nSUBS = 5: raXin[0..5] gets the supplied samples,
        // raXin[5..32] = 0.0.
        let samples = [1.0, 2.0, 3.0, 4.0, 5.0];
        let ra_xin = assemble_xin(&samples, 5).expect("assemble succeeds at n_subs=5");
        for (i, expected) in samples.iter().enumerate() {
            assert_eq!(ra_xin[i], *expected, "raXin[{i}] active mismatch");
        }
        for (i, v) in ra_xin.iter().enumerate().skip(5) {
            assert_eq!(*v, 0.0, "raXin[{i}] should be zero-filled");
        }
    }

    #[test]
    fn assemble_xin_ignores_extra_trailing_samples_past_n_subs() {
        // §C.2.5's inactive-fill step zeros raXin past nSUBS even
        // if the caller's sample slice has more entries; the
        // assembly must follow nSUBS exactly, not the slice
        // length.
        let samples: Vec<f64> = (0..NUM_SUBBAND).map(|i| 100.0 + i as f64).collect();
        let ra_xin = assemble_xin(&samples, 7).expect("assemble succeeds at n_subs=7");
        for (i, v) in ra_xin.iter().enumerate().take(7) {
            assert_eq!(*v, 100.0 + i as f64, "raXin[{i}] active mismatch");
        }
        for (i, v) in ra_xin.iter().enumerate().skip(7) {
            assert_eq!(
                *v, 0.0,
                "raXin[{i}] should be zero past nSUBS regardless of slice length"
            );
        }
    }

    #[test]
    fn assemble_xin_rejects_n_subs_past_thirty_two() {
        // n_subs = 33 would write past raXin[31]; the §C.2.5
        // signature caps nSUBS at NumSubband = 32.
        let samples = [0.0_f64; 64];
        let err = assemble_xin(&samples, 33).expect_err("n_subs=33 rejected");
        assert_eq!(err, QmfAssembleError::SubsOutOfRange { n_subs: 33 });
    }

    #[test]
    fn assemble_xin_rejects_short_sample_slice() {
        // n_subs = 4 but only 2 samples supplied.
        let samples = [10.0, 20.0];
        let err = assemble_xin(&samples, 4).expect_err("short slice rejected");
        assert_eq!(
            err,
            QmfAssembleError::SampleSliceTooShort {
                provided: 2,
                required: 4
            }
        );
    }

    #[test]
    fn assemble_xin_accepts_exact_length_sample_slice() {
        // Boundary: subband_samples.len() == n_subs (no trailing
        // slack) — spec only requires nSUBS scalars.
        let samples = [7.5, 8.25, 9.125];
        let ra_xin = assemble_xin(&samples, 3).expect("exact-length slice accepted");
        assert_eq!(ra_xin[0], 7.5);
        assert_eq!(ra_xin[1], 8.25);
        assert_eq!(ra_xin[2], 9.125);
        for v in ra_xin.iter().skip(3) {
            assert_eq!(*v, 0.0);
        }
    }

    #[test]
    fn assemble_xin_preserves_negative_and_subnormal_values() {
        // Bit-identical pass-through for the active range:
        // §C.2.5 reads `raXin[i] = aSubband[i].raSample[...]`
        // without scaling. Cover signed + subnormal inputs to
        // confirm the copy preserves them verbatim.
        let samples = [-1.5_f64, f64::MIN_POSITIVE / 4.0, -0.0, 3.0];
        let ra_xin = assemble_xin(&samples, 4).expect("assemble succeeds");
        for (i, sample) in samples.iter().enumerate() {
            assert_eq!(
                ra_xin[i].to_bits(),
                sample.to_bits(),
                "raXin[{i}] bit-mismatch"
            );
        }
    }

    #[test]
    fn assemble_xin_inactive_tail_is_positive_zero() {
        // The spec writes `raXin[i] = 0.0;` — positive zero. A
        // negative-zero in the tail would be a spec deviation that
        // could perturb the cosine-modulation stage's behaviour at
        // `i=0`'s asymmetric B[k] = raXin[0] * raCosMod[…] step.
        let samples = [1.0];
        let ra_xin = assemble_xin(&samples, 1).expect("assemble succeeds");
        for (i, v) in ra_xin.iter().enumerate().skip(1) {
            assert_eq!(
                v.to_bits(),
                0.0_f64.to_bits(),
                "raXin[{i}] should be +0.0 bit-pattern"
            );
        }
    }

    // -----------------------------------------------------------
    // shift_x_history() — post-PCM shift of the raX[] register.
    // -----------------------------------------------------------

    #[test]
    fn shift_x_history_moves_low_half_to_high_half_by_thirty_two() {
        // After the shift, raX[32..512] should hold what
        // raX[0..480] held on entry.
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        for (i, slot) in ra_x.iter_mut().enumerate() {
            *slot = i as f64;
        }
        let snapshot = ra_x;
        shift_x_history(&mut ra_x);
        for (i, v) in ra_x.iter().enumerate().skip(NUM_SUBBAND) {
            assert_eq!(
                *v,
                snapshot[i - NUM_SUBBAND],
                "raX[{i}] should equal pre-shift raX[{}]",
                i - NUM_SUBBAND
            );
        }
    }

    #[test]
    fn shift_x_history_leaves_first_thirty_two_entries_untouched() {
        // raX[0..32] is not written by the shift (the spec's loop
        // condition is `i >= 32`); the driver overwrites them
        // immediately afterwards via cos_mod_stage().
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        for (i, slot) in ra_x.iter_mut().enumerate() {
            *slot = (i as f64) * 0.25;
        }
        let snapshot_low: Vec<f64> = ra_x[..NUM_SUBBAND].to_vec();
        shift_x_history(&mut ra_x);
        for (i, expected) in snapshot_low.iter().enumerate() {
            assert_eq!(
                ra_x[i], *expected,
                "raX[{i}] should be unchanged by the shift"
            );
        }
    }

    #[test]
    fn shift_x_history_is_identity_on_uniform_register() {
        // If every entry already holds the same value, the shift
        // is a no-op (each slot is replaced with an equal value).
        let mut ra_x = [4.25_f64; X_HISTORY_LEN];
        shift_x_history(&mut ra_x);
        for (i, v) in ra_x.iter().enumerate() {
            assert_eq!(*v, 4.25, "raX[{i}] = {v} should stay 4.25");
        }
    }

    #[test]
    fn shift_x_history_zeroes_propagate_into_low_block() {
        // A common §C.2.5 startup state: raX[] = 0.0 everywhere.
        // The shift is then a no-op and the register stays silent.
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        shift_x_history(&mut ra_x);
        for (i, v) in ra_x.iter().enumerate() {
            assert_eq!(*v, 0.0, "raX[{i}] = {v} should stay 0");
        }
    }

    #[test]
    fn shift_x_history_top_block_after_shift_is_from_pre_shift_indices_448_to_479() {
        // Spot check: after the shift, raX[480..512] holds what
        // raX[448..480] held on entry.
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        for (i, slot) in ra_x.iter_mut().enumerate() {
            *slot = (i as f64) - 256.0;
        }
        shift_x_history(&mut ra_x);
        for (i, v) in ra_x.iter().enumerate().skip(480) {
            let expected = ((i - NUM_SUBBAND) as f64) - 256.0;
            assert_eq!(
                *v,
                expected,
                "raX[{i}] should be pre-shift raX[{}] = {expected}",
                i - NUM_SUBBAND
            );
        }
    }

    #[test]
    fn shift_x_history_reverse_iteration_does_not_chain_overwrites() {
        // Sanity check: if the implementation walked forward
        // instead of in reverse, raX[32] would be overwritten with
        // raX[0], then raX[64] would be overwritten with raX[32]
        // (which is now raX[0]), etc. — every i ≡ 0 (mod 32) slot
        // would collapse to raX[0]'s original value.
        // Construct an input where this failure mode would be
        // visible (distinct values at the 32-step boundaries) and
        // confirm the reverse-walking implementation handles it
        // correctly.
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        for (i, slot) in ra_x.iter_mut().enumerate() {
            *slot = i as f64; // raX[i] = i, all 512 values distinct
        }
        shift_x_history(&mut ra_x);
        // raX[64] should hold the pre-shift raX[32] = 32, not
        // raX[0] = 0 (the forward-iteration mistake).
        assert_eq!(ra_x[64], 32.0, "raX[64] reverse-shift mismatch");
        assert_eq!(ra_x[96], 64.0, "raX[96] reverse-shift mismatch");
        // And the top of the register holds the highest pre-shift
        // index that's still in range.
        assert_eq!(ra_x[511], (511 - NUM_SUBBAND) as f64);
    }

    #[test]
    fn shift_x_history_repeated_calls_walk_block_by_block() {
        // Two consecutive shifts displace the low block by 64
        // entries; three shifts displace it by 96; etc. — confirm
        // the primitive composes correctly across per-sample
        // iterations.
        let mut ra_x = [0.0_f64; X_HISTORY_LEN];
        for (i, slot) in ra_x.iter_mut().enumerate() {
            *slot = i as f64;
        }
        // Reserve raX[0..32] = sentinel before the first shift
        // (matching what cos_mod_stage would write into raX[0..32]
        // before the next per-sample iteration writes again).
        for v in ra_x.iter_mut().take(NUM_SUBBAND) {
            *v = -1.0;
        }
        shift_x_history(&mut ra_x);
        // After 1 shift, raX[64] = pre-shift raX[32] = 32.
        assert_eq!(ra_x[64], 32.0);
        // Reserve raX[0..32] = sentinel again.
        for v in ra_x.iter_mut().take(NUM_SUBBAND) {
            *v = -2.0;
        }
        shift_x_history(&mut ra_x);
        // After 2 shifts, raX[96] holds what raX[64] held one
        // shift ago, which held what raX[32] held before any
        // shift — i.e., raX[96] = 32.
        assert_eq!(ra_x[96], 32.0);
    }

    // -----------------------------------------------------------
    // Constants
    // -----------------------------------------------------------

    #[test]
    fn x_history_len_is_five_hundred_twelve() {
        // §C.2.5 / §2.4 line 217 caps `raX[]` at 512 entries,
        // matching the 512-tap §D.8 FIR set the driver consumes.
        assert_eq!(X_HISTORY_LEN, 512);
    }

    #[test]
    fn x_history_len_is_a_whole_multiple_of_num_subband() {
        // The shift step writes `raX[i] = raX[i-32]`, which would
        // overrun or under-fill the register if X_HISTORY_LEN
        // weren't a multiple of NUM_SUBBAND. 512 = 16 * 32.
        assert_eq!(X_HISTORY_LEN % NUM_SUBBAND, 0);
        assert_eq!(X_HISTORY_LEN / NUM_SUBBAND, 16);
    }

    // -----------------------------------------------------------
    // Error rendering
    // -----------------------------------------------------------

    #[test]
    fn subs_out_of_range_error_renders_human_readable_message() {
        let err = QmfAssembleError::SubsOutOfRange { n_subs: 64 };
        let s = format!("{err}");
        assert!(s.contains("64"), "message should include the bad n_subs");
        assert!(
            s.contains("NumSubband"),
            "message should reference the spec's cap"
        );
    }

    #[test]
    fn sample_slice_too_short_error_renders_provided_and_required() {
        let err = QmfAssembleError::SampleSliceTooShort {
            provided: 2,
            required: 5,
        };
        let s = format!("{err}");
        assert!(s.contains("2"), "message should include provided length");
        assert!(s.contains("5"), "message should include required length");
    }
}
