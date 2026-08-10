//! End-to-end validation of the **recovered-§D.10-book decode paths**
//! — the §5.5 phase-1 high-frequency VQ subbands (`nVQSUB < nSUBS`)
//! and the §C.2.2 inverse-ADPCM prediction (`PMODE != 0`) — on
//! spec-built streams with **synthetic** stand-in books.
//!
//! The real §D.10 books' numeric contents are the recorded docs gap
//! (`docs/audio/dts/dts-d10-vq-tables-GAP.md`; the round-9 container
//! forensics settled that they were never in the staged PDF), so no
//! numeric ground truth exists to decode real streams against. What
//! *is* fully specified — and what these tests pin bit-exactly — is
//! everything **around** the numbers:
//!
//! * the §5.5 phase-1 walk (10-bit `nVQIndex` per HF subband, ahead
//!   of the LFE phase), the §D.10.2 two-int8-÷ 24 entry decoding, and
//!   the `SCALES[ch][n][0] · HFREQ[m]` fill over exactly the
//!   subframe's rows (the p.33 pick rule, including the
//!   termination-frame valid prefix);
//! * the §5.4.1 PVQ capture → §D.10.1 ÷ 2¹³ coefficient lookup →
//!   §C.2.2 4-tap in-place prediction per subsubframe, with the
//!   reconstruction history carried across subsubframes, subframes,
//!   and (per the §5.3.1 `HFLAG` gate) frames;
//! * the typed blocker surface staying exactly as before when a
//!   needed book is absent.
//!
//! Method (the joint-intensity round's pattern): the builder's frame
//! content is a pure function of its spec, so the expected subband
//! matrices are recomputed *without parsing* and pushed through the
//! public §C.2.5 QMF; `decode_*` must equal that bit-exactly.

mod common;

use common::{
    build_frame_from_spec, hf_vq_index, pvq_index, scales_index, synthetic_adpcm_book,
    synthetic_adpcm_coeff, synthetic_hf_book, synthetic_hf_element, synthetic_vq_codebooks,
    template_header, JointFrameSpec, Lcg,
};
use oxideav_dts::{
    decode_core_frame, join_scale, parse_frame_header, AudioArrayDecodeError, AudioArrayError,
    CoreFrameDecodeError, CoreStreamDecoder, DtsFrameHeader, FilterBankSelection, FrameType,
    MultiChannelQmf, StepSizeTable, SubframePcmDecoder, SubframePcmError, VqCodebooks, NUM_SUBBAND,
    RMS_6BIT,
};

/// The §C.2.2 4-tap predictor, run over one subband column with the
/// given priming history (`hist[3]` = the immediately preceding
/// sample) — the analytic mirror of the in-walk reconstruction.
fn predict_column(col: &mut [f64], coeffs: &[f64; 4], hist: &[f64; 4]) {
    for m in 0..col.len() {
        let mut acc = col[m];
        for (t, &c) in coeffs.iter().enumerate() {
            let past = if m > t {
                col[m - t - 1]
            } else {
                hist[4 + m - t - 1]
            };
            acc += c * past;
        }
        col[m] = acc;
    }
}

/// Recompute one frame's per-channel subband matrices straight from
/// the builder's spec (no bit-stream parsing), mirroring the §5.5
/// walk: phase-1 HF-VQ fill, LFE LCG draws (cursor alignment only),
/// NFE dequant, then per-subband §C.2.2 prediction. `history` is the
/// persistent per-`(ch, n)` four-sample reconstruction history
/// (oldest first), advanced exactly as the decoder advances its own.
fn analytic_matrices(
    spec: &JointFrameSpec,
    rate_index: u8,
    history: &mut [[[f64; 4]; NUM_SUBBAND]; 2],
) -> Vec<Vec<[f64; NUM_SUBBAND]>> {
    let table = StepSizeTable::for_rate(rate_index);
    let step = table.step_size(8).expect("ABITS=8 step size");
    let mut lcg = Lcg(spec.seed);
    let mut matrices: Vec<Vec<[f64; NUM_SUBBAND]>> = vec![Vec::new(), Vec::new()];

    for subframe in 0..spec.n_subframes {
        let sf_psc = if subframe == spec.n_subframes - 1 {
            spec.psc as usize
        } else {
            0
        };
        let rows = if sf_psc > 0 {
            (spec.n_ssc - 1) * 8 + sf_psc
        } else {
            spec.n_ssc * 8
        };
        let mut sf: Vec<Vec<[f64; NUM_SUBBAND]>> = vec![vec![[0.0; NUM_SUBBAND]; rows]; 2];

        // §5.5 phase 1: HF-VQ columns — SCALES[ch][n][0] · vector[m]
        // over the subframe's rows.
        let n_vqsub = spec.n_vqsub();
        for ch in 0..2 {
            for n in n_vqsub[ch]..spec.n_subs[ch] {
                let scale = f64::from(RMS_6BIT[scales_index(n) as usize]);
                let v = hf_vq_index(subframe, ch, n) as usize;
                for (m, row) in sf[ch].iter_mut().enumerate() {
                    row[n] = scale * synthetic_hf_element(v, m);
                }
            }
        }

        // §5.5 LFE phase: the builder draws 2·LFF·nSSC LCG samples
        // before the audio data; mirror the draws to stay aligned.
        if spec.lfe {
            for _ in 0..2 * spec.n_ssc {
                lcg.next_u32();
            }
        }

        // §5.5 audio data: NFE dequant in walk order.
        for ssf in 0..spec.n_ssc {
            let count = if sf_psc > 0 && ssf == spec.n_ssc - 1 {
                sf_psc
            } else {
                8
            };
            for ch in 0..2 {
                for n in 0..n_vqsub[ch] {
                    let scale = f64::from(RMS_6BIT[scales_index(n) as usize]);
                    for m in 0..count {
                        let index = lcg.nfe_sample();
                        sf[ch][ssf * 8 + m][n] = step * scale * f64::from(index);
                    }
                }
            }
        }

        // §C.2.2 prediction over the PMODE subbands (continuous across
        // the subframe's subsubframes), then advance every subband's
        // history over the subframe's final rows.
        for ch in 0..2 {
            for n in 0..spec.adpcm_subbands[ch] {
                let i = pvq_index(subframe, ch, n) as usize;
                let coeffs = [0, 1, 2, 3].map(|j| synthetic_adpcm_coeff(i, j));
                let mut col: Vec<f64> = sf[ch].iter().map(|row| row[n]).collect();
                predict_column(&mut col, &coeffs, &history[ch][n]);
                for (row, &value) in sf[ch].iter_mut().zip(&col) {
                    row[n] = value;
                }
            }
            let take = rows.min(4);
            for n in 0..NUM_SUBBAND {
                let mut h = history[ch][n];
                h.copy_within(take.., 0);
                for k in 0..take {
                    h[4 - take + k] = sf[ch][rows - take + k][n];
                }
                history[ch][n] = h;
            }
        }

        for ch in 0..2 {
            matrices[ch].extend(sf[ch].iter().copied());
        }
    }
    matrices
}

/// Push analytic matrices through the public §C.2.5 QMF with the
/// header-derived `FILTS` / output `rScale`.
fn synthesize(
    qmf: &mut MultiChannelQmf,
    matrices: &[Vec<[f64; NUM_SUBBAND]>],
    n_subs: [usize; 2],
    header: &DtsFrameHeader,
) -> Vec<Vec<i32>> {
    let refs: Vec<&[[f64; NUM_SUBBAND]]> = matrices.iter().map(|m| m.as_slice()).collect();
    let filter: FilterBankSelection = header.filter_bank_selection();
    let r_scale = header.output_r_scale().expect("PCMR not reserved");
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    qmf.synthesize_planar(&refs, &n_subs, filter, r_scale, &mut pcm)
        .expect("analytic QMF synthesis");
    pcm
}

fn decode_with_books(frame: &[u8], header: &DtsFrameHeader, books: VqCodebooks) -> Vec<Vec<i32>> {
    let mut dec = SubframePcmDecoder::new(2);
    dec.set_vq_codebooks(books);
    dec.decode_core_frame_into(frame, header)
        .expect("frame decodes with recovered books")
}

fn blocked_kind(err: CoreFrameDecodeError) -> (bool, usize, usize) {
    match err {
        CoreFrameDecodeError::Decode(SubframePcmError::AudioData(
            AudioArrayDecodeError::Blocked(AudioArrayError::VqCodebookUnavailable {
                ch,
                n,
                high_frequency_vq,
            }),
        )) => (high_frequency_vq, ch, n),
        other => panic!("expected the §D.10 blocker, got {other:?}"),
    }
}

/// A high-frequency-VQ frame (`nVQSUB < nSUBS` on both channels)
/// decodes bit-exactly to the analytic reconstruction once the
/// §D.10.2 book is supplied — pinning the phase-1 cursor math (the
/// 10-bit indices sit *before* the LFE phase), the index order, the
/// entry unpacking, and the `SCALES · HFREQ` fill.
#[test]
fn hf_vq_frame_decode_matches_analytic_reconstruction() {
    let template = template_header();
    let spec = JointFrameSpec {
        hf_subbands: [8, 4],
        ..JointFrameSpec::default_plain(0xD10_00001)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);

    assert_eq!(ours, expect, "HF-VQ decode must match the analytic fill");
    // Non-vacuous: the HF columns carry real energy — zeroing them
    // changes the PCM.
    let mut ablated = matrices;
    for (ch, m) in ablated.iter_mut().enumerate() {
        for row in m.iter_mut() {
            for slot in row.iter_mut().take(32).skip(spec.n_vqsub()[ch]) {
                *slot = 0.0;
            }
        }
    }
    let ablated_pcm = synthesize(&mut MultiChannelQmf::new(2), &ablated, [32, 32], &header);
    assert_ne!(ours, ablated_pcm, "the HF-VQ fill must be audible");
}

/// A book-stripped decoder ([`VqCodebooks::none`]) surfaces the exact
/// typed HF-VQ blocker (first offending channel/subband), also when
/// carrying only the ADPCM book — while the default decoder (built-in
/// real books, round 439) decodes the same frame outright.
#[test]
fn hf_vq_frame_without_book_stays_blocked() {
    let template = template_header();
    let spec = JointFrameSpec {
        hf_subbands: [0, 4],
        ..JointFrameSpec::default_plain(0xD10_00002)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    // The default decoder now carries the built-in real books: the
    // frame that used to hit the §D.10 wall decodes.
    decode_core_frame(&frame, &header).expect("built-in books decode HF-VQ frames by default");

    let mut bare = SubframePcmDecoder::new(2);
    bare.set_vq_codebooks(VqCodebooks::none());
    let (hf, ch, n) = blocked_kind(bare.decode_core_frame_into(&frame, &header).unwrap_err());
    assert!(hf, "the missing book is the §D.10.2 HF-VQ one");
    assert_eq!((ch, n), (1, 28), "first HF-VQ subband of channel 1");

    let mut dec = SubframePcmDecoder::new(2);
    dec.set_vq_codebooks(VqCodebooks::none().with_adpcm(synthetic_adpcm_book()));
    let (hf, _, _) = blocked_kind(dec.decode_core_frame_into(&frame, &header).unwrap_err());
    assert!(hf, "the ADPCM book alone must not unblock HF-VQ");
}

/// An ADPCM frame (`PMODE = 1` on leading subbands of both channels,
/// two subframes) decodes bit-exactly to the analytic reconstruction:
/// PVQ lookup, ÷ 2¹³ coefficients, per-subsubframe §C.2.2 prediction,
/// and the history carry across subsubframes AND subframes.
#[test]
fn adpcm_frame_decode_matches_analytic_reconstruction() {
    let template = template_header();
    let spec = JointFrameSpec {
        adpcm_subbands: [4, 2],
        n_subframes: 2,
        ..JointFrameSpec::default_plain(0xD10_00003)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "ADPCM decode must match the analytic chain");

    // Non-vacuous: prediction really ran — the no-prediction analytic
    // differs.
    let mut hist2 = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let no_adpcm = JointFrameSpec {
        n_subframes: 2,
        ..JointFrameSpec::default_plain(0xD10_00003)
    };
    let plain = analytic_matrices(&no_adpcm, header.rate_index, &mut hist2);
    // Same residuals, no prediction: the PVQ/PMODE planes change the
    // side-info bits but not the LCG audio content.
    let plain_pcm = synthesize(&mut MultiChannelQmf::new(2), &plain, [32, 32], &header);
    assert_ne!(ours, plain_pcm, "the §C.2.2 prediction must be audible");
}

/// A book-stripped decoder surfaces the exact typed ADPCM blocker
/// (also when only the HF-VQ book is attached) — while the default
/// decoder (built-in real books) decodes the same frame outright.
#[test]
fn adpcm_frame_without_book_stays_blocked() {
    let template = template_header();
    let spec = JointFrameSpec {
        adpcm_subbands: [0, 3],
        ..JointFrameSpec::default_plain(0xD10_00004)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    decode_core_frame(&frame, &header).expect("built-in books decode ADPCM frames by default");

    let mut bare = SubframePcmDecoder::new(2);
    bare.set_vq_codebooks(VqCodebooks::none());
    let (hf, ch, n) = blocked_kind(bare.decode_core_frame_into(&frame, &header).unwrap_err());
    assert!(!hf, "the missing book is the §D.10.1 ADPCM one");
    assert_eq!((ch, n), (1, 0), "first PMODE subband of channel 1");

    let mut dec = SubframePcmDecoder::new(2);
    dec.set_vq_codebooks(VqCodebooks::none().with_hfreq(synthetic_hf_book()));
    let (hf, _, _) = blocked_kind(dec.decode_core_frame_into(&frame, &header).unwrap_err());
    assert!(!hf, "the HF-VQ book alone must not unblock ADPCM");
}

/// §5.3.1 `HFLAG = 1`: the second frame's prediction is primed by the
/// first frame's reconstruction history ("the decoder will use
/// reconstruction history of the previous frame"); with `HFLAG = 0`
/// the history is ignored. Both gates are pinned bit-exactly, and the
/// two decodes differ (the gate is audible).
#[test]
fn hflag_gates_cross_frame_adpcm_history() {
    let template = template_header();
    let spec1 = JointFrameSpec {
        adpcm_subbands: [4, 4],
        ..JointFrameSpec::default_plain(0xD10_00005)
    };
    let frame1 = build_frame_from_spec(&template, &spec1);
    let header1 = parse_frame_header(&frame1).expect("frame 1 parses");

    for hflag in [true, false] {
        let spec2 = JointFrameSpec {
            adpcm_subbands: [4, 4],
            predictor_history: hflag,
            ..JointFrameSpec::default_plain(0xD10_00006)
        };
        let frame2 = build_frame_from_spec(&template, &spec2);
        let header2 = parse_frame_header(&frame2).expect("frame 2 parses");

        let mut dec = CoreStreamDecoder::new(2);
        dec.set_vq_codebooks(synthetic_vq_codebooks());
        let pcm1 = dec.decode_frame(&frame1, &header1).expect("frame 1");
        let pcm2 = dec.decode_frame(&frame2, &header2).expect("frame 2");

        // Analytic mirror with one persistent QMF and the same gate.
        let mut qmf = MultiChannelQmf::new(2);
        let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
        let m1 = analytic_matrices(&spec1, header1.rate_index, &mut history);
        let e1 = synthesize(&mut qmf, &m1, [32, 32], &header1);
        assert_eq!(pcm1, e1, "frame 1 (HFLAG={hflag}) must match");
        if !hflag {
            history = [[[0.0; 4]; NUM_SUBBAND]; 2];
        }
        let m2 = analytic_matrices(&spec2, header2.rate_index, &mut history);
        let e2 = synthesize(&mut qmf, &m2, [32, 32], &header2);
        assert_eq!(pcm2, e2, "frame 2 (HFLAG={hflag}) must match");
    }

    // The gate is audible: decode frame 2 both ways behind the same
    // frame 1 and require different PCM.
    let mut outs: Vec<Vec<Vec<i32>>> = Vec::new();
    for hflag in [true, false] {
        let spec2 = JointFrameSpec {
            adpcm_subbands: [4, 4],
            predictor_history: hflag,
            ..JointFrameSpec::default_plain(0xD10_00006)
        };
        let frame2 = build_frame_from_spec(&template, &spec2);
        let header2 = parse_frame_header(&frame2).expect("frame 2 parses");
        let mut dec = CoreStreamDecoder::new(2);
        dec.set_vq_codebooks(synthetic_vq_codebooks());
        dec.decode_frame(&frame1, &header1).expect("frame 1");
        outs.push(dec.decode_frame(&frame2, &header2).expect("frame 2"));
    }
    assert_ne!(
        outs[0], outs[1],
        "carrying vs ignoring the previous frame's history must differ"
    );
}

/// Termination frame × HF-VQ: the phase-1 fill picks exactly the
/// valid prefix (`(nSSC−1)·8 + PSC` rows) of each 32-sample vector —
/// the p.33 pad rule — and the PCM length is the §5.4.1 partial
/// budget.
#[test]
fn termination_frame_hf_vq_fills_valid_prefix_only() {
    let template = template_header();
    let spec = JointFrameSpec {
        hf_subbands: [8, 0],
        frame_type: FrameType::Termination,
        psc: 5,
        short_raw: 10,
        ..JointFrameSpec::default_plain(0xD10_00007)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("termination frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());
    assert_eq!(ours[0].len(), 13 * 32, "(2·8 − 3) blocks × 32 PCM");

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    assert_eq!(matrices[0].len(), 13);
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "partial-subsubframe HF fill must match");
}

/// Termination frame × ADPCM: the §C.2.2 prediction runs over the
/// partial subsubframe's `PSC` residuals and the history advance uses
/// the truncated final rows.
#[test]
fn termination_frame_adpcm_predicts_partial_subsubframe() {
    let template = template_header();
    let spec = JointFrameSpec {
        adpcm_subbands: [3, 0],
        frame_type: FrameType::Termination,
        psc: 3,
        short_raw: 4,
        ..JointFrameSpec::default_plain(0xD10_00008)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("termination frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());
    assert_eq!(ours[0].len(), 11 * 32);

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "partial-subsubframe prediction must match");
}

/// Kitchen sink: HF-VQ + ADPCM + LFE + joint-intensity + DYNF in one
/// frame — every §5.5 phase in play at once, all cursors exact. The
/// jointly-coded channel imports sub-bands from a source whose own
/// upper range is HF-VQ-filled, so the §C.2.3 copy consumes §D.10.2
/// output.
#[test]
fn combined_hf_adpcm_lfe_joint_frame_matches_analytic() {
    let template = template_header();
    let spec = JointFrameSpec {
        n_subs: [32, 16],
        hf_subbands: [8, 0],
        adpcm_subbands: [2, 1],
        lfe: true,
        dynf_code: Some(0), // DYNF present, 0 dB (unity gain)
        ..JointFrameSpec::default_joint(0xD10_00009)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("combined frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let mut matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    // §C.2.3 joint import: ch1 pulls sub-bands 16..32 from ch0 —
    // including ch0's HF-VQ-filled 24..32 — scaled by the §D.3 ramp.
    let source = matrices[0].clone();
    for (dst_row, src_row) in matrices[1].iter_mut().zip(&source) {
        for (k, &raw) in common::JOINT_SCALE_RAW.iter().enumerate() {
            let n = 16 + k;
            let factor = join_scale(raw as i32 + 64).expect("biased §D.3 index");
            dst_row[n] = factor * src_row[n];
        }
    }
    // Effective nSUBS: destination widened to the source's 32.
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "all §5.5 phases must compose bit-exactly");
}

/// A truncated phase-1 HF-VQ region (stream cut mid-index) surfaces a
/// typed EOF, not a panic or a mis-aligned decode.
#[test]
fn truncated_hf_vq_region_is_typed_eof() {
    let template = template_header();
    let spec = JointFrameSpec {
        hf_subbands: [28, 0],
        ..JointFrameSpec::default_plain(0xD10_0000A)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    // Locate the first §5.5 bit (phase 1) by walking the side info
    // with the crate's own cursor math, then cut inside the 28-index
    // (280-bit) phase-1 region.
    let hb = header.header_bit_length() as usize;
    let (coding, ach_bits) =
        oxideav_dts::decode_audio_coding_header_at(&frame, hb, header.crc_present)
            .expect("audio coding header");
    let (side, side_bits) =
        oxideav_dts::decode_primary_side_info_at(&frame, hb + ach_bits, &coding.channel_params)
            .expect("side info");
    let (_tail, tail_bits) = oxideav_dts::decode_primary_side_info_tail_at(
        &frame,
        hb + ach_bits + side_bits,
        &coding.joinx,
        &coding.n_subs(),
        header.dynamic_range,
        header.crc_present,
    )
    .expect("side-info tail");
    assert_eq!(side.subsubframe_count.psc, 0);
    let phase1_bit = hb + ach_bits + side_bits + tail_bits;
    let cut = (phase1_bit + 100) / 8; // inside the 280-bit region

    let mut dec = SubframePcmDecoder::new(2);
    dec.set_vq_codebooks(synthetic_vq_codebooks());
    let err = dec
        .decode_core_frame_into(&frame[..cut], &header)
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreFrameDecodeError::Decode(SubframePcmError::AudioData(
                AudioArrayDecodeError::Bitstream(oxideav_dts::Error::UnexpectedEof)
            ))
        ),
        "got {err:?}"
    );
}

/// A plain common-Core frame decodes identically whichever books are
/// attached — built-in (the default), synthetic, or none: the books
/// only *add* reachable frames.
#[test]
fn books_do_not_perturb_common_core_decode() {
    let template = template_header();
    let spec = JointFrameSpec::default_plain(0xD10_0000B);
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("synthetic frame parses");

    let plain = decode_core_frame(&frame, &header).expect("common Core decodes");
    let with_books = decode_with_books(&frame, &header, synthetic_vq_codebooks());
    assert_eq!(plain, with_books);
}

/// `nSSC = 4` (the maximum subframe): the HF fill consumes **all 32**
/// elements of each §D.10.2 vector — the p.33 "maximum possible
/// subframe" case with no pad — and still matches the analytic
/// reconstruction bit-exactly.
#[test]
fn nssc4_hf_vq_uses_full_32_element_vectors() {
    let template = template_header();
    let spec = JointFrameSpec {
        n_ssc: 4,
        hf_subbands: [8, 4],
        ..JointFrameSpec::default_plain(0xD10_0000C)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("nSSC=4 frame parses");

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());
    assert_eq!(ours[0].len(), 32 * 32, "4 subsubframes x 8 rows x 32 PCM");

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    assert_eq!(matrices[0].len(), 32, "all 32 vector elements consumed");
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "full-vector HF fill must match");
}

/// Two subframes x HF-VQ x ADPCM x ASPF x CPF: per-subframe phase-1
/// index regions (different indices per subframe), per-subframe PVQ
/// planes, a DSYNC after every subsubframe, HCRC/AHCRC/SICRC framing
/// — all cursors exact, bit-exact against the analytic chain.
#[test]
fn multi_subframe_hf_adpcm_aspf_cpf_grid_matches_analytic() {
    let template = template_header();
    let spec = JointFrameSpec {
        n_subframes: 2,
        hf_subbands: [8, 4],
        adpcm_subbands: [3, 2],
        aspf: true,
        cpf: true,
        ..JointFrameSpec::default_plain(0xD10_0000D)
    };
    let frame = build_frame_from_spec(&template, &spec);
    let header = parse_frame_header(&frame).expect("grid frame parses");
    assert!(header.crc_present && header.aspf);

    let ours = decode_with_books(&frame, &header, synthetic_vq_codebooks());
    assert_eq!(ours[0].len(), 2 * 512);

    let mut history = [[[0.0; 4]; NUM_SUBBAND]; 2];
    let matrices = analytic_matrices(&spec, header.rate_index, &mut history);
    let expect = synthesize(&mut MultiChannelQmf::new(2), &matrices, [32, 32], &header);
    assert_eq!(ours, expect, "the full grid must compose bit-exactly");
}

/// Attaching books does not perturb the decode of the bundled *real*
/// encoder streams (both are common-Core: no HF-VQ, no PMODE) — the
/// recovered-book paths are strictly additive.
#[test]
fn books_do_not_perturb_real_fixture_streams() {
    use oxideav_dts::iter_frames;
    for (fixture, channels) in [
        (&include_bytes!("fixtures/dts_5_frames.bin")[..], 2usize),
        (&include_bytes!("fixtures/dts_51_lfe.bin")[..], 5),
    ] {
        let mut plain = CoreStreamDecoder::new(channels);
        let mut with_books = CoreStreamDecoder::new(channels);
        with_books.set_vq_codebooks(synthetic_vq_codebooks());
        for fv in iter_frames(fixture) {
            let fv = fv.expect("real fixture frames iterate cleanly");
            let a = plain.decode_frame(fv.data, &fv.header).expect("plain");
            let b = with_books.decode_frame(fv.data, &fv.header).expect("books");
            assert_eq!(a, b, "books must be strictly additive");
        }
    }
}
