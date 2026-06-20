//! §D.4 Dynamic Range Control (`RANGE`) look-up table for the DTS Core
//! §5.4.1 side-information `DYNF != 0` tail.
//!
//! Transcribed verbatim from ETSI TS 102 114 V1.3.1 (2011-08) Annex D
//! §D.4 "Dynamic Range Control" (staged at
//! `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`, PDF
//! p.195-197; CSV mirror + provenance at
//! `docs/audio/dts/tables/dts-d4-drc-range.csv` /
//! `dts-d4-drc.meta.md`). The spec table is laid out as two
//! index/value groups per printed row; this module preserves the
//! `Multiplier` column only (the linear gain factor), indexed by the
//! 8-bit `nIndex` field `0..=255`. The companion `Q18 binary` column
//! is the same value in fixed point and the `Log Multiplier (dB)`
//! column is informative (an exact 0.25 dB ramp from −31.75 dB to
//! +32.00 dB) — neither is needed for a floating-point decode.
//!
//! Per the §5.4.1 Table 5-28 pseudocode the looked-up `RANGE`
//! multiplies every reconstructed PCM sample, applied **after** the
//! §C.2.5 QMF synthesis:
//!
//! ```text
//! if ( DYNF != 0 ) {
//!   nIndex = ExtractBits(8);
//!   RANGEtbl.LookUp(nIndex, RANGE);
//!   for (ch=0; ch<nPCHS; ch++)
//!     for (n=0; n<nNumSamples; n++)
//!       AudioCh[ch].ReconstructedSamples[n] *= RANGE;
//! }
//! ```
//!
//! Index `127` maps to the unity multiplier `1.0000` (0.0000 dB),
//! confirming the index alignment.
//!
//! This module is feature-independent (no `oxideav-core` dep), so it
//! is available under both the default and `--no-default-features`
//! build modes.

/// Number of entries in the §D.4 `RANGE` table — the full range of the
/// 8-bit `nIndex` field (`ExtractBits(8)`).
pub const DRC_RANGE_LEN: usize = 256;

/// The §D.4 unity-gain index (`RANGE == 1.0000`, `0.0000` dB).
pub const DRC_RANGE_UNITY_INDEX: usize = 127;

/// §D.4 Dynamic Range Control multiplier table (`RANGEtbl`), indexed by
/// the 8-bit `nIndex`. Entry `i` is the linear gain applied to every
/// reconstructed PCM sample when `DYNF != 0` (see [`drc_range`]).
///
/// Transcribed from ETSI TS 102 114 V1.3.1 §D.4, "Multiplier" column,
/// indices `0..=255`.
pub static DRC_RANGE_MULTIPLIER: [f64; DRC_RANGE_LEN] = [
    0.0259, 0.0266, 0.0274, 0.0282, 0.029, 0.0299, 0.0307, 0.0316, 0.0325, 0.0335, 0.0345, 0.0355,
    0.0365, 0.0376, 0.0387, 0.0398, 0.041, 0.0422, 0.0434, 0.0447, 0.046, 0.0473, 0.0487, 0.0501,
    0.0516, 0.0531, 0.0546, 0.0562, 0.0579, 0.0596, 0.0613, 0.0631, 0.0649, 0.0668, 0.0688, 0.0708,
    0.0729, 0.075, 0.0772, 0.0794, 0.0818, 0.0841, 0.0866, 0.0891, 0.0917, 0.0944, 0.0972, 0.1,
    0.1029, 0.1059, 0.109, 0.1122, 0.1155, 0.1189, 0.1223, 0.1259, 0.1296, 0.1334, 0.1372, 0.1413,
    0.1454, 0.1496, 0.154, 0.1585, 0.1631, 0.1679, 0.1728, 0.1778, 0.183, 0.1884, 0.1939, 0.1995,
    0.2054, 0.2113, 0.2175, 0.2239, 0.2304, 0.2371, 0.2441, 0.2512, 0.2585, 0.2661, 0.2738, 0.2818,
    0.2901, 0.2985, 0.3073, 0.3162, 0.3255, 0.335, 0.3447, 0.3548, 0.3652, 0.3758, 0.3868, 0.3981,
    0.4097, 0.4217, 0.434, 0.4467, 0.4597, 0.4732, 0.487, 0.5012, 0.5158, 0.5309, 0.5464, 0.5623,
    0.5788, 0.5957, 0.6131, 0.631, 0.6494, 0.6683, 0.6879, 0.7079, 0.7286, 0.7499, 0.7718, 0.7943,
    0.8175, 0.8414, 0.866, 0.8913, 0.9173, 0.9441, 0.9716, 1.0, 1.0292, 1.0593, 1.0902, 1.122,
    1.1548, 1.1885, 1.2232, 1.2589, 1.2957, 1.3335, 1.3725, 1.4125, 1.4538, 1.4962, 1.5399, 1.5849,
    1.6312, 1.6788, 1.7278, 1.7783, 1.8302, 1.8836, 1.9387, 1.9953, 2.0535, 2.1135, 2.1752, 2.2387,
    2.3041, 2.3714, 2.4406, 2.5119, 2.5852, 2.6607, 2.7384, 2.8184, 2.9007, 2.9854, 3.0726, 3.1623,
    3.2546, 3.3497, 3.4475, 3.5481, 3.6517, 3.7584, 3.8681, 3.9811, 4.0973, 4.217, 4.3401, 4.4668,
    4.5973, 4.7315, 4.8697, 5.0119, 5.1582, 5.3088, 5.4639, 5.6234, 5.7876, 5.9566, 6.1306, 6.3096,
    6.4938, 6.6834, 6.8786, 7.0795, 7.2862, 7.4989, 7.7179, 7.9433, 8.1752, 8.414, 8.6596, 8.9125,
    9.1728, 9.4406, 9.7163, 10.0, 10.292, 10.5925, 10.9018, 11.2202, 11.5478, 11.885, 12.2321,
    12.5893, 12.9569, 13.3352, 13.7246, 14.1254, 14.5378, 14.9624, 15.3993, 15.8489, 16.3117,
    16.788, 17.2783, 17.7828, 18.3021, 18.8365, 19.3865, 19.9526, 20.5353, 21.1349, 21.752,
    22.3872, 23.0409, 23.7137, 24.4062, 25.1189, 25.8523, 26.6073, 27.3842, 28.1838, 29.0068,
    29.8538, 30.7256, 31.6228, 32.5462, 33.4965, 34.4747, 35.4813, 36.5174, 37.5837, 38.6812,
    39.8107,
];

/// Look up the §D.4 Dynamic Range Control `RANGE` multiplier for an
/// 8-bit `nIndex` (`ExtractBits(8)`), returning the linear gain factor
/// applied to every reconstructed PCM sample when `DYNF != 0`.
///
/// `index` is taken modulo nothing — it is the raw 8-bit field, so all
/// `u8` values are in range by construction.
#[must_use]
pub fn drc_range(index: u8) -> f64 {
    DRC_RANGE_MULTIPLIER[index as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_full_8bit_range() {
        assert_eq!(DRC_RANGE_MULTIPLIER.len(), 256);
        assert_eq!(DRC_RANGE_LEN, 256);
    }

    #[test]
    fn unity_at_index_127() {
        // §D.4: index 127 -> Multiplier 1.0000 (0.0000 dB).
        assert_eq!(drc_range(127), 1.0);
        assert_eq!(DRC_RANGE_UNITY_INDEX, 127);
    }

    #[test]
    fn anchor_rows_match_spec() {
        // Verbatim §D.4 anchor values from the staged PDF.
        assert_eq!(drc_range(0), 0.0259); //  -31.75 dB
        assert_eq!(drc_range(47), 0.1); //    -20.00 dB
        assert_eq!(drc_range(80), 0.2585); // -11.75 dB
        assert_eq!(drc_range(127), 1.0); //     0.00 dB
        assert_eq!(drc_range(128), 1.0292); //  0.25 dB
        assert_eq!(drc_range(207), 10.0); //   20.00 dB
        assert_eq!(drc_range(255), 39.8107); // 32.00 dB
    }

    #[test]
    fn table_is_strictly_monotone_increasing() {
        // The §D.4 multiplier rises monotonically with the index (the
        // dB column is an exact 0.25 dB ramp), so every successor is
        // strictly larger.
        for i in 1..DRC_RANGE_LEN {
            assert!(
                DRC_RANGE_MULTIPLIER[i] > DRC_RANGE_MULTIPLIER[i - 1],
                "entry {i} not greater than predecessor"
            );
        }
    }

    #[test]
    fn multiplier_tracks_log_db_column() {
        // Cross-check the transcribed Multiplier column against the
        // informative Log-Multiplier(dB) column: dB[i] = -31.75 + 0.25*i,
        // and Multiplier ≈ 10^(dB/20) to within the spec's 4-decimal
        // rounding.
        for (i, &actual) in DRC_RANGE_MULTIPLIER.iter().enumerate() {
            let db = -31.75 + 0.25 * i as f64;
            let predicted = 10f64.powf(db / 20.0);
            let rel = (predicted - actual).abs() / actual;
            assert!(
                rel < 0.01,
                "index {i}: rel err {rel} (pred {predicted}, got {actual})"
            );
        }
    }
}
