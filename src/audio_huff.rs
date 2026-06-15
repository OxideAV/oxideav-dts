//! DTS Coherent Acoustics — Annex D §D.5 audio-data quantization-index
//! Huffman code books (the low-`ABITS` families), feeding the §5.5
//! Table 5-29 `nQType == 1` ("Huffman code") `AUDIO[m]` extraction
//! path.
//!
//! Source: ETSI TS 102 114 V1.3.1 (2011-08), staged PDF at
//! `docs/audio/dts/etsi-ts-102114-dts-coherent-acoustics.pdf`.
//!
//! # What this module covers
//!
//! The §5.5 `Audio Data` walker dispatches each subband on its
//! `nQType` (see [`crate::audio_quant_type`]). For `nQType == 1` the
//! eight `AUDIO[m]` quantization indices are Huffman-coded, decoded
//! through the §D.5 code book that the `(ABITS, SEL)` pair selects
//! (Table 5-26, staged PDF p.27). This module transcribes the
//! **signed-level** audio-data code books for the four lowest `ABITS`
//! families — the ones whose mid-tread quantizer has a small enough
//! level count to print a full code book:
//!
//! | ABITS | levels | clause  | SEL → book                |
//! |-------|--------|---------|---------------------------|
//! | 1     | 3      | §D.5.1  | SEL 0 → `A3`              |
//! | 2     | 5      | §D.5.3  | SEL 0/1/2 → `A5/B5/C5`   |
//! | 3     | 7      | §D.5.4  | SEL 0/1/2 → `A7/B7/C7`   |
//! | 4     | 9      | §D.5.5  | SEL 0/1/2 → `A9/B9/C9`   |
//!
//! (The terminal `SEL` entry of each group is the `V…` block code —
//! `nQType == 3`, handled by [`crate::decode_block_code`] — not a
//! Huffman book, so it is *not* in this module.)
//!
//! Unlike the §5.4.1 side-information code books (BHUFF/THUFF/SHUFF),
//! whose symbols are *unsigned* indices, the audio-data books decode
//! to **signed** quantization levels: the printed "Quantization level"
//! column runs `0, 1, -1, 2, -2, …`, mirroring the mid-tread
//! quantizer's symmetric output. The decoded level is the signed
//! integer `AUDIO[m]` that §5.5 then scales by `rScale`
//! ([`crate::dequant_subsubframe`]).
//!
//! # Scope and follow-ups
//!
//! The higher-`ABITS` audio-data families (§D.5.7 13-level, §D.5.8
//! 17-level, §D.5.9 25-level, §D.5.10 33-level, §D.5.11 65-level,
//! §D.5.12 129-level) are larger transcriptions and remain follow-ups.
//! Wiring this decoder into the §5.5 per-subsubframe `Audio Data`
//! walker is likewise a separate increment; this module exposes the
//! tables and the single-symbol decode so the walker can dispatch into
//! it once the higher families land.

use crate::bitreader::BitReader;
use crate::{Error, Result};

/// One entry of a §D.5 audio-data Huffman code book:
/// `(quantization_level, code_length, code)`. The codeword is the low
/// `code_length` bits of `code`, read MSB-first from the bit stream
/// (the [`BitReader::read_bits`] convention used throughout this
/// crate). `quantization_level` is the **signed** mid-tread output
/// level the §5.5 `AUDIO[m]` index carries.
type AudioHuffEntry = (i16, u8, u16);

/// Maximum code length across every §D.5.1/§D.5.3/§D.5.4/§D.5.5 book
/// transcribed here (the longest printed code is 6 bits, in the
/// 9-level §D.5.5 tables). The decoder reads bits one at a time up to
/// this bound; an unmatched pattern after that many bits is a
/// stream-format failure.
const MAX_AUDIO_HUFF_CODE_LEN: u32 = 6;

// ---------------------------------------------------------------
// §D.5.1 — 3 Levels (ABITS 1, SEL 0). Staged PDF p.198.
// ---------------------------------------------------------------

/// Annex D §D.5.1 Table A3.
const TABLE_A3: &[AudioHuffEntry] = &[
    (0, 1, 0),  //  0
    (1, 2, 2),  // +1
    (-1, 2, 3), // -1
];

// ---------------------------------------------------------------
// §D.5.3 — 5 Levels (ABITS 2, SEL 0/1/2). Staged PDF p.199.
// ---------------------------------------------------------------

/// Annex D §D.5.3 Table A5.
const TABLE_A5: &[AudioHuffEntry] = &[(0, 1, 0), (1, 2, 2), (-1, 3, 6), (2, 4, 14), (-2, 4, 15)];

/// Annex D §D.5.3 Table B5.
const TABLE_B5: &[AudioHuffEntry] = &[(0, 2, 2), (1, 2, 0), (-1, 2, 1), (2, 3, 6), (-2, 3, 7)];

/// Annex D §D.5.3 Table C5.
const TABLE_C5: &[AudioHuffEntry] = &[(0, 1, 0), (1, 3, 4), (-1, 3, 5), (2, 3, 6), (-2, 3, 7)];

// ---------------------------------------------------------------
// §D.5.4 — 7 Levels (ABITS 3, SEL 0/1/2). Staged PDF p.199-200.
// ---------------------------------------------------------------

/// Annex D §D.5.4 Table A7.
const TABLE_A7: &[AudioHuffEntry] = &[
    (0, 1, 0),
    (1, 3, 6),
    (-1, 3, 5),
    (2, 3, 4),
    (-2, 4, 14),
    (3, 5, 31),
    (-3, 5, 30),
];

/// Annex D §D.5.4 Table B7.
const TABLE_B7: &[AudioHuffEntry] = &[
    (0, 2, 3),
    (1, 2, 1),
    (-1, 2, 0),
    (2, 3, 4),
    (-2, 4, 11),
    (3, 5, 21),
    (-3, 5, 20),
];

/// Annex D §D.5.4 Table C7.
const TABLE_C7: &[AudioHuffEntry] = &[
    (0, 2, 3),
    (1, 2, 2),
    (-1, 2, 1),
    (2, 4, 3),
    (-2, 4, 2),
    (3, 4, 1),
    (-3, 4, 0),
];

// ---------------------------------------------------------------
// §D.5.5 — 9 Levels (ABITS 4, SEL 0/1/2). Staged PDF p.200-201.
// ---------------------------------------------------------------

/// Annex D §D.5.5 Table A9.
const TABLE_A9: &[AudioHuffEntry] = &[
    (0, 1, 0),
    (1, 3, 7),
    (-1, 3, 5),
    (2, 4, 13),
    (-2, 4, 9),
    (3, 4, 8),
    (-3, 5, 25),
    (4, 6, 49),
    (-4, 6, 48),
];

/// Annex D §D.5.5 Table B9.
const TABLE_B9: &[AudioHuffEntry] = &[
    (0, 2, 2),
    (1, 2, 0),
    (-1, 3, 7),
    (2, 3, 3),
    (-2, 3, 2),
    (3, 5, 27),
    (-3, 5, 26),
    (4, 5, 25),
    (-4, 5, 24),
];

/// Annex D §D.5.5 Table C9.
const TABLE_C9: &[AudioHuffEntry] = &[
    (0, 2, 2),
    (1, 2, 0),
    (-1, 3, 7),
    (2, 3, 6),
    (-2, 3, 2),
    (3, 4, 6),
    (-3, 5, 15),
    (4, 6, 29),
    (-4, 6, 28),
];

/// One of the §D.5.1/§D.5.3/§D.5.4/§D.5.5 audio-data quantization-index
/// Huffman code books, selected by the §5.5 `(ABITS, SEL)` pair.
///
/// Each variant names its §D.5 table and the `ABITS` family
/// (= mid-tread level count) it belongs to. Resolve from a
/// `(ABITS, SEL)` pair with [`AudioHuffCodebook::from_abits_sel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioHuffCodebook {
    /// §D.5.1 Table A3 — ABITS 1, SEL 0 (3 levels).
    A3,
    /// §D.5.3 Table A5 — ABITS 2, SEL 0 (5 levels).
    A5,
    /// §D.5.3 Table B5 — ABITS 2, SEL 1 (5 levels).
    B5,
    /// §D.5.3 Table C5 — ABITS 2, SEL 2 (5 levels).
    C5,
    /// §D.5.4 Table A7 — ABITS 3, SEL 0 (7 levels).
    A7,
    /// §D.5.4 Table B7 — ABITS 3, SEL 1 (7 levels).
    B7,
    /// §D.5.4 Table C7 — ABITS 3, SEL 2 (7 levels).
    C7,
    /// §D.5.5 Table A9 — ABITS 4, SEL 0 (9 levels).
    A9,
    /// §D.5.5 Table B9 — ABITS 4, SEL 1 (9 levels).
    B9,
    /// §D.5.5 Table C9 — ABITS 4, SEL 2 (9 levels).
    C9,
}

impl AudioHuffCodebook {
    /// Resolve the audio-data Huffman code book for one subband from
    /// its `(ABITS, SEL)` pair, per the Table 5-26 `SEL`-column order
    /// (staged PDF p.27):
    ///
    /// * ABITS 1 group `A3 V3` → SEL 0 = `A3`;
    /// * ABITS 2 group `A5 B5 C5 V5` → SEL 0/1/2 = `A5/B5/C5`;
    /// * ABITS 3 group `A7 B7 C7 V7` → SEL 0/1/2 = `A7/B7/C7`;
    /// * ABITS 4 group `A9 B9 C9 V9` → SEL 0/1/2 = `A9/B9/C9`.
    ///
    /// Returns `None` when the `(ABITS, SEL)` pair does not select a
    /// Huffman book in this module: an `ABITS` outside `1..=4`, or a
    /// `SEL` at (or past) the group's terminal `V…` block-code entry
    /// (the [`crate::AudioQuantType::BlockCode`] path, not Huffman).
    /// Use [`crate::audio_quant_type`] first to confirm the subband is
    /// `nQType == 1` before calling this.
    #[must_use]
    pub fn from_abits_sel(abits: u8, sel: u8) -> Option<Self> {
        match (abits, sel) {
            (1, 0) => Some(Self::A3),
            (2, 0) => Some(Self::A5),
            (2, 1) => Some(Self::B5),
            (2, 2) => Some(Self::C5),
            (3, 0) => Some(Self::A7),
            (3, 1) => Some(Self::B7),
            (3, 2) => Some(Self::C7),
            (4, 0) => Some(Self::A9),
            (4, 1) => Some(Self::B9),
            (4, 2) => Some(Self::C9),
            _ => None,
        }
    }

    /// The `ABITS` family (= mid-tread quantizer level count) this book
    /// belongs to: 1 (3 levels), 2 (5 levels), 3 (7 levels), or 4
    /// (9 levels).
    #[must_use]
    pub fn abits(self) -> u8 {
        match self {
            Self::A3 => 1,
            Self::A5 | Self::B5 | Self::C5 => 2,
            Self::A7 | Self::B7 | Self::C7 => 3,
            Self::A9 | Self::B9 | Self::C9 => 4,
        }
    }

    /// The number of quantizer levels of this book's `ABITS` family
    /// (Table 5-26 "Number of Index Quantization Levels"): 3, 5, 7, or
    /// 9. Equals the number of entries in the underlying §D.5 table.
    #[must_use]
    pub fn levels(self) -> u16 {
        match self.abits() {
            1 => 3,
            2 => 5,
            3 => 7,
            _ => 9,
        }
    }

    /// The static §D.5 code-book table backing this variant and a
    /// stable name for [`Error::HuffmanDecodeFailed`].
    fn table(self) -> (&'static [AudioHuffEntry], &'static str) {
        match self {
            Self::A3 => (TABLE_A3, "A3"),
            Self::A5 => (TABLE_A5, "A5"),
            Self::B5 => (TABLE_B5, "B5"),
            Self::C5 => (TABLE_C5, "C5"),
            Self::A7 => (TABLE_A7, "A7"),
            Self::B7 => (TABLE_B7, "B7"),
            Self::C7 => (TABLE_C7, "C7"),
            Self::A9 => (TABLE_A9, "A9"),
            Self::B9 => (TABLE_B9, "B9"),
            Self::C9 => (TABLE_C9, "C9"),
        }
    }
}

/// Walk a §D.5 audio-data Huffman code book one bit at a time,
/// MSB-first, returning the matching signed quantization level when a
/// code of the prefix-matched length is found. Returns
/// [`Error::HuffmanDecodeFailed`] when no entry matches within
/// [`MAX_AUDIO_HUFF_CODE_LEN`] bits.
fn decode_audio_huff(br: &mut BitReader<'_>, codebook: AudioHuffCodebook) -> Result<i16> {
    let (table, name) = codebook.table();
    let mut value: u32 = 0;
    let mut bits_read: u8 = 0;
    while bits_read < MAX_AUDIO_HUFF_CODE_LEN as u8 {
        let bit = br.read_bits(1)?;
        value = (value << 1) | bit;
        bits_read += 1;
        for &(level, code_len, code) in table {
            if code_len == bits_read && value == code as u32 {
                return Ok(level);
            }
        }
    }
    Err(Error::HuffmanDecodeFailed { table: name })
}

/// Decode a single §5.5 `nQType == 1` `AUDIO[m]` quantization index
/// from `bytes` starting at `bit_offset` (MSB-first from `bytes[0]`),
/// through the §D.5 code book selected by `codebook`.
///
/// Returns `(quantization_level, bits_consumed)` where
/// `quantization_level` is the **signed** mid-tread output level §5.5
/// scales by `rScale`, and `bits_consumed` is the codeword length.
///
/// # Errors
///
/// * [`Error::UnexpectedEof`] when the buffer ends mid-codeword;
/// * [`Error::HuffmanDecodeFailed`] when no §D.5 entry matches.
pub fn decode_audio_huff_at(
    bytes: &[u8],
    bit_offset: usize,
    codebook: AudioHuffCodebook,
) -> Result<(i16, usize)> {
    let byte_offset = bit_offset / 8;
    let intra_byte = bit_offset % 8;
    let mut br = BitReader::from_byte_offset(bytes, byte_offset);
    if intra_byte > 0 {
        br.read_bits(intra_byte as u32)?;
    }
    let level = decode_audio_huff(&mut br, codebook)?;
    let bits_consumed = br.absolute_bit_position() - bit_offset;
    Ok((level, bits_consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pack a series of (value, bit_width) fields MSB-first.
    fn pack_fields(fields: &[(u32, u8)]) -> Vec<u8> {
        let total_bits: usize = fields.iter().map(|(_, w)| *w as usize).sum();
        let mut out = vec![0u8; total_bits.div_ceil(8)];
        let mut bit_pos = 0usize;
        for &(value, width) in fields {
            for i in (0..width).rev() {
                let bit = ((value >> i) & 1) as u8;
                out[bit_pos / 8] |= bit << (7 - (bit_pos % 8));
                bit_pos += 1;
            }
        }
        out
    }

    const ALL_BOOKS: &[AudioHuffCodebook] = &[
        AudioHuffCodebook::A3,
        AudioHuffCodebook::A5,
        AudioHuffCodebook::B5,
        AudioHuffCodebook::C5,
        AudioHuffCodebook::A7,
        AudioHuffCodebook::B7,
        AudioHuffCodebook::C7,
        AudioHuffCodebook::A9,
        AudioHuffCodebook::B9,
        AudioHuffCodebook::C9,
    ];

    #[test]
    fn from_abits_sel_resolves_table_5_26_groups() {
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(1, 0),
            Some(AudioHuffCodebook::A3)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(2, 0),
            Some(AudioHuffCodebook::A5)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(2, 1),
            Some(AudioHuffCodebook::B5)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(2, 2),
            Some(AudioHuffCodebook::C5)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(3, 0),
            Some(AudioHuffCodebook::A7)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(3, 1),
            Some(AudioHuffCodebook::B7)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(3, 2),
            Some(AudioHuffCodebook::C7)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(4, 0),
            Some(AudioHuffCodebook::A9)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(4, 1),
            Some(AudioHuffCodebook::B9)
        );
        assert_eq!(
            AudioHuffCodebook::from_abits_sel(4, 2),
            Some(AudioHuffCodebook::C9)
        );
    }

    #[test]
    fn from_abits_sel_none_for_terminal_or_out_of_family() {
        // Terminal SEL of each group is the V… block code, not Huffman.
        assert_eq!(AudioHuffCodebook::from_abits_sel(1, 1), None); // V3
        assert_eq!(AudioHuffCodebook::from_abits_sel(2, 3), None); // V5
        assert_eq!(AudioHuffCodebook::from_abits_sel(3, 3), None); // V7
        assert_eq!(AudioHuffCodebook::from_abits_sel(4, 3), None); // V9
                                                                   // No bits allocated / outside the transcribed families.
        assert_eq!(AudioHuffCodebook::from_abits_sel(0, 0), None);
        assert_eq!(AudioHuffCodebook::from_abits_sel(5, 0), None);
        assert_eq!(AudioHuffCodebook::from_abits_sel(7, 0), None);
    }

    #[test]
    fn abits_and_levels_match_family() {
        assert_eq!(AudioHuffCodebook::A3.abits(), 1);
        assert_eq!(AudioHuffCodebook::A3.levels(), 3);
        assert_eq!(AudioHuffCodebook::C5.abits(), 2);
        assert_eq!(AudioHuffCodebook::C5.levels(), 5);
        assert_eq!(AudioHuffCodebook::B7.abits(), 3);
        assert_eq!(AudioHuffCodebook::B7.levels(), 7);
        assert_eq!(AudioHuffCodebook::C9.abits(), 4);
        assert_eq!(AudioHuffCodebook::C9.levels(), 9);
    }

    #[test]
    fn level_count_equals_table_length() {
        for &book in ALL_BOOKS {
            let (table, _) = book.table();
            assert_eq!(
                table.len() as u16,
                book.levels(),
                "{book:?} table length must equal its level count"
            );
        }
    }

    #[test]
    fn every_book_is_a_complete_prefix_code() {
        // A valid Huffman book: no code is a prefix of another, and the
        // Kraft sum over all leaves equals 1 (a full mid-tread set).
        for &book in ALL_BOOKS {
            let (table, name) = book.table();
            // Prefix-freeness: for any two entries, the shorter is not a
            // prefix of the longer.
            for (i, &(_, len_i, code_i)) in table.iter().enumerate() {
                for (j, &(_, len_j, code_j)) in table.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    if len_i <= len_j {
                        let shift = len_j - len_i;
                        assert_ne!(
                            (code_j >> shift),
                            code_i,
                            "{name}: code {code_i:b}/{len_i} is a prefix of {code_j:b}/{len_j}"
                        );
                    }
                }
            }
            // Kraft equality (complete code).
            let kraft: f64 = table
                .iter()
                .map(|&(_, len, _)| 2f64.powi(-(len as i32)))
                .sum();
            assert!(
                (kraft - 1.0).abs() < 1e-9,
                "{name}: Kraft sum {kraft} != 1 (incomplete code)"
            );
        }
    }

    #[test]
    fn every_book_round_trips_every_symbol() {
        // Encode each printed codeword, decode it, and confirm the
        // signed level + consumed-bit count come back exactly.
        for &book in ALL_BOOKS {
            let (table, _) = book.table();
            for &(level, code_len, code) in table {
                let stream = pack_fields(&[(code as u32, code_len)]);
                let (got, bits) = decode_audio_huff_at(&stream, 0, book).unwrap();
                assert_eq!(got, level, "{book:?}: level for code {code:b}/{code_len}");
                assert_eq!(bits, code_len as usize, "{book:?}: consumed bits");
            }
        }
    }

    #[test]
    fn books_decode_signed_levels_symmetrically() {
        // Each family carries a symmetric ± level set around 0; verify
        // the printed level columns include both signs up to the family
        // amplitude.
        let max_amp = |book: AudioHuffCodebook| (book.levels() as i16 - 1) / 2;
        for &book in ALL_BOOKS {
            let (table, _) = book.table();
            let amp = max_amp(book);
            for lvl in -amp..=amp {
                assert!(
                    table.iter().any(|&(l, _, _)| l == lvl),
                    "{book:?}: missing level {lvl}"
                );
            }
        }
    }

    #[test]
    fn a3_specific_codes() {
        // §D.5.1 Table A3, transcribed verbatim (PDF p.198).
        assert_eq!(
            decode_audio_huff_at(&pack_fields(&[(0, 1)]), 0, AudioHuffCodebook::A3).unwrap(),
            (0, 1)
        );
        assert_eq!(
            decode_audio_huff_at(&pack_fields(&[(2, 2)]), 0, AudioHuffCodebook::A3).unwrap(),
            (1, 2)
        );
        assert_eq!(
            decode_audio_huff_at(&pack_fields(&[(3, 2)]), 0, AudioHuffCodebook::A3).unwrap(),
            (-1, 2)
        );
    }

    #[test]
    fn a9_longest_codes_are_six_bits() {
        // §D.5.5 Table A9 (PDF p.200): ±4 are the 6-bit codes 49/48.
        assert_eq!(
            decode_audio_huff_at(&pack_fields(&[(49, 6)]), 0, AudioHuffCodebook::A9).unwrap(),
            (4, 6)
        );
        assert_eq!(
            decode_audio_huff_at(&pack_fields(&[(48, 6)]), 0, AudioHuffCodebook::A9).unwrap(),
            (-4, 6)
        );
    }

    #[test]
    fn decode_at_unaligned_offset_matches_aligned() {
        // Prepend 3 filler bits; the decode must match the aligned read.
        let aligned = pack_fields(&[(31, 5)]); // A7 level +3
        let shifted = pack_fields(&[(0b101, 3), (31, 5)]);
        let a = decode_audio_huff_at(&aligned, 0, AudioHuffCodebook::A7).unwrap();
        let b = decode_audio_huff_at(&shifted, 3, AudioHuffCodebook::A7).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, (3, 5));
    }

    #[test]
    fn truncated_stream_surfaces_eof() {
        // An empty buffer cannot supply even the first codeword bit.
        assert_eq!(
            decode_audio_huff_at(&[], 0, AudioHuffCodebook::A9).unwrap_err(),
            Error::UnexpectedEof
        );
        // A single byte whose first bit forces the long branch but
        // whose tail runs out: `110001` (A9 level +4) needs 6 bits;
        // start 4 bits into the byte so only 4 remain. The first three
        // available bits `100` are not a complete A9 code shorter than
        // 3 bits with that prefix, so the read walks past EOF. Use a
        // byte laid out as `xxxx 1000` and a level whose code begins
        // `100…`: A9's 4-bit codes are 13/9/8 — `1000` = 8 (level -2)
        // is exactly 4 bits and resolves, so instead truncate harder:
        // start 6 bits in, leaving 2 bits, and require the 6-bit code.
        let one_byte = pack_fields(&[(0b00000011, 8)]);
        assert_eq!(one_byte.len(), 1);
        // Remaining bits from offset 6: `11`. A9 has no 1- or 2-bit code
        // matching `1`/`11`, so the third-bit read hits EOF.
        assert_eq!(
            decode_audio_huff_at(&one_byte, 6, AudioHuffCodebook::A9).unwrap_err(),
            Error::UnexpectedEof
        );
    }

    #[test]
    fn complete_codes_always_resolve_within_max_len() {
        // Every §D.5 book transcribed here is a complete prefix code
        // (Kraft sum = 1, checked above), so any bit pattern long
        // enough resolves to *some* symbol within
        // MAX_AUDIO_HUFF_CODE_LEN bits — the `HuffmanDecodeFailed` arm
        // is only reachable on a truncated read (covered by
        // `truncated_stream_surfaces_eof`). Confirm every 6-bit prefix
        // decodes for the deepest family (A9).
        for raw in 0u32..(1 << MAX_AUDIO_HUFF_CODE_LEN) {
            let stream = pack_fields(&[(raw, MAX_AUDIO_HUFF_CODE_LEN as u8)]);
            let res = decode_audio_huff_at(&stream, 0, AudioHuffCodebook::A9);
            assert!(
                res.is_ok(),
                "A9: 6-bit prefix {raw:06b} failed to resolve: {res:?}"
            );
        }
    }
}
