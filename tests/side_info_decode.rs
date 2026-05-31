//! Round-195 integration tests for the §5.4.1 ABITS / SCALES
//! side-info bit-stream decoders.
//!
//! These exercise the public `decode_abits_at` / `decode_scales_at`
//! entry points (the byte-slice + bit-offset wrappers around the
//! crate-internal bit reader) so the typical caller pattern — drive a
//! contiguous side-info block one field at a time, advancing a bit
//! cursor through the returned `bits_consumed` — is validated outside
//! the in-module tests' direct `BitReader` access.

use oxideav_dts::{
    decode_abits_at, decode_scales_at, AbitsCodebook, ScalesCodebook, RMS_6BIT, RMS_7BIT,
};

/// Pack a series of (code, code_length) pairs into a byte stream
/// MSB-first. Trailing bits are zero-padded.
fn pack_codes(codes: &[(u16, u8)]) -> Vec<u8> {
    let total_bits: usize = codes.iter().map(|(_, len)| *len as usize).sum();
    let total_bytes = total_bits.div_ceil(8);
    let mut out = vec![0u8; total_bytes];
    let mut bit_pos: usize = 0;
    for &(code, len) in codes {
        for i in (0..len).rev() {
            let bit = ((code >> i) & 1) as u8;
            let byte_idx = bit_pos / 8;
            let bit_in_byte = 7 - (bit_pos % 8);
            out[byte_idx] |= bit << bit_in_byte;
            bit_pos += 1;
        }
    }
    out
}

/// Walk a contiguous block of ABITS fields encoded with the same
/// `BHUFF[ch]` codebook, advancing the bit cursor through each
/// `bits_consumed` return. This mirrors how §5.4.1 Table 5-28's
/// `for (n=0; n<nVQSUB[ch]; n++) QABITS.ppQ[nQSelect]->InverseQ(...)`
/// loop drives one channel's ABITS extraction.
#[test]
fn abits_block_decode_walks_contiguous_subband_loop_under_bhuff_a12() {
    // Hand-pack a 5-subband ABITS block using BHUFF=0 (Table A12):
    // subband-by-subband ABITS values [1, 5, 12, 1, 8]. From the
    // staged PDF Annex D §D.5.6 Table A12 the encoded (code_length,
    // code) pairs are:
    //   ABITS=1  -> (1, 0)
    //   ABITS=5  -> (5, 30)
    //   ABITS=12 -> (9, 504)
    //   ABITS=1  -> (1, 0)
    //   ABITS=8  -> (8, 254)
    // Total bits = 1 + 5 + 9 + 1 + 8 = 24 = 3 bytes exactly.
    let stream = pack_codes(&[(0, 1), (30, 5), (504, 9), (0, 1), (254, 8)]);
    assert_eq!(stream.len(), 3);

    let mut cursor_bits = 0usize;
    let mut decoded = Vec::new();
    for _ in 0..5 {
        let (abits, consumed) = decode_abits_at(&stream, cursor_bits, AbitsCodebook::A12).unwrap();
        decoded.push(abits);
        cursor_bits += consumed;
    }
    assert_eq!(decoded, vec![1, 5, 12, 1, 8]);
    // The cursor must land exactly at the bit-pack end (24 bits in,
    // so the per-call bits_consumed totals must sum to 24).
    assert_eq!(cursor_bits, 1 + 5 + 9 + 1 + 8);
}

/// Walk a contiguous block of SCALES fields encoded with SHUFF=0
/// (SA129 / Table A5), demonstrating the difference-accumulator
/// contract: the running `n_scale_sum` from each call feeds the next.
/// This is the §5.4.1 Table 5-28 inner loop:
///
/// ```text
/// nScaleSum = 0;
/// for (n=0; n<nVQSUB[ch]; n++)
///   if (ABITS[ch][n] > 0) {
///     QSCALES.ppQ[nQSelect]->InverseQ(InputFrame, nScale);
///     if (nQSelect < 5)  nScaleSum += nScale;
///     else               nScaleSum  = nScale;
///     pScaleTable->LookUp(nScaleSum, SCALES[ch][n][0]);
///   }
/// ```
#[test]
fn scales_block_decode_accumulates_through_difference_loop_under_shuff_sa129() {
    // From Annex D §D.5.3 Table A5 (the 5-level difference codebook
    // for SA129..SC129):
    //   diff=0  -> (1, 0)
    //   diff=+1 -> (2, 2)    (binary 10)
    //   diff=-1 -> (3, 6)    (binary 110)
    //   diff=+2 -> (4, 14)   (binary 1110)
    //   diff=-2 -> (4, 15)   (binary 1111)
    //
    // Encode the difference sequence (+2, +1, 0, -1, -2) starting from
    // n_scale_sum=10. After each step the absolute index becomes
    // 12, 13, 13, 12, 10. The corresponding RMS_6BIT lookups are:
    //   RMS_6BIT[12] = 26
    //   RMS_6BIT[13] = 34
    //   RMS_6BIT[13] = 34
    //   RMS_6BIT[12] = 26
    //   RMS_6BIT[10] = 16
    let stream = pack_codes(&[(14, 4), (2, 2), (0, 1), (6, 3), (15, 4)]);
    // Total bits = 4 + 2 + 1 + 3 + 4 = 14, so 2 bytes (the high 2 bits
    // of byte 1 carry actual payload; the low 2 bits are pad).
    assert_eq!(stream.len(), 2);

    let mut cursor_bits = 0usize;
    let mut n_scale_sum: i32 = 10;
    let mut decoded_scales: Vec<u32> = Vec::new();
    let mut accumulator_history: Vec<i32> = Vec::new();
    for _ in 0..5 {
        let (scale, new_sum, consumed) =
            decode_scales_at(&stream, cursor_bits, ScalesCodebook::Sa129, n_scale_sum).unwrap();
        decoded_scales.push(scale);
        accumulator_history.push(new_sum);
        n_scale_sum = new_sum;
        cursor_bits += consumed;
    }
    assert_eq!(
        decoded_scales,
        vec![
            RMS_6BIT[12],
            RMS_6BIT[13],
            RMS_6BIT[13],
            RMS_6BIT[12],
            RMS_6BIT[10],
        ]
    );
    assert_eq!(accumulator_history, vec![12, 13, 13, 12, 10]);
    assert_eq!(cursor_bits, 4 + 2 + 1 + 3 + 4);
}

/// Cross-check that the linear-7-bit SHUFF path bypasses the
/// difference-accumulator (each call overwrites the running sum)
/// and routes through the §D.1.2 7-bit RMS table instead of §D.1.1.
#[test]
fn scales_block_decode_linear_path_overwrites_accumulator() {
    // Pack three 7-bit absolute SCALES indexes: 5, 31, 100.
    // Concatenated MSB-first: 0000101 0011111 1100100 = 21 bits;
    // padded to 24 bits with three trailing zeros:
    //   bit positions 0..=23 = `0000_1010_0111_1111_0010_0000`
    //   = bytes `0x0A 0x7F 0x20`.
    let stream = pack_codes(&[(5, 7), (31, 7), (100, 7)]);
    assert_eq!(stream, [0x0A, 0x7F, 0x20]);

    let mut cursor_bits = 0usize;
    let mut n_scale_sum: i32 = 999; // seed with garbage; linear must overwrite
    let mut decoded = Vec::new();
    for _ in 0..3 {
        let (scale, new_sum, consumed) = decode_scales_at(
            &stream,
            cursor_bits,
            ScalesCodebook::Linear7Bit,
            n_scale_sum,
        )
        .unwrap();
        decoded.push((scale, new_sum));
        n_scale_sum = new_sum;
        cursor_bits += consumed;
    }
    assert_eq!(
        decoded,
        vec![(RMS_7BIT[5], 5), (RMS_7BIT[31], 31), (RMS_7BIT[100], 100),]
    );
    assert_eq!(cursor_bits, 21);
}
