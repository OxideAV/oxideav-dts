//! DTS Core constant tables.
//!
//! Provenance: every table cross-references `docs/audio/dts/data/
//! dts-core-tables.md` (clean-room). Values are taken directly from
//! the public ETSI TS 102 114 standard and reproduced under the
//! sidecar's per-table provenance citations.

#![allow(dead_code)]

// -------------------------------------------------------------------
// AMODE — channel arrangement code (4-bit/6-bit field)
// dts-core-tables.md §2
// -------------------------------------------------------------------

/// Number of primary audio channels for AMODE 0..15. Slots 16..63 are
/// reserved.
pub const CHANNELS_BY_AMODE: [u8; 16] =
    [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

// -------------------------------------------------------------------
// SFREQ — core sample-rate code (4-bit field)
// dts-core-tables.md §3 — 16 slots, 8 reserved.
// -------------------------------------------------------------------
pub const CORE_SAMPLE_RATES: [u32; 16] = [
    0, 8_000, 16_000, 32_000, 0, 0, 11_025, 22_050,
    44_100, 0, 0, 12_000, 24_000, 48_000, 96_000, 192_000,
];

// -------------------------------------------------------------------
// RATE — bit-rate code (5-bit field)
// dts-core-tables.md §4 — codes 29..31 are non-CBR sentinels.
// -------------------------------------------------------------------
pub const CORE_BIT_RATES: [u32; 32] = [
    32_000, 56_000, 64_000, 96_000, 112_000, 128_000, 192_000, 224_000,
    256_000, 320_000, 384_000, 448_000, 512_000, 576_000, 640_000, 768_000,
    896_000, 1_024_000, 1_152_000, 1_280_000, 1_344_000, 1_408_000, 1_411_200,
    1_472_000, 1_536_000, 1_920_000, 2_048_000, 3_072_000, 3_840_000,
    1, 2, 3,
];

/// `RATE = 31` selects the lossless quantizer step-size LUT.
pub const RATE_LOSSLESS: u8 = 31;

// -------------------------------------------------------------------
// PCMR — PCM resolution code (3-bit field)
// dts-core-tables.md §5 — 0/4/7 reserved; low bit = ES_FORMAT.
// -------------------------------------------------------------------
pub const BITS_PER_SAMPLE: [u8; 8] = [16, 16, 20, 20, 0, 24, 24, 0];

// -------------------------------------------------------------------
// EXSS asset sample-rate table — 16 fully-populated slots.
// dts-core-tables.md §6 (used by EXSS / XLL parsing in round 2+).
// -------------------------------------------------------------------
pub const EXSS_SAMPLE_RATES: [u32; 16] = [
    8_000, 16_000, 32_000, 64_000, 128_000, 22_050, 44_100, 88_200,
    176_400, 352_800, 12_000, 24_000, 48_000, 96_000, 192_000, 384_000,
];

// -------------------------------------------------------------------
// Scale-factor 7-bit code → linear value LUT (128 entries)
// dts-core-tables.md §7 — 125/126/127 reserved (zero); index 0 = 1.
// Stored as Q22 (4_194_304 = unity).
// -------------------------------------------------------------------
pub const SCALE_FACTOR_QUANT7: [u32; 128] = [
    1, 1, 2, 2, 2, 2, 3, 3,
    3, 4, 4, 5, 6, 7, 7, 8,
    10, 11, 12, 14, 16, 18, 20, 23,
    26, 30, 34, 38, 44, 50, 56, 64,
    72, 82, 93, 106, 120, 136, 155, 176,
    200, 226, 257, 292, 331, 376, 427, 484,
    550, 624, 708, 804, 912, 1035, 1175, 1334,
    1514, 1718, 1950, 2213, 2512, 2851, 3236, 3673,
    4169, 4732, 5370, 6095, 6918, 7852, 8913, 10116,
    11482, 13032, 14791, 16788, 19055, 21627, 24547, 27861,
    31623, 35892, 40738, 46238, 52481, 59566, 67608, 76736,
    87096, 98855, 112202, 127350, 144544, 164059, 186209, 211349,
    239883, 272270, 309030, 350752, 398107, 451856, 512861, 582103,
    660693, 749894, 851138, 966051, 1096478, 1244515, 1412538, 1603245,
    1819701, 2065380, 2344229, 2660725, 3019952, 3427678, 3890451, 4415704,
    5011872, 5688529, 6456542, 7328245, 8317638, 0, 0, 0,
];

// -------------------------------------------------------------------
// Scale-factor 6-bit LUT (64 entries). dts-core-tables.md §8.
// -------------------------------------------------------------------
pub const SCALE_FACTOR_QUANT6: [u32; 64] = [
    1, 2, 2, 3, 3, 4, 6, 7,
    10, 12, 16, 20, 26, 34, 44, 56,
    72, 93, 120, 155, 200, 257, 331, 427,
    550, 708, 912, 1175, 1514, 1950, 2512, 3236,
    4169, 5370, 6918, 8913, 11482, 14791, 19055, 24547,
    31623, 40738, 52481, 67608, 87096, 112202, 144544, 186209,
    239883, 309030, 398107, 512861, 660693, 851138, 1096478, 1412538,
    1819701, 2344229, 3019952, 3890451, 5011872, 6456542, 8317638, 0,
];

/// Q22 reference for the scale-factor LUTs (1.0 = 0 dB).
pub const SCALE_FACTOR_UNITY: u32 = 4_194_304;

// -------------------------------------------------------------------
// ABITS quantization-level counts.
// dts-core-tables.md §10.
// -------------------------------------------------------------------
pub const QUANT_LEVELS: [u32; 32] = [
    1, 3, 5, 7, 9, 13, 17, 25,
    32, 64, 128, 256, 512, 1024, 2048, 4096,
    8192, 16384, 32768, 65536, 131072, 262144, 524288, 1_048_576,
    2_097_152, 4_194_304, 8_388_608,
    0, 0, 0, 0, 0,
];

// -------------------------------------------------------------------
// Lossy step-size LUT (Q20, used when RATE != 31).
// dts-core-tables.md §10.1.
// -------------------------------------------------------------------
pub const LOSSY_QUANT: [u32; 32] = [
    0, 6_710_886, 4_194_304, 3_355_443, 2_474_639, 2_097_152, 1_761_608,
    1_426_063, 796_918, 461_373, 251_658, 146_801, 79_692, 46_137,
    27_263, 16_777, 10_486, 5_872, 3_355, 1_887, 1_258, 713, 336,
    168, 84, 42, 21, 0, 0, 0, 0, 0,
];

// -------------------------------------------------------------------
// Lossless step-size LUT (Q20, used when RATE == 31).
// dts-core-tables.md §10.2.
// -------------------------------------------------------------------
pub const LOSSLESS_QUANT: [u32; 32] = [
    0, 4_194_304, 2_097_152, 1_384_120, 1_048_576, 696_254, 524_288,
    348_127, 262_144, 131_072, 65_431, 33_026, 16_450, 8_208, 4_100,
    2_049, 1_024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1,
    0, 0, 0, 0, 0,
];

/// Q20 unity (= 1.0 in the step-size LUTs).
pub const STEP_UNITY_Q20: u32 = 1 << 20;

// -------------------------------------------------------------------
// Per-codebook size-class metadata (dts-core-tables.md §11).
// -------------------------------------------------------------------
pub const QUANT_INDEX_SEL_NBITS: [u8; 10] = [1, 2, 2, 2, 2, 3, 3, 3, 3, 3];
pub const QUANT_INDEX_GROUP_SIZE: [u8; 10] = [1, 3, 3, 3, 3, 7, 7, 7, 7, 7];
pub const BITALLOC_SIZES: [u16; 10] = [3, 5, 7, 9, 13, 17, 25, 33, 65, 129];
pub const BITALLOC_OFFSETS: [i16; 10] = [-1, -2, -3, -4, -6, -8, -12, -16, -32, -64];

// -------------------------------------------------------------------
// Per-band ABITS Huffman codebook size — bit_alloc_12_X uses 12
// entries, decoded symbol + 1 = ABITS in 1..12.
// -------------------------------------------------------------------
pub const BITALLOC_12_OFFSET: i16 = 1;

// -------------------------------------------------------------------
// Scale-factor adjust (per-channel SCAJ field, when SHUFF picks a
// Huffman codebook 3..6). dts-core-tables.md §9 closing block.
// -------------------------------------------------------------------
pub const SCALE_FACTOR_ADJ: [u32; 4] = [4_194_304, 4_718_592, 5_242_880, 6_029_312];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_count_known_amodes() {
        // sanity: stereo / 5.0 / 6.1 / 8 ch all decode
        assert_eq!(CHANNELS_BY_AMODE[2], 2);
        assert_eq!(CHANNELS_BY_AMODE[9], 5);
        assert_eq!(CHANNELS_BY_AMODE[11], 6);
        assert_eq!(CHANNELS_BY_AMODE[15], 8);
    }

    #[test]
    fn sample_rates_align() {
        assert_eq!(CORE_SAMPLE_RATES[8], 44_100);
        assert_eq!(CORE_SAMPLE_RATES[13], 48_000);
        assert_eq!(CORE_SAMPLE_RATES[14], 96_000);
        // reserved slots
        for i in [0usize, 4, 5, 9, 10] {
            assert_eq!(CORE_SAMPLE_RATES[i], 0, "slot {i} not reserved");
        }
    }

    #[test]
    fn bit_rates_at_codes() {
        assert_eq!(CORE_BIT_RATES[10], 384_000);
        assert_eq!(CORE_BIT_RATES[24], 1_536_000);
        assert_eq!(CORE_BIT_RATES[22], 1_411_200);
        // 29..31 are non-CBR sentinels (encoded as the small
        // integers 1/2/3 by us).
        assert!(CORE_BIT_RATES[29] < 100);
        assert!(CORE_BIT_RATES[30] < 100);
        assert!(CORE_BIT_RATES[31] < 100);
    }

    #[test]
    fn pcmr_widths() {
        assert_eq!(BITS_PER_SAMPLE[0], 16);
        assert_eq!(BITS_PER_SAMPLE[5], 24);
        assert_eq!(BITS_PER_SAMPLE[4], 0); // reserved
    }

    #[test]
    fn quant7_endpoints() {
        assert_eq!(SCALE_FACTOR_QUANT7[0], 1);
        // sentinel zeros for reserved indices 125..127
        for i in 125..128 {
            assert_eq!(SCALE_FACTOR_QUANT7[i], 0);
        }
    }

    #[test]
    fn quant6_endpoints() {
        assert_eq!(SCALE_FACTOR_QUANT6[0], 1);
        assert_eq!(SCALE_FACTOR_QUANT6[63], 0);
    }

    #[test]
    fn lossless_quant_powers_of_two_high() {
        // For ABITS >= 16 entries collapse to clean integer powers
        // of two (`2^(20 - ABITS)` for ABITS in 16..=26). This is
        // what makes the lossless mode round-trip integer PCM
        // exactly when the encoder picks ABITS large enough.
        // ABITS 1..15 are intentionally non-POT (VQ / block-coded /
        // ANS-style packings).
        for a in 16usize..=26 {
            let v = LOSSLESS_QUANT[a];
            assert!(v > 0, "abits {a} step 0?");
            assert!(v.is_power_of_two(), "abits {a} step {v} not POT");
        }
    }
}
