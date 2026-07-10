//! §D.10 Vector-Quantization code books — everything the spec *does*
//! define about them (dimensions, index widths, entry packing, and
//! element scaling), plus a structural scanner for the §5.5 phase-1
//! high-frequency VQ indices.
//!
//! The two books' **numeric contents are a recorded gap**: ETSI
//! TS 102 114 states, once for each of §D.10.1 and §D.10.2, "Due to
//! its extensive size, this table is not included here" (PDF p.255).
//! `docs/audio/dts/dts-d10-vq-tables-GAP.md` records the full gap
//! analysis and the legitimate recovery path (an observer-derived
//! black-box trace sweeping each index in isolation — the values are
//! data, not implementation). Until such a trace is staged, decode of
//! a `nVQSUB < nSUBS` (HF-VQ) or `PMODE != 0` (ADPCM) subband
//! surfaces the typed
//! [`crate::AudioArrayError::VqCodebookUnavailable`] refusal.
//!
//! What this module ships **now** is the spec-defined shell around the
//! missing data, so a recovered book drops straight in:
//!
//! * the wire facts (index widths, book sizes, vector lengths) as
//!   constants, from the §5.4/§5.5 walkers and the §D.10 definitions;
//! * the §D.10 entry-decoding primitives —
//!   [`unpack_hfreq_vq_entry`] (16-bit entry → two 8-bit signed
//!   elements, **each ÷ 24**) and [`adpcm_vq_coeff`] (stored integer
//!   ÷ 2¹³, spec anchor: entry `9928` → `1.2119140625`);
//! * [`scan_hf_vq_indices_at`], the purely structural §5.5 phase-1
//!   walk (`nVQIndex = ExtractBits(10)` per HF subband) that captures
//!   the indices a future lookup will consume — and that an
//!   observer-derived recovery harness needs for cross-checking.
//!
//! ## The `/24` correction (round 408)
//!
//! An earlier trace revision left the §D.10.2 element scaling
//! ambiguous (`24` vs `2⁴`). The corrected
//! `docs/audio/dts/dts-lfe-interpolation-and-audio-walker.md` §2.1 and
//! `dts-d10-vq-tables-GAP.md` settle it: the divisor is the **literal
//! number 24** (verified against a 400-dpi render of the spec page:
//! the `24` sits on the text baseline at normal glyph size, unlike
//! §D.10.1's `2^13` whose exponent is a raised superscript).

use crate::bitreader::BitReader;
use crate::Result;

// ------------------------------------------------------------------
// §D.10.2 — High-Frequency Subband VQ (`HFreqVQ`)
// ------------------------------------------------------------------

/// §D.10.2 code-book size: `2^10 = 1024` vectors.
pub const HFREQ_VQ_BOOK_SIZE: usize = 1024;

/// Width of the §5.5 phase-1 `nVQIndex` bitstream field
/// (`nVQIndex = ExtractBits(10)`, Table 5-29).
pub const HFREQ_VQ_INDEX_BITS: u32 = 10;

/// Elements per §D.10.2 vector: 32 subband samples (one subband
/// analysis window).
pub const HFREQ_VQ_VECTOR_LEN: usize = 32;

/// 16-bit table entries per §D.10.2 vector: each entry packs **two**
/// vector elements, so 16 entries make one 32-element vector.
pub const HFREQ_VQ_ENTRIES_PER_VECTOR: usize = HFREQ_VQ_VECTOR_LEN / 2;

/// The §D.10.2 element divisor: each 8-bit signed integer unpacked
/// from a 16-bit entry is divided by the **literal number 24** (not
/// `2^4`) to give a vector element. See the module docs for the
/// round-408 trace correction that pinned this value.
pub const HFREQ_VQ_ELEMENT_DIVISOR: f64 = 24.0;

/// Decode one 16-bit §D.10.2 `HFreqVQ` table entry into its two
/// vector elements: split into two 8-bit signed integers, each
/// divided by [`HFREQ_VQ_ELEMENT_DIVISOR`] (= the literal 24).
///
/// Returned as `[high-byte element, low-byte element]` in that fixed
/// order. The spec defines the packing ("each table entry is 16 bits
/// = two packed vector elements") but — with the book's numeric
/// contents omitted — publishes no anchor pinning which byte is the
/// earlier vector element; a staged observer-derived book will settle
/// the intra-entry order end-to-end (see
/// `docs/audio/dts/dts-d10-vq-tables-GAP.md`, "What a usable source
/// must provide").
#[must_use]
pub fn unpack_hfreq_vq_entry(entry: u16) -> [f64; 2] {
    let hi = (entry >> 8) as u8 as i8;
    let lo = entry as u8 as i8;
    [
        f64::from(hi) / HFREQ_VQ_ELEMENT_DIVISOR,
        f64::from(lo) / HFREQ_VQ_ELEMENT_DIVISOR,
    ]
}

// ------------------------------------------------------------------
// §D.10.1 — ADPCM Coefficient VQ (`ADPCMCoeffVQ`)
// ------------------------------------------------------------------

/// §D.10.1 code-book size: `2^12 = 4096` vectors.
pub const ADPCM_VQ_BOOK_SIZE: usize = 4096;

/// Width of the §5.4 `PVQ` index bitstream field
/// (`nVQIndex = ExtractBits(12)`).
pub const ADPCM_VQ_INDEX_BITS: u32 = 12;

/// Elements per §D.10.1 vector: the 4 ADPCM subband-prediction
/// coefficients (`PVQ[ch][n]`, consumed by the §C.2.2 predictor).
pub const ADPCM_VQ_VECTOR_LEN: usize = 4;

/// The §D.10.1 stored-entry scaling divisor: the actual coefficient
/// is the stored signed integer divided by `2^13 = 8192`.
pub const ADPCM_VQ_COEFF_DIVISOR: f64 = 8192.0;

/// Scale a §D.10.1 `ADPCMCoeffVQ` stored integer entry to the actual
/// prediction coefficient: `entry / 2^13`.
///
/// The spec's single published anchor: entry `9928` →
/// `9928 / 2^13 = 1.2119140625` (§D.10.1, PDF p.255).
#[must_use]
pub fn adpcm_vq_coeff(entry: i32) -> f64 {
    f64::from(entry) / ADPCM_VQ_COEFF_DIVISOR
}

// ------------------------------------------------------------------
// §5.5 phase 1 — structural HF-VQ index scan
// ------------------------------------------------------------------

/// Walk the §5.5 Table 5-29 phase-1 high-frequency VQ region
/// structurally, capturing the 10-bit `nVQIndex` of every HF-VQ
/// subband without attempting the (gap-blocked) `HFreqVQ.LookUp`.
///
/// Per the corrected walker trace
/// (`docs/audio/dts/dts-lfe-interpolation-and-audio-walker.md` §2.1):
///
/// ```text
/// for (ch = 0; ch < nPCHS; ch++)
///     for (n = nVQSUB[ch]; n < nSUBS[ch]; n++)
///         nVQIndex = ExtractBits(10);   // then HFreqVQ.LookUp(...)
/// ```
///
/// * `bytes` / `bit_offset` — positioned at the first §5.5 bit (the
///   phase-1 region precedes the LFE phase and the audio-data
///   arrays).
/// * `n_vqsub` / `n_subs` — the per-channel loop bounds
///   ([`crate::AudioCodingHeader::n_vqsub`] / `n_subs`); slices of
///   equal length, one entry per primary channel.
///
/// Returns `(indices, bits_consumed)` where `indices[ch]` holds the
/// captured 10-bit indices for channel `ch`'s subbands
/// `nVQSUB[ch]..nSUBS[ch]` in walk order (empty when the channel has
/// no HF-VQ subbands — the common Core case where
/// `nVQSUB == nSUBS`). `bits_consumed` is exactly
/// `10 · Σ (nSUBS[ch] − nVQSUB[ch])`, letting a caller advance its
/// cursor to the §5.5 LFE phase.
///
/// The captured indices become decodable subband samples once an
/// observer-derived §D.10.2 book is staged; until then they serve
/// stream inspection and the recovery harness itself.
///
/// # Errors
///
/// [`crate::Error::UnexpectedEof`] on a truncated region.
pub fn scan_hf_vq_indices_at(
    bytes: &[u8],
    bit_offset: usize,
    n_vqsub: &[usize],
    n_subs: &[usize],
) -> Result<(Vec<Vec<u16>>, usize)> {
    debug_assert_eq!(n_vqsub.len(), n_subs.len());
    let byte_offset = bit_offset / 8;
    let intra_byte = bit_offset % 8;
    let mut br = BitReader::from_byte_offset(bytes, byte_offset);
    if intra_byte > 0 {
        br.read_bits(intra_byte as u32)?;
    }

    let mut indices = Vec::with_capacity(n_vqsub.len());
    for (&vqsub, &subs) in n_vqsub.iter().zip(n_subs) {
        let mut ch_indices = Vec::new();
        for _ in vqsub..subs {
            ch_indices.push(br.read_bits(HFREQ_VQ_INDEX_BITS)? as u16);
        }
        indices.push(ch_indices);
    }

    let bits_consumed = br.absolute_bit_position() - bit_offset;
    Ok((indices, bits_consumed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

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

    /// The §D.10.1 anchor printed in the spec: stored entry 9928 →
    /// coefficient 1.2119140625 (= 9928 / 2^13).
    #[test]
    fn adpcm_anchor_entry_9928() {
        assert_eq!(adpcm_vq_coeff(9928), 1.2119140625);
        assert_eq!(adpcm_vq_coeff(0), 0.0);
        assert_eq!(adpcm_vq_coeff(-8192), -1.0);
    }

    /// §D.10.2 entry unpacking: two 8-bit signed halves, each divided
    /// by the literal 24.
    #[test]
    fn hfreq_entry_unpacks_two_signed_bytes_over_24() {
        // hi = 0x18 = +24 -> 1.0; lo = 0xE8 = -24 -> -1.0.
        assert_eq!(unpack_hfreq_vq_entry(0x18E8), [1.0, -1.0]);
        // Zero entry -> two zero elements.
        assert_eq!(unpack_hfreq_vq_entry(0), [0.0, 0.0]);
        // hi = 0x7F = +127 -> 127/24; lo = 0x80 = -128 -> -128/24.
        let [a, b] = unpack_hfreq_vq_entry(0x7F80);
        assert!((a - 127.0 / 24.0).abs() < 1e-15);
        assert!((b - (-128.0 / 24.0)).abs() < 1e-15);
    }

    /// The book/vector dimensional facts hold together: 16 two-element
    /// entries per 32-element vector; 10 bits address 1024 vectors;
    /// 12 bits address 4096.
    #[test]
    fn dimensional_facts_consistent() {
        assert_eq!(HFREQ_VQ_ENTRIES_PER_VECTOR * 2, HFREQ_VQ_VECTOR_LEN);
        assert_eq!(1usize << HFREQ_VQ_INDEX_BITS, HFREQ_VQ_BOOK_SIZE);
        assert_eq!(1usize << ADPCM_VQ_INDEX_BITS, ADPCM_VQ_BOOK_SIZE);
    }

    /// The structural scan reads exactly 10 bits per HF-VQ subband in
    /// (ch, n) walk order and reports the consumed bit count.
    #[test]
    fn scan_captures_indices_in_walk_order() {
        // ch0: nVQSUB=2, nSUBS=4 -> 2 indices; ch1: 3..3 -> none;
        // ch2: 0..2 -> 2 indices.
        let vals = [0x3FFu32, 0x001, 0x155, 0x2AA];
        let fields: Vec<(u32, u8)> = vals.iter().map(|&v| (v, 10u8)).collect();
        let stream = pack_fields(&fields);
        let (idx, bits) = scan_hf_vq_indices_at(&stream, 0, &[2, 3, 0], &[4, 3, 2]).unwrap();
        assert_eq!(bits, 40);
        assert_eq!(idx, vec![vec![0x3FF, 0x001], vec![], vec![0x155, 0x2AA]]);
    }

    /// A non-byte-aligned start cursor is honoured (the §5.5 region
    /// rarely begins on a byte boundary).
    #[test]
    fn scan_honours_bit_offset() {
        let fields = [(0b101u32, 3u8), (0x2AB, 10)];
        let stream = pack_fields(&fields);
        let (idx, bits) = scan_hf_vq_indices_at(&stream, 3, &[1], &[2]).unwrap();
        assert_eq!(bits, 10);
        assert_eq!(idx, vec![vec![0x2AB]]);
    }

    /// The common Core case (`nVQSUB == nSUBS` everywhere) consumes
    /// zero bits.
    #[test]
    fn scan_empty_when_no_hf_vq_subbands() {
        let (idx, bits) = scan_hf_vq_indices_at(&[0u8; 4], 0, &[2, 4], &[2, 4]).unwrap();
        assert_eq!(bits, 0);
        assert_eq!(idx, vec![Vec::<u16>::new(), Vec::new()]);
    }

    /// A truncated region reports EOF rather than fabricating indices.
    #[test]
    fn scan_reports_eof_on_truncation() {
        let stream = [0u8; 1]; // 8 bits; a single index needs 10.
        assert_eq!(
            scan_hf_vq_indices_at(&stream, 0, &[0], &[1]).unwrap_err(),
            Error::UnexpectedEof
        );
    }
}
