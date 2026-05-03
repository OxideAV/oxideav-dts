//! High-frequency VQ codebook for DTS Core (and X96 ABITS=1).
//!
//! Provenance: `docs/audio/dts/data/dts-vq-codebook.md`. The full
//! codebook is `[1024][32]` signed 8-bit (32 KB). The clean-room
//! sidecar embeds only the first 16 entries verbatim (entry 0 is
//! all-zeros silence, plus 15 illustrative entries) and recommends
//! pulling the remaining 1008 entries from
//! `libavcodec/dcadata.c` lines 4240..6290.
//!
//! The workspace policy forbids consulting any third-party library
//! source for the unembedded entries. Round 1 therefore ships the
//! 16 documented entries and zero-fills the remainder; this affects
//! decoder fidelity only when the encoder picks a VQ index ≥ 16
//! (rare for low-frequency content, common for high-frequency
//! transients). Documented in README "Backlog (round 2+)".

#![allow(dead_code)]

pub const VQ_ENTRIES: usize = 1024;
pub const VQ_VECTOR: usize = 32;

/// Resolve the 32 samples of VQ entry `idx` (0..1023). Out-of-range
/// indices return all zeros.
pub fn lookup(idx: usize) -> [i8; VQ_VECTOR] {
    if idx < KNOWN_ENTRIES.len() {
        KNOWN_ENTRIES[idx]
    } else {
        [0; VQ_VECTOR]
    }
}

/// First 16 entries of the high-frequency VQ codebook, sourced
/// verbatim from `dts-vq-codebook.md` §3 (entries 0..15 inline).
const KNOWN_ENTRIES: &[[i8; VQ_VECTOR]] = &[
    // [0] silence
    [0; 32],
    // [1]
    [
        -4, -2, 2, 1, -16, -10, 1, 3, 1, 0, 6, 1, -3, 7, 1, -22,
        2, -4, -3, 11, 14, 6, -1, 1, -13, 29, -28, 10, 10, -8, 0, -9,
    ],
    // [2]
    [
        -8, 8, -7, 10, -3, -12, -5, -8, 1, -2, 9, -2, -5, -18, 1, 9,
        -8, -8, 3, 41, 7, -9, -9, 22, -42, -29, 14, -18, -14, -32, 1, -15,
    ],
    // [3]
    [
        -16, 8, 15, 16, -16, 5, 2, 7, -6, -16, -7, 1, 1, -3, -2, 0,
        8, 20, -26, -11, 2, -17, 0, -3, -34, -37, 10, 44, -2, 22, 2, -4,
    ],
    // [4]
    [
        7, 14, 5, 6, 15, -1, 3, -3, -9, -23, -5, -14, 8, -1, -14, -6,
        -5, -8, 54, 31, -6, 18, 2, -19, -2, -11, -30, -6, -19, 2, -2, -14,
    ],
    // [5]
    [
        1, 2, -2, -1, -3, -3, 1, -5, 1, -3, -4, -8, 5, -4, 0, 1,
        3, 7, -5, -4, -3, -12, 3, -2, -3, 12, -53, -51, 6, -1, 6, 8,
    ],
    // [6]
    [
        0, -1, 5, 1, -6, -8, 7, 5, -18, -4, -1, 1, 0, -3, -3, -14,
        -1, -6, 0, -14, -1, -1, 5, -3, -11, 1, -20, 10, 2, 19, -2, -2,
    ],
    // [7]
    [
        2, 4, 3, 0, 5, 0, 3, 1, -2, 0, -6, -3, -4, -5, -3, -3,
        -7, 0, -34, 4, -43, 17, 0, -53, -13, -7, 24, 14, 5, -18, 9, -20,
    ],
    // [8]
    [
        1, 0, -3, 2, 3, -5, -2, 7, -21, 5, -25, 23, 11, -28, 2, 1,
        -11, 9, 13, -6, -12, 5, 7, 2, 4, -11, -6, -1, 8, 0, 1, -2,
    ],
    // [9]
    [
        2, -4, -6, -4, 0, -5, -29, 13, -6, -22, -3, -43, 12, -41, 5, 24,
        18, -9, -36, -6, 4, -7, -4, 13, 4, -15, -1, -5, 1, 2, -5, 4,
    ],
    // [10]
    [
        0, -1, 13, -6, -5, 1, 0, -3, 1, -5, 19, -22, 31, -27, 4, -15,
        -6, 15, 9, -13, 1, -9, 10, -17, 4, -1, -1, 4, 2, 0, -3, -5,
    ],
    // [11]
    [
        -7, 3, -8, 13, 19, -12, 8, -19, -3, -2, -24, 31, 14, 0, 7, -13,
        -18, 0, 3, 6, 13, -2, 1, -12, -21, 9, -2, 30, 21, -14, 2, -14,
    ],
    // [12]
    [
        -3, -7, 8, -1, -2, -9, 6, 1, -7, 7, 13, 3, -1, -10, 30, 4,
        -10, 12, 5, 6, -13, -7, -4, -2, -2, 7, -3, -6, 3, 4, 1, 2,
    ],
    // [13]
    [
        -8, 9, 2, -3, -5, 2, 0, 9, 3, 7, -4, -16, -13, 3, 23, -27,
        18, 46, -38, 6, 4, 43, -1, 0, 8, -7, -4, -1, 11, -7, 6, -3,
    ],
    // [14]
    [
        1, 1, 18, -8, -6, 0, 3, 4, 22, -3, -4, -2, -4, -11, 40, -7,
        -3, -13, -14, -7, -10, 14, 7, 5, -14, 11, -5, 7, 21, -2, 9, -3,
    ],
    // [15]
    [
        0, 0, -2, 4, -2, 0, 2, 0, -1, 2, -1, 0, 0, 2, 2, 2,
        -1, 1, -3, -1, -15, -2, -63, -27, -21, -47, -14, 1, -14, 10, 0, 2,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_0_is_silence() {
        let e = lookup(0);
        for v in e {
            assert_eq!(v, 0);
        }
    }

    #[test]
    fn entry_1_first_sample() {
        let e = lookup(1);
        assert_eq!(e[0], -4);
        assert_eq!(e[1], -2);
    }

    #[test]
    fn out_of_range_zero() {
        let e = lookup(2000);
        for v in e {
            assert_eq!(v, 0);
        }
    }

    #[test]
    fn entry_15_last_sample() {
        let e = lookup(15);
        assert_eq!(e[31], 2);
    }
}
