//! Round-350 integration test for the §5.4.1 Table 5-28 `DYNF`
//! (dynamic-range `RANGE`) / `CPF` (`SICRC`) side-info tail, exercised
//! end to end through the public [`oxideav_dts::decode_core_frame`]
//! Core-frame orchestrator.
//!
//! These build complete one-channel Core frames from raw bytes — the
//! §5.3.1 frame header (via the public BE encoder), the §5.3.2 Audio
//! Coding Header (Table 5-21), the §5.4.1 side info (Table 5-28) with
//! its `RANGE`/`SICRC` tail, and the §5.5 Audio Data arrays — so the
//! whole `decode_core_frame` path that the §D.4 `RANGE` multiply and
//! the `SICRC` skip live in is validated outside the in-module tests.
//!
//! The core property: a `DYNF != 0` frame whose audio-data and
//! side-info bytes are identical to a baseline `DYNF == 0` frame decodes
//! to the baseline PCM scaled, sample for sample, by the §D.4
//! [`oxideav_dts::drc_range`] multiplier (round-to-nearest, `i32`
//! saturated). This proves both the tail framing (the cursor lands on
//! the §5.5 region after the 8-bit `RANGE` index) and the post-QMF
//! multiply.

use oxideav_dts::{decode_core_frame, drc_range, parse_frame_header};

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

/// The §5.3.1 BE frame header bytes for a clean one-channel raw-BE Core
/// frame (PCMR index 0 -> 16-bit -> rScale 32768, FILTS=0), optionally
/// with `DYNF` set. `CPF` (`crc_present`) is left at the base value
/// (false), so no HCRC/AHCRC/SICRC are emitted.
fn header_bytes(dynf: bool) -> Vec<u8> {
    // Same real BE header the in-crate subframe_pcm tests reuse.
    let hdr_bytes: [u8; 16] = [
        0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x3f, 0xf0, 0xb5, 0xe0, 0x01, 0x38, 0x00, 0x03, 0xef,
        0x7f,
    ];
    let mut header = parse_frame_header(&hdr_bytes).unwrap();
    header.dynamic_range = dynf;
    header.aspf = false;
    assert!(!header.crc_present, "base header must have CPF=0");
    oxideav_dts::encode_frame_header_be(&header).unwrap()
}

/// The §5.3.2 Audio Coding Header (Table 5-21) for one channel with
/// nSUBS=nVQSUB=2 (so there is no high-frequency VQ subband), linear
/// ABITS (BHUFF=6) and linear SCALES (SHUFF=5) codebooks, and a SEL
/// plane that puts the ABITS-8 group (plane slot 7, 3-bit width) at
/// SEL=7 — the terminal NoEncoding (NFE) entry — with every other slot
/// SEL=0. ADJ (2 bits) is transmitted for every SEL=0 slot (all but
/// slot 7), all set to 0 (Adj0 / unity). Only subband 0 carries bits
/// (ABITS=8); subband 1 is NoBits (ABITS=0).
fn ach_one_channel_nfe_body() -> Vec<(u32, u8)> {
    let mut body: Vec<(u32, u8)> = vec![
        (0, 4), // SUBFS -> 1 subframe
        (0, 3), // PCHS  -> 1 channel
        (0, 5), // SUBS  -> nSUBS 2  (we drive only subband 0)
        (1, 5), // VQSUB -> nVQSUB 2 (== nSUBS, so no high-frequency VQ)
        (0, 3), // JOINX
        (0, 2), // THUFF
        (5, 3), // SHUFF=5 -> SCALES 6-bit linear (raw 6-bit index)
        (6, 3), // BHUFF=6 -> ABITS 5-bit linear (raw 5-bit index)
    ];
    // SEL plane (ABITS-major), widths 1,2,2,2,2,3,3,3,3,3. Slot 7 = 7.
    let widths = [1u8, 2, 2, 2, 2, 3, 3, 3, 3, 3];
    for (n, &w) in widths.iter().enumerate() {
        let v = if n == 7 { 7 } else { 0 };
        body.push((v, w));
    }
    // ADJ plane: a 2-bit ADJ follows each SEL=0 slot (every slot but 7).
    for n in 0..10 {
        if n != 7 {
            body.push((0, 2));
        }
    }
    body
}

/// The §5.4.1 side info (Table 5-28) SCALES block for one subframe with
/// subband 0 carrying ABITS=8 (so the §5.5 NFE path runs) and a non-zero
/// SCALES factor. With BHUFF=6 the ABITS plane is a raw 5-bit field per
/// subband, and with SHUFF=5 the SCALES factor is a raw 6-bit index into
/// the §D.1.1 RMS table — both linear, so no Huffman code to look up.
fn side_info_one_subband_abits8() -> Vec<(u32, u8)> {
    // SSC=0 -> nSSC 1; PSC=0; PMODE[0][0..2]=0 (2 bits, nSUBS=2);
    // ABITS plane (BHUFF=6 Linear5Bit): subband 0 -> 8, subband 1 -> 0.
    let mut body: Vec<(u32, u8)> = vec![
        (0, 2), // SSC
        (0, 3), // PSC
        (0, 1), // PMODE[0][0]
        (0, 1), // PMODE[0][1]
        (8, 5), // ABITS[0][0] = 8  (Linear5Bit)
        (0, 5), // ABITS[0][1] = 0  (Linear5Bit)
    ];
    // SCALES plane: subband 0 has ABITS>0 -> one SHUFF=5 (6-bit linear)
    // factor. A non-zero RMS index is needed for non-zero PCM; index 20
    // is comfortably inside the §D.1.1 64-entry table.
    body.push((20, 6)); // SCALES[0][0] via SHUFF=5 (6-bit linear index)
    body
}

/// Build a full one-channel NFE Core frame. `dynf`/`range_index` insert
/// the §5.4.1 `RANGE` tail; the §5.5 audio data carries `samples` (8
/// signed 5-bit NFE words for subband 0's single subsubframe).
fn build_frame(dynf: bool, range_index: u8, samples: &[i32; 8]) -> Vec<u8> {
    let mut bytes = header_bytes(dynf);
    let mut body = ach_one_channel_nfe_body();
    body.extend(side_info_one_subband_abits8());
    if dynf {
        body.push((u32::from(range_index), 8)); // RANGE index
    }
    // §5.5 Audio Data: subband 0, ABITS=8 -> NFE width 5, 8 samples.
    for &s in samples {
        body.push(((s as u32) & 0x1f, 5));
    }
    body.push((0xffff, 16)); // DSYNC trailer (last subsubframe)
    let body_bytes = pack_fields(&body);
    bytes.extend_from_slice(&body_bytes);
    bytes.extend_from_slice(&[0u8; 4]); // header-lookahead slack
    bytes
}

/// Round-to-nearest, i32-saturating scale — mirrors the crate's
/// `apply_range`.
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

#[test]
fn baseline_nfe_frame_decodes_to_nonzero_pcm() {
    let samples = [7i32, -7, 5, -5, 3, -3, 6, -6];
    let bytes = build_frame(false, 0, &samples);
    let header = parse_frame_header(&bytes).unwrap();
    assert!(!header.dynamic_range);

    let pcm = decode_core_frame(&bytes, &header).expect("baseline frame decodes");
    assert_eq!(pcm.len(), 1);
    assert_eq!(pcm[0].len(), 256); // 1 subsubframe * 8 rows * 32 bands
    assert!(
        pcm[0].iter().any(|&s| s != 0),
        "non-zero audio data must produce non-zero PCM"
    );
}

#[test]
fn dynf_frame_scales_baseline_pcm_by_d4_range() {
    let samples = [7i32, -7, 5, -5, 3, -3, 6, -6];

    let base_bytes = build_frame(false, 0, &samples);
    let base_hdr = parse_frame_header(&base_bytes).unwrap();
    let base_pcm = decode_core_frame(&base_bytes, &base_hdr).expect("baseline decodes");

    // RANGE index 207 -> §D.4 multiplier 10.0 (+20 dB).
    let range_index = 207u8;
    let range = drc_range(range_index);
    assert_eq!(range, 10.0);

    let dynf_bytes = build_frame(true, range_index, &samples);
    let dynf_hdr = parse_frame_header(&dynf_bytes).unwrap();
    assert!(dynf_hdr.dynamic_range);
    let dynf_pcm = decode_core_frame(&dynf_bytes, &dynf_hdr).expect("DYNF frame decodes");

    assert_eq!(dynf_pcm.len(), base_pcm.len());
    assert_eq!(dynf_pcm[0].len(), base_pcm[0].len());

    // Every sample is the baseline scaled by the §D.4 RANGE multiplier.
    for (i, (&b, &d)) in base_pcm[0].iter().zip(dynf_pcm[0].iter()).enumerate() {
        assert_eq!(
            d,
            scale_sat(b, range),
            "sample {i}: baseline {b} * {range} != DYNF {d}"
        );
    }
    // And the DYNF PCM is genuinely louder somewhere (proves it ran).
    assert!(dynf_pcm[0]
        .iter()
        .zip(&base_pcm[0])
        .any(|(&d, &b)| d.abs() > b.abs()));
}

#[test]
fn dynf_unity_range_equals_baseline() {
    let samples = [7i32, -7, 5, -5, 3, -3, 6, -6];
    let base_bytes = build_frame(false, 0, &samples);
    let base_hdr = parse_frame_header(&base_bytes).unwrap();
    let base_pcm = decode_core_frame(&base_bytes, &base_hdr).unwrap();

    // RANGE index 127 -> §D.4 unity multiplier 1.0 (0 dB): the DYNF
    // frame must decode bit-identically to the baseline (the only stream
    // difference being the consumed header bit + the 8-bit RANGE field).
    let unity_bytes = build_frame(true, 127, &samples);
    let unity_hdr = parse_frame_header(&unity_bytes).unwrap();
    let unity_pcm = decode_core_frame(&unity_bytes, &unity_hdr).unwrap();

    assert_eq!(unity_pcm, base_pcm);
}
