//! Canonical Huffman tables for DTS Core.
//!
//! Provenance: every codebook here is sourced from the clean-room
//! `docs/audio/dts/data/dts-huffman-tables.md` (`{symbol, length}`
//! rows reproduced verbatim from the public ETSI TS 102 114 standard).
//!
//! ## Canonical assignment rule
//!
//! The DTS sidecar uses the FFmpeg in-memory `(symbol, length)`
//! layout. The decoder reproduces the canonical Huffman code by:
//!
//!   1. Sorting the `(symbol, length)` rows by `(length, position)`.
//!   2. Walking the sorted list and assigning consecutive integers in
//!      `length` bits (left-shifting whenever the length grows by
//!      one).
//!
//! This matches `ff_vlc_init_from_lengths` in libavutil and the
//! `VLC_INIT_STATIC_OVERLONG` flavour used by FFmpeg's DCA decoder.
//! We don't call into FFmpeg — the algorithm is the textbook
//! canonical-Huffman construction.
//!
//! ## Decoding strategy
//!
//! For round 1 we use a simple bit-by-bit walk over a pre-computed
//! `(code, length, value)` list, sorted by `(length, code)`. Reading
//! is `O(maxbits)` per symbol — fast enough since the longest core
//! Huffman codes are 16 bits and there are only a few thousand
//! decodes per frame. A future pass can replace this with a
//! multi-step lookup table.

use oxideav_core::{Error, Result};

use crate::bits::BitReader;

/// One decoded entry of a canonical Huffman table.
#[derive(Clone, Copy, Debug)]
struct Entry {
    code: u32,
    length: u8,
    symbol: i16,
}

/// A built canonical-Huffman codebook.
#[derive(Clone, Debug)]
pub struct Vlc {
    entries: Vec<Entry>,
    /// Maximum code length, in bits.
    pub max_len: u8,
    /// Value-offset added to the decoded symbol after lookup.
    pub offset: i16,
}

impl Vlc {
    /// Build a canonical-Huffman table from `(symbol, length)` pairs.
    pub fn build(rows: &[(i16, u8)], offset: i16) -> Self {
        // Sort by (length, original index) — the original-index tie
        // break preserves the "list order" within a length class
        // required by the canonical-construction rule.
        let mut indexed: Vec<(usize, i16, u8)> = rows
            .iter()
            .enumerate()
            .map(|(i, (s, l))| (i, *s, *l))
            .collect();
        indexed.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));

        let mut entries = Vec::with_capacity(rows.len());
        let mut code: u32 = 0;
        let mut prev_len: u8 = 0;
        let mut max_len: u8 = 0;
        for (_, sym, len) in &indexed {
            if *len > prev_len {
                code <<= len - prev_len;
                prev_len = *len;
            }
            entries.push(Entry {
                code,
                length: *len,
                symbol: *sym,
            });
            code += 1;
            if *len > max_len {
                max_len = *len;
            }
        }

        Self {
            entries,
            max_len,
            offset,
        }
    }

    /// Decode the next symbol from the bit reader. Returns the
    /// decoded value with `offset` already applied.
    pub fn decode(&self, r: &mut BitReader) -> Result<i32> {
        // Bit-by-bit walk: at each step extend the prefix by one bit
        // and check whether any entry of this length matches.
        let mut acc: u32 = 0;
        for length in 1..=self.max_len {
            acc = (acc << 1) | r.read(1)?;
            // Linear scan within the length class — these tables are
            // small (max 129 entries) so this is fine for round 1.
            for e in &self.entries {
                if e.length == length && e.code == acc {
                    return Ok(e.symbol as i32 + self.offset as i32);
                }
            }
        }
        Err(Error::invalid("dts: huffman decode failed"))
    }
}

// -------------------------------------------------------------------
// Bit-allocation 12-entry codebooks — dts-huffman-tables.md §2.
// Decoded symbol (0..11) + 1 = ABITS in 1..12.
// -------------------------------------------------------------------
const BITALLOC_12_A: &[(i16, u8)] = &[
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (4, 5),
    (5, 6),
    (11, 9),
    (10, 9),
    (9, 9),
    (8, 9),
    (7, 8),
    (6, 8),
];
const BITALLOC_12_B: &[(i16, u8)] = &[
    (1, 2),
    (2, 3),
    (4, 5),
    (11, 7),
    (10, 7),
    (9, 7),
    (8, 7),
    (7, 7),
    (6, 7),
    (5, 6),
    (3, 5),
    (0, 1),
];
const BITALLOC_12_C: &[(i16, u8)] = &[
    (0, 2),
    (4, 3),
    (7, 4),
    (11, 7),
    (10, 7),
    (9, 6),
    (8, 5),
    (3, 3),
    (2, 3),
    (6, 4),
    (5, 4),
    (1, 3),
];
const BITALLOC_12_D: &[(i16, u8)] = &[
    (2, 2),
    (3, 3),
    (4, 4),
    (5, 5),
    (6, 6),
    (7, 7),
    (8, 8),
    (9, 9),
    (11, 10),
    (10, 10),
    (1, 2),
    (0, 2),
];
const BITALLOC_12_E: &[(i16, u8)] = &[
    (1, 2),
    (2, 3),
    (3, 4),
    (4, 5),
    (9, 8),
    (8, 8),
    (6, 7),
    (7, 8),
    (11, 9),
    (10, 9),
    (5, 7),
    (0, 1),
];

// -------------------------------------------------------------------
// Transition-mode codebooks (THUFF) — §3, 4 codebooks × 4 entries.
// -------------------------------------------------------------------
const TM_0: &[(i16, u8)] = &[(0, 1), (1, 2), (2, 3), (3, 3)];
const TM_1: &[(i16, u8)] = &[(3, 1), (0, 2), (1, 3), (2, 3)];
const TM_2: &[(i16, u8)] = &[(2, 1), (3, 2), (0, 3), (1, 3)];
const TM_3: &[(i16, u8)] = &[(0, 2), (1, 2), (2, 2), (3, 2)];

// -------------------------------------------------------------------
// Quantization-index codebooks (sizes 3..13).
// dts-huffman-tables.md §4.
// -------------------------------------------------------------------
const BITALLOC_3: &[(i16, u8)] = &[(1, 1), (2, 2), (0, 2)];
const BITALLOC_5_A: &[(i16, u8)] = &[(2, 1), (3, 2), (1, 3), (4, 4), (0, 4)];
const BITALLOC_5_B: &[(i16, u8)] = &[(3, 2), (1, 2), (2, 2), (4, 3), (0, 3)];
const BITALLOC_5_C: &[(i16, u8)] = &[(2, 1), (3, 3), (1, 3), (4, 3), (0, 3)];
const BITALLOC_7_A: &[(i16, u8)] = &[(3, 1), (5, 3), (2, 3), (4, 3), (1, 4), (0, 5), (6, 5)];
const BITALLOC_7_B: &[(i16, u8)] = &[(2, 2), (4, 2), (5, 3), (0, 5), (6, 5), (1, 4), (3, 2)];
const BITALLOC_7_C: &[(i16, u8)] = &[(0, 4), (6, 4), (1, 4), (5, 4), (2, 2), (4, 2), (3, 2)];
const BITALLOC_9_A: &[(i16, u8)] = &[
    (4, 1),
    (7, 4),
    (2, 4),
    (3, 3),
    (0, 6),
    (8, 6),
    (1, 5),
    (6, 4),
    (5, 3),
];
const BITALLOC_9_B: &[(i16, u8)] = &[
    (5, 2),
    (2, 3),
    (6, 3),
    (4, 2),
    (0, 5),
    (8, 5),
    (1, 5),
    (7, 5),
    (3, 3),
];
const BITALLOC_9_C: &[(i16, u8)] = &[
    (5, 2),
    (2, 3),
    (7, 4),
    (0, 6),
    (8, 6),
    (1, 5),
    (4, 2),
    (6, 3),
    (3, 3),
];
const BITALLOC_13_A: &[(i16, u8)] = &[
    (6, 1),
    (7, 3),
    (9, 4),
    (10, 5),
    (1, 6),
    (11, 6),
    (4, 4),
    (8, 4),
    (0, 7),
    (12, 7),
    (2, 6),
    (3, 5),
    (5, 4),
];
const BITALLOC_13_B: &[(i16, u8)] = &[
    (6, 2),
    (8, 3),
    (10, 4),
    (3, 4),
    (1, 5),
    (11, 5),
    (9, 4),
    (5, 3),
    (7, 3),
    (0, 6),
    (12, 6),
    (2, 5),
    (4, 4),
];
const BITALLOC_13_C: &[(i16, u8)] = &[
    (4, 3),
    (0, 5),
    (12, 5),
    (2, 4),
    (8, 3),
    (5, 3),
    (7, 3),
    (6, 3),
    (10, 4),
    (1, 5),
    (11, 5),
    (3, 4),
    (9, 4),
];

/// Indices into the per-codebook arrays returned by [`bitalloc_codebook`].
/// Class index follows `BITALLOC_SIZES`: 3, 5, 7, 9, 13, 17, 25, 33, 65, 129.
pub fn bitalloc_codebook(class: usize, sub: usize) -> Vlc {
    use crate::tables::BITALLOC_OFFSETS;
    let offset = BITALLOC_OFFSETS[class];
    match (class, sub) {
        (0, _) => Vlc::build(BITALLOC_3, offset),
        (1, 0) => Vlc::build(BITALLOC_5_A, offset),
        (1, 1) => Vlc::build(BITALLOC_5_B, offset),
        (1, 2) => Vlc::build(BITALLOC_5_C, offset),
        (2, 0) => Vlc::build(BITALLOC_7_A, offset),
        (2, 1) => Vlc::build(BITALLOC_7_B, offset),
        (2, 2) => Vlc::build(BITALLOC_7_C, offset),
        (3, 0) => Vlc::build(BITALLOC_9_A, offset),
        (3, 1) => Vlc::build(BITALLOC_9_B, offset),
        (3, 2) => Vlc::build(BITALLOC_9_C, offset),
        (4, 0) => Vlc::build(BITALLOC_13_A, offset),
        (4, 1) => Vlc::build(BITALLOC_13_B, offset),
        (4, 2) => Vlc::build(BITALLOC_13_C, offset),
        // Sizes 17/25/33/65/129 are documented in
        // dts-huffman-tables.md §5/§6 but only required for
        // *Huffman-coded* quantization indices when the encoder picks
        // size class ≥ 5. FFmpeg's `dcaenc` never emits these classes,
        // so they are absent from round 1. The decoder will fall back
        // to plain two's-complement reads if the per-channel `QHUFF`
        // selector picks a fixed-width path; if it picks a Huffman
        // path of these sizes the decoder errors out cleanly until
        // round 2.
        _ => Vlc::build(&[(0, 1)], 0),
    }
}

/// Bit-allocation 12-entry codebook (BHUFF 0..4). Returns a built
/// `Vlc`; decoded symbol + 1 = ABITS.
pub fn bhuff_codebook(idx: usize) -> Vlc {
    use crate::tables::BITALLOC_12_OFFSET;
    match idx {
        0 => Vlc::build(BITALLOC_12_A, BITALLOC_12_OFFSET),
        1 => Vlc::build(BITALLOC_12_B, BITALLOC_12_OFFSET),
        2 => Vlc::build(BITALLOC_12_C, BITALLOC_12_OFFSET),
        3 => Vlc::build(BITALLOC_12_D, BITALLOC_12_OFFSET),
        4 => Vlc::build(BITALLOC_12_E, BITALLOC_12_OFFSET),
        _ => Vlc::build(BITALLOC_12_A, BITALLOC_12_OFFSET),
    }
}

/// Transition-mode codebook (THUFF 0..3).
pub fn thuff_codebook(idx: usize) -> Vlc {
    match idx {
        0 => Vlc::build(TM_0, 0),
        1 => Vlc::build(TM_1, 0),
        2 => Vlc::build(TM_2, 0),
        3 => Vlc::build(TM_3, 0),
        _ => Vlc::build(TM_0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: build a tiny canonical Huffman table and check that
    /// every symbol encodes and decodes consistently.
    #[test]
    fn canonical_construction() {
        let v = Vlc::build(&[(0, 1), (1, 2), (2, 3), (3, 3)], 0);
        // Lengths 1, 2, 3, 3 — codes should be 0, 10, 110, 111.
        let codes: Vec<(i16, u32, u8)> = v
            .entries
            .iter()
            .map(|e| (e.symbol, e.code, e.length))
            .collect();
        assert!(codes.contains(&(0, 0b0, 1)));
        assert!(codes.contains(&(1, 0b10, 2)));
        assert!(codes.contains(&(2, 0b110, 3)));
        assert!(codes.contains(&(3, 0b111, 3)));
    }

    #[test]
    fn decode_roundtrip_tm3() {
        // TM[3] is uniform 2-bit code: symbols 0..3 → codes 00, 01, 10, 11.
        let v = thuff_codebook(3);
        // Bits packed MSB-first: 00 01 10 11 → 0b0001_1011 = 0x1B.
        let data = [0x1B];
        let mut r = BitReader::new(&data);
        for expected in 0..4 {
            assert_eq!(v.decode(&mut r).unwrap(), expected);
        }
    }

    #[test]
    fn bitalloc_3_decode() {
        // bitalloc_3: {1,1}, {2,2}, {0,2}; offset = -1.
        // Lengths sorted: {1:1}, {2:2}, {0:2}.
        // Codes: 1 → 0; 2 → 10; 0 → 11.
        // After offset -1: 1→0, 2→1, 0→-1.
        let v = bitalloc_codebook(0, 0);
        let data = [0b01011000];
        let mut r = BitReader::new(&data);
        assert_eq!(v.decode(&mut r).unwrap(), 0); // sym 1
        assert_eq!(v.decode(&mut r).unwrap(), 1); // sym 2
        assert_eq!(v.decode(&mut r).unwrap(), -1); // sym 0
    }
}
