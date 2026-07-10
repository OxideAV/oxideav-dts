//! Round-408 integration test for the §5.7.2.2 DRC override rule:
//! "the DRC values in the Rev2AUX data chunk **should be used instead
//! of** any dynamic range control coefficients found in the legacy
//! core stream (indicated by flag DYNF)".
//!
//! Builds complete one-channel NFE Core frames (the proven
//! `dynf_range_decode.rs` layout) and appends a DWORD-aligned §5.7.2
//! Rev2 Auxiliary Data Chunk carrying per-subsubframe DRC codes with a
//! **genuine Annex B CRC** — the gate `decode_core_frame` uses to
//! accept the chunk (a CRC-invalid chunk must be ignored, both because
//! it may be a false sync alias and because the spec's aux CRCs are
//! genuinely verified, unlike the core HCRC family).
//!
//! Properties pinned:
//! * a CRC-valid Rev2AUX DRC chunk **overrides** the legacy `DYNF`
//!   `RANGE` gain (the frame decodes to baseline × Rev2 gain, not
//!   baseline × RANGE gain, and not the product of both);
//! * the override also applies when the frame has no `DYNF` at all;
//! * a CRC-**invalid** chunk changes nothing (legacy `RANGE` still
//!   applies).

use oxideav_dts::{
    decode_core_frame, dts_crc16, dts_dynrng_to_linear, parse_frame_header, REV2_AUX_SYNC_WORD,
    REV2_DRC_VERSION_SINGLE_BAND,
};

/// Pack a list of `(value, width)` fields MSB-first into bytes.
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

/// The §5.3.1 BE frame header for a one-channel raw-BE Core frame with
/// `NBLKS + 1 = 8` blocks (= 256 samples = exactly one 256-sample
/// §5.7.2 subsubframe, so the Rev2AUX chunk carries one DRC byte).
fn header_bytes(dynf: bool) -> Vec<u8> {
    let hdr_bytes: [u8; 16] = [
        0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03, 0xef,
        0x7f,
    ];
    let mut header = parse_frame_header(&hdr_bytes).unwrap();
    header.dynamic_range = dynf;
    header.aspf = false;
    header.blocks_per_frame = 7; // NBLKS: 8 blocks -> 1 subsubframe
    assert!(!header.crc_present);
    oxideav_dts::encode_frame_header_be(&header).unwrap()
}

/// §5.3.2 Audio Coding Header: one channel, nSUBS = nVQSUB = 2, linear
/// ABITS/SCALES books, SEL plane putting the ABITS-8 group at the
/// terminal NFE entry (same layout as `dynf_range_decode.rs`).
fn ach_one_channel_nfe_body() -> Vec<(u32, u8)> {
    let mut body: Vec<(u32, u8)> = vec![
        (0, 4), // SUBFS -> 1 subframe
        (0, 3), // PCHS  -> 1 channel
        (0, 5), // SUBS  -> nSUBS 2
        (1, 5), // VQSUB -> nVQSUB 2
        (0, 3), // JOINX
        (0, 2), // THUFF
        (5, 3), // SHUFF=5 -> 6-bit linear SCALES
        (6, 3), // BHUFF=6 -> 5-bit linear ABITS
    ];
    let widths = [1u8, 2, 2, 2, 2, 3, 3, 3, 3, 3];
    for (n, &w) in widths.iter().enumerate() {
        body.push((if n == 7 { 7 } else { 0 }, w));
    }
    for n in 0..10 {
        if n != 7 {
            body.push((0, 2)); // ADJ per SEL=0 slot
        }
    }
    body
}

/// §5.4.1 side info: subband 0 carries ABITS = 8 with a non-zero
/// SCALES factor; subband 1 is NoBits.
fn side_info_one_subband_abits8() -> Vec<(u32, u8)> {
    vec![
        (0, 2),  // SSC -> nSSC 1
        (0, 3),  // PSC
        (0, 1),  // PMODE[0][0]
        (0, 1),  // PMODE[0][1]
        (8, 5),  // ABITS[0][0] = 8
        (0, 5),  // ABITS[0][1] = 0
        (20, 6), // SCALES[0][0] (6-bit linear RMS index)
    ]
}

/// Build a §5.7.2 Rev2AUX chunk (declared size 8) carrying one 8-bit
/// DRC code, with a correct or deliberately corrupt Annex B CRC.
fn rev2_chunk(drc_code: u8, valid_crc: bool) -> Vec<u8> {
    let mut fields: Vec<(u32, u8)> = vec![
        (7, 7), // nRev2AUXDataByteSize - 1 -> size 8
        (0, 1), // bESMetaDataFlag = 0
        (1, 1), // bBroadcastMetadataPresent
        (1, 1), // bDRCMetadataPresent
        (0, 1), // bDialnormMetadata = 0
        (u32::from(REV2_DRC_VERSION_SINGLE_BAND), 4),
        (0, 1), // nByteAlign0 (to the byte boundary)
        (u32::from(drc_code), 8),
    ];
    // Zero-pad the reserved region so the CRC lands at byte
    // offset size - 2 = 6 from the size field (the fields above
    // occupy 3 whole bytes).
    fields.push((0, 8));
    fields.push((0, 8));
    fields.push((0, 8));
    let body = pack_fields(&fields);
    assert_eq!(body.len(), 6);

    let mut chunk = REV2_AUX_SYNC_WORD.to_be_bytes().to_vec();
    chunk.extend_from_slice(&body);
    let mut crc = dts_crc16(&body);
    if !valid_crc {
        crc ^= 0xFFFF;
    }
    chunk.extend_from_slice(&crc.to_be_bytes());
    chunk
}

/// Build a full frame: header + NFE body (+ optional `RANGE` code) +
/// DSYNC, zero-padded to a DWORD boundary, then (optionally) the
/// DWORD-aligned Rev2AUX chunk.
fn build_frame(
    dynf: bool,
    range_code: u8,
    samples: &[i32; 8],
    rev2: Option<(u8, bool)>,
) -> Vec<u8> {
    let mut bytes = header_bytes(dynf);
    let mut body = ach_one_channel_nfe_body();
    body.extend(side_info_one_subband_abits8());
    if dynf {
        body.push((u32::from(range_code), 8));
    }
    for &s in samples {
        body.push(((s as u32) & 0x1f, 5)); // ABITS-8 NFE, width 5
    }
    body.push((0xffff, 16)); // DSYNC
    bytes.extend_from_slice(&pack_fields(&body));
    bytes.extend_from_slice(&[0u8; 4]); // header-lookahead slack

    if let Some((code, valid)) = rev2 {
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(&rev2_chunk(code, valid));
    }
    bytes
}

/// Round-to-nearest, i32-saturating scale — mirrors the crate's DRC
/// application convention.
fn scale_sat(v: i32, m: f64) -> i32 {
    let s = (v as f64 * m).round();
    if s >= i32::MAX as f64 {
        i32::MAX
    } else if s <= i32::MIN as f64 {
        i32::MIN
    } else {
        s as i32
    }
}

const SAMPLES: [i32; 8] = [7, -7, 5, -5, 3, -3, 6, -6];

fn decode(bytes: &[u8]) -> Vec<Vec<i32>> {
    let header = parse_frame_header(bytes).unwrap();
    decode_core_frame(bytes, &header).expect("frame decodes")
}

/// A CRC-valid Rev2AUX DRC chunk overrides the legacy DYNF RANGE gain:
/// the decoded PCM is baseline × Rev2 gain, not baseline × RANGE gain
/// and not the product of both.
#[test]
fn crc_valid_rev2_drc_overrides_legacy_dynf() {
    let baseline = decode(&build_frame(false, 0, &SAMPLES, None));

    let legacy_code = 80u8; // +20 dB -> 10.0x
    let rev2_code = 0u8.wrapping_sub(80); // -20 dB -> 0.1x
    let rev2_gain = dts_dynrng_to_linear(rev2_code);

    let pcm = decode(&build_frame(
        true,
        legacy_code,
        &SAMPLES,
        Some((rev2_code, true)),
    ));

    assert_eq!(pcm[0].len(), baseline[0].len());
    assert_eq!(pcm[0].len(), 256); // 8 blocks = one 256-sample subsubframe
    for (i, (&b, &d)) in baseline[0].iter().zip(pcm[0].iter()).enumerate() {
        assert_eq!(
            d,
            scale_sat(b, rev2_gain),
            "sample {i}: expected baseline {b} x Rev2 gain, got {d}"
        );
    }
    // Sanity: the result genuinely differs from the legacy-gain decode.
    let legacy_only = decode(&build_frame(true, legacy_code, &SAMPLES, None));
    assert_ne!(pcm, legacy_only);
}

/// The override applies even when the frame carries no DYNF at all
/// (the Rev2AUX chunk "may be encoded in the stream even when
/// AUXF=FALSE"; its DRC replaces whatever the legacy path would do).
#[test]
fn rev2_drc_applies_without_dynf() {
    let baseline = decode(&build_frame(false, 0, &SAMPLES, None));
    let rev2_code = 80u8; // +20 dB -> 10.0x
    let gain = dts_dynrng_to_linear(rev2_code);
    let pcm = decode(&build_frame(false, 0, &SAMPLES, Some((rev2_code, true))));
    for (&b, &d) in baseline[0].iter().zip(pcm[0].iter()) {
        assert_eq!(d, scale_sat(b, gain));
    }
    assert!(pcm[0].iter().any(|&s| s != 0));
}

/// A CRC-invalid Rev2AUX chunk is ignored: the legacy DYNF RANGE gain
/// still applies, exactly as if the chunk were absent.
#[test]
fn crc_invalid_rev2_chunk_is_ignored() {
    let legacy_code = 80u8;
    let with_bad_chunk = decode(&build_frame(
        true,
        legacy_code,
        &SAMPLES,
        Some((0u8.wrapping_sub(80), false)),
    ));
    let without_chunk = decode(&build_frame(true, legacy_code, &SAMPLES, None));
    assert_eq!(with_bad_chunk, without_chunk);
    assert!(with_bad_chunk[0].iter().any(|&s| s != 0));
}
