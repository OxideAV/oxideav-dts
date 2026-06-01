//! Cosine-modulation coefficient matrix for the DTS Core 32-band
//! synthesis QMF filterbank.
//!
//! Transcribed verbatim from `docs/audio/dts/dts-core-extracts.md`
//! §2.3 ("Cosine-modulation coefficient definition (§C.2.5, PDF
//! p.184)"), which in turn quotes Annex C §C.2.5 `PreCalCosMod()` of
//! ETSI TS 102 114 V1.3.1 (the staged PDF at
//! `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`).
//!
//! The 544-entry array `raCosMod[]` is pre-computed once by the
//! decoder and re-used by every `QMFInterpolation` invocation
//! (§C.2.5, PDF p.185). The §2.4 extract documents the four roles
//! the four blocks play inside that loop:
//!
//! ```text
//!   Block 1 (indices   0..=255): cos((2i+1)(2k+1) π / 64)  — 16×16
//!   Block 2 (indices 256..=511): cos(i(2k+1)     π / 32)   — 16×16
//!   Block 3 (indices 512..=527): + 0.25 / (2·cos((2k+1) π / 128))  — 16
//!   Block 4 (indices 528..=543): − 0.25 / (2·sin((2k+1) π / 128))  — 16
//! ```
//!
//! The j-counter in the spec's `PreCalCosMod()` pseudocode walks
//! 0..=543 with no gaps; this module materialises the same packing.
//!
//! Scope: this round only lands the matrix builder. The downstream
//! `QMFInterpolation` synthesis loop (and the §D.8 512-tap
//! `raCoeffLossy` / `raCoeffLossLess` FIR coefficient tables it
//! consumes) is a follow-up — the §D.8 tables are referenced in the
//! staged PDF p.238 but not yet transcribed under `docs/audio/dts/`.

/// Total number of entries in the [`raCosMod`] array per
/// `PreCalCosMod()` (§C.2.5 / `dts-core-extracts.md` §2.3).
///
/// Decomposes as 256 + 256 + 16 + 16 (the four blocks of the
/// spec's pseudocode).
pub const COS_MOD_LEN: usize = 544;

/// Start index of Block 1 inside [`raCosMod`]
/// (`cos((2i+1)(2k+1) π / 64)`).
pub const COS_MOD_BLOCK1_START: usize = 0;

/// Start index of Block 2 inside [`raCosMod`]
/// (`cos(i(2k+1) π / 32)`).
pub const COS_MOD_BLOCK2_START: usize = 256;

/// Start index of Block 3 inside [`raCosMod`]
/// (`+0.25 / (2·cos((2k+1) π / 128))`).
pub const COS_MOD_BLOCK3_START: usize = 512;

/// Start index of Block 4 inside [`raCosMod`]
/// (`−0.25 / (2·sin((2k+1) π / 128))`).
pub const COS_MOD_BLOCK4_START: usize = 528;

/// Pre-compute the 544-entry cosine-modulation matrix.
///
/// This is a direct Rust transliteration of the §C.2.5
/// `PreCalCosMod()` pseudocode transcribed in
/// `docs/audio/dts/dts-core-extracts.md` §2.3:
///
/// ```text
///   PreCalCosMod() {
///       for (j=0,k=0; k<16; k++)
///           for (i=0; i<16; i++)
///               raCosMod[j++] = cos((2*i+1)*(2*k+1)*Pi/64);
///       for (k=0; k<16; k++)
///           for (i=0; i<16; i++)
///               raCosMod[j++] = cos((i)*(2*k+1)*Pi/32);
///       for (k=0; k<16; k++)
///           raCosMod[j++] = 0.25 / (2*cos((2*k+1)*Pi/128));
///       for (k=0; k<16; k++)
///           raCosMod[j++] = -0.25 / (2*sin((2*k+1)*Pi/128));
///   }
/// ```
///
/// The returned array is intended to be allocated once per decoder
/// instance (§C.2.5: "computed once") and shared across every
/// `QMFInterpolation` call for the lifetime of that instance.
///
/// The output is deterministic: every byte-identical run produces
/// the same array. Callers that need bit-exact reproducibility
/// across runs can rely on this directly without seeding a PRNG.
pub fn precal_cos_mod() -> [f64; COS_MOD_LEN] {
    let mut ra = [0.0_f64; COS_MOD_LEN];
    let mut j = 0usize;

    // Block 1: indices 0..256
    //   raCosMod[j++] = cos((2*i+1)*(2*k+1) * π / 64)
    for k in 0..16 {
        for i in 0..16 {
            let num = ((2 * i + 1) * (2 * k + 1)) as f64;
            ra[j] = (num * core::f64::consts::PI / 64.0).cos();
            j += 1;
        }
    }
    debug_assert_eq!(j, COS_MOD_BLOCK2_START);

    // Block 2: indices 256..512
    //   raCosMod[j++] = cos(i * (2*k+1) * π / 32)
    for k in 0..16 {
        for i in 0..16 {
            let num = (i * (2 * k + 1)) as f64;
            ra[j] = (num * core::f64::consts::PI / 32.0).cos();
            j += 1;
        }
    }
    debug_assert_eq!(j, COS_MOD_BLOCK3_START);

    // Block 3: indices 512..528
    //   raCosMod[j++] = 0.25 / (2 * cos((2*k+1) * π / 128))
    for k in 0..16 {
        let arg = ((2 * k + 1) as f64) * core::f64::consts::PI / 128.0;
        ra[j] = 0.25 / (2.0 * arg.cos());
        j += 1;
    }
    debug_assert_eq!(j, COS_MOD_BLOCK4_START);

    // Block 4: indices 528..544
    //   raCosMod[j++] = -0.25 / (2 * sin((2*k+1) * π / 128))
    for k in 0..16 {
        let arg = ((2 * k + 1) as f64) * core::f64::consts::PI / 128.0;
        ra[j] = -0.25 / (2.0 * arg.sin());
        j += 1;
    }
    debug_assert_eq!(j, COS_MOD_LEN);

    ra
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for cross-checks between the closed-form values and
    /// the matrix entry. The expected values come from the same
    /// `cos` / `sin` calls in the spec pseudocode, so the residual is
    /// effectively zero on IEEE-754; we leave a small slack for the
    /// theoretical-vs-runtime-rounding mismatch.
    const EPS: f64 = 1e-12;

    #[test]
    fn length_matches_spec() {
        let ra = precal_cos_mod();
        assert_eq!(ra.len(), 544);
        assert_eq!(COS_MOD_LEN, 544);
    }

    #[test]
    fn block_boundaries_are_documented_constants() {
        // Documented in module docs + start-constant docstrings.
        assert_eq!(COS_MOD_BLOCK1_START, 0);
        assert_eq!(COS_MOD_BLOCK2_START, 256);
        assert_eq!(COS_MOD_BLOCK3_START, 512);
        assert_eq!(COS_MOD_BLOCK4_START, 528);
        // Decomposition adds up.
        assert_eq!(
            COS_MOD_BLOCK2_START - COS_MOD_BLOCK1_START + COS_MOD_BLOCK3_START
                - COS_MOD_BLOCK2_START
                + COS_MOD_BLOCK4_START
                - COS_MOD_BLOCK3_START
                + COS_MOD_LEN
                - COS_MOD_BLOCK4_START,
            COS_MOD_LEN
        );
    }

    #[test]
    fn block1_first_entry_is_cos_pi_over_64() {
        // k=0, i=0 → (2·0+1)(2·0+1) π/64 = π/64
        let ra = precal_cos_mod();
        let expected = (core::f64::consts::PI / 64.0).cos();
        assert!((ra[0] - expected).abs() < EPS);
    }

    #[test]
    fn block1_walks_all_256_indices() {
        // Spec pseudocode walks k outer, i inner, both 0..16. The
        // packed index for (k, i) is 16*k + i.
        let ra = precal_cos_mod();
        for k in 0..16 {
            for i in 0..16 {
                let idx = COS_MOD_BLOCK1_START + 16 * k + i;
                let num = ((2 * i + 1) * (2 * k + 1)) as f64;
                let expected = (num * core::f64::consts::PI / 64.0).cos();
                assert!(
                    (ra[idx] - expected).abs() < EPS,
                    "Block 1 ({k}, {i}) mismatch: got {} expected {}",
                    ra[idx],
                    expected
                );
            }
        }
    }

    #[test]
    fn block2_first_entry_is_one() {
        // k=0, i=0 → cos(0·1·π/32) = cos(0) = 1.0
        let ra = precal_cos_mod();
        assert!((ra[COS_MOD_BLOCK2_START] - 1.0).abs() < EPS);
    }

    #[test]
    fn block2_walks_all_256_indices() {
        let ra = precal_cos_mod();
        for k in 0..16 {
            for i in 0..16 {
                let idx = COS_MOD_BLOCK2_START + 16 * k + i;
                let num = (i * (2 * k + 1)) as f64;
                let expected = (num * core::f64::consts::PI / 32.0).cos();
                assert!(
                    (ra[idx] - expected).abs() < EPS,
                    "Block 2 ({k}, {i}) mismatch: got {} expected {}",
                    ra[idx],
                    expected
                );
            }
        }
    }

    #[test]
    fn block3_first_entry_matches_closed_form() {
        // k=0 → 0.25 / (2 · cos(π/128))
        let ra = precal_cos_mod();
        let arg = core::f64::consts::PI / 128.0;
        let expected = 0.25 / (2.0 * arg.cos());
        assert!((ra[COS_MOD_BLOCK3_START] - expected).abs() < EPS);
    }

    #[test]
    fn block3_walks_all_16_indices() {
        let ra = precal_cos_mod();
        for k in 0..16 {
            let idx = COS_MOD_BLOCK3_START + k;
            let arg = ((2 * k + 1) as f64) * core::f64::consts::PI / 128.0;
            let expected = 0.25 / (2.0 * arg.cos());
            assert!(
                (ra[idx] - expected).abs() < EPS,
                "Block 3 ({k}) mismatch: got {} expected {}",
                ra[idx],
                expected
            );
        }
    }

    #[test]
    fn block3_entries_are_strictly_positive() {
        // (2k+1)π/128 is in (0, π/2) for k in 0..16, so cos > 0
        // and the +0.25 / (2·cos) factor is strictly positive.
        let ra = precal_cos_mod();
        for k in 0..16 {
            let v = ra[COS_MOD_BLOCK3_START + k];
            assert!(v > 0.0, "Block 3 ({k}) = {v} should be > 0");
        }
    }

    #[test]
    fn block4_first_entry_matches_closed_form() {
        // k=0 → -0.25 / (2 · sin(π/128))
        let ra = precal_cos_mod();
        let arg = core::f64::consts::PI / 128.0;
        let expected = -0.25 / (2.0 * arg.sin());
        assert!((ra[COS_MOD_BLOCK4_START] - expected).abs() < EPS);
    }

    #[test]
    fn block4_walks_all_16_indices() {
        let ra = precal_cos_mod();
        for k in 0..16 {
            let idx = COS_MOD_BLOCK4_START + k;
            let arg = ((2 * k + 1) as f64) * core::f64::consts::PI / 128.0;
            let expected = -0.25 / (2.0 * arg.sin());
            assert!(
                (ra[idx] - expected).abs() < EPS,
                "Block 4 ({k}) mismatch: got {} expected {}",
                ra[idx],
                expected
            );
        }
    }

    #[test]
    fn block4_entries_are_strictly_negative() {
        // (2k+1)π/128 is in (0, π/2) for k in 0..16, so sin > 0
        // and the -0.25 / (2·sin) factor is strictly negative.
        let ra = precal_cos_mod();
        for k in 0..16 {
            let v = ra[COS_MOD_BLOCK4_START + k];
            assert!(v < 0.0, "Block 4 ({k}) = {v} should be < 0");
        }
    }

    #[test]
    fn block1_last_entry_of_row_zero() {
        // k=0, i=15 → (2·15+1)(2·0+1) π / 64 = 31 π / 64
        let ra = precal_cos_mod();
        let expected = (31.0 * core::f64::consts::PI / 64.0).cos();
        assert!((ra[15] - expected).abs() < EPS);
    }

    #[test]
    fn block1_row_count_and_row_length() {
        // Verify the packing density: 16 rows of 16 columns =
        // 256 entries. Done by cross-checking the index arithmetic
        // matches the explicit row/col enumeration.
        let ra = precal_cos_mod();
        let mut seen = 0usize;
        for k in 0..16 {
            for i in 0..16 {
                let idx = COS_MOD_BLOCK1_START + 16 * k + i;
                let num = ((2 * i + 1) * (2 * k + 1)) as f64;
                let expected = (num * core::f64::consts::PI / 64.0).cos();
                assert!((ra[idx] - expected).abs() < EPS);
                seen += 1;
            }
        }
        assert_eq!(seen, 256);
    }

    #[test]
    fn block2_row_first_entry_is_one_for_all_k() {
        // i=0 always gives cos(0) = 1 regardless of k.
        let ra = precal_cos_mod();
        for k in 0..16 {
            let idx = COS_MOD_BLOCK2_START + 16 * k;
            assert!(
                (ra[idx] - 1.0).abs() < EPS,
                "Block 2 row {k} entry 0 should be 1.0"
            );
        }
    }

    #[test]
    fn block3_value_grows_with_k() {
        // (2k+1)π/128 grows monotonically with k, cos(·) shrinks,
        // so +0.25 / (2·cos) grows monotonically with k.
        let ra = precal_cos_mod();
        for k in 1..16 {
            let prev = ra[COS_MOD_BLOCK3_START + k - 1];
            let cur = ra[COS_MOD_BLOCK3_START + k];
            assert!(
                cur > prev,
                "Block 3 should grow: prev={prev} cur={cur} at k={k}"
            );
        }
    }

    #[test]
    fn block4_magnitude_shrinks_with_k() {
        // (2k+1)π/128 grows, sin(·) grows, so |-0.25 / (2·sin)|
        // shrinks. The entries are negative; their absolute values
        // shrink.
        let ra = precal_cos_mod();
        for k in 1..16 {
            let prev = ra[COS_MOD_BLOCK4_START + k - 1].abs();
            let cur = ra[COS_MOD_BLOCK4_START + k].abs();
            assert!(
                cur < prev,
                "|Block 4| should shrink: prev={prev} cur={cur} at k={k}"
            );
        }
    }

    #[test]
    fn deterministic_across_calls() {
        // Two independent invocations must produce bit-identical
        // arrays (§C.2.5 documents the matrix as computed once and
        // reused; that only makes sense if the result is
        // deterministic).
        let a = precal_cos_mod();
        let b = precal_cos_mod();
        for i in 0..COS_MOD_LEN {
            // Bit-exact, not approximate, because the same `cos` /
            // `sin` arguments are passed in the same order.
            assert_eq!(a[i].to_bits(), b[i].to_bits(), "mismatch at index {i}");
        }
    }

    #[test]
    fn all_entries_are_finite() {
        // No NaN, no ±∞. The Block 3 / 4 denominators are cos / sin
        // at angles strictly inside (0, π/2), so neither vanishes
        // and the division is finite.
        let ra = precal_cos_mod();
        for (i, v) in ra.iter().enumerate() {
            assert!(v.is_finite(), "ra[{i}] = {v} is not finite");
        }
    }

    #[test]
    fn block1_and_block2_entries_are_bounded_by_one() {
        // cos returns a value in [-1, +1]. Block 1 / 2 are pure
        // cosine evaluations.
        let ra = precal_cos_mod();
        for (i, v) in ra[COS_MOD_BLOCK1_START..COS_MOD_BLOCK3_START]
            .iter()
            .enumerate()
        {
            assert!(v.abs() <= 1.0 + EPS, "ra[{i}] = {v} outside [-1, 1]");
        }
    }
}
