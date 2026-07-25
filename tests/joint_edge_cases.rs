//! Joint-intensity **boundary battery**: every §5.4.1 / §C.2.3 edge
//! the spec's joint-coding clauses admit, each validated bit-exactly
//! against an analytic reconstruction (the frames are spec-built with
//! known content — see `tests/common/mod.rs`) or, for the malformed
//! variants, against the exact typed error. Bit-budget accounting is
//! implicitly exact in every positive case: a one-bit drift in the
//! JOIN_SHUFF / JOIN_SCALES / RANGE / SICRC tail walk would misalign
//! the §5.5 region and break the `DSYNC` check or the PCM equality.

mod common;

use common::{build_frame_from_spec, scales_index, JointFrameSpec, Lcg};
use oxideav_dts::{
    decode_core_frame, dts_dynrng_to_linear, join_scale, parse_frame_header, CoreFrameDecodeError,
    DtsFrameHeader, Error, MultiChannelQmf, StepSizeTable, NUM_SUBBAND, RMS_6BIT,
};

fn template() -> DtsFrameHeader {
    parse_frame_header(include_bytes!("fixtures/dts_5_frames.bin")).expect("template header parses")
}

/// Analytic PCM for a [`JointFrameSpec`] frame: recompute every
/// subframe's per-subband matrices from the builder-known content
/// (LCG × §D.2 step × §D.1.1 scale), apply the §C.2.3 import and the
/// per-subframe `RANGE` gain, and run one continuous §C.2.5 QMF.
fn analytic_pcm(spec: &JointFrameSpec, header: &DtsFrameHeader) -> Vec<Vec<i32>> {
    let table = StepSizeTable::for_rate(header.rate_index);
    let step = table.step_size(8).expect("ABITS=8 step size");
    let rows = spec.n_ssc * 8;

    // Effective per-channel QMF counts per the §C.2.5 driving-call
    // note: a jointly-coded channel synthesizes over the source's
    // count when the source is wider.
    let mut eff = spec.n_subs;
    for (ch, slot) in eff.iter_mut().enumerate() {
        if spec.joinx[ch] > 0 {
            let src = (spec.joinx[ch] - 1) as usize;
            *slot = (*slot).max(spec.n_subs[src]);
        }
    }

    let gain = spec.dynf_code.map(dts_dynrng_to_linear);
    let mut qmf = MultiChannelQmf::new(2);
    let mut pcm: Vec<Vec<i32>> = vec![Vec::new(); 2];
    let mut lcg = Lcg(spec.seed);

    for _subframe in 0..spec.n_subframes {
        let mut matrices = vec![vec![[0.0f64; NUM_SUBBAND]; rows]; 2];
        for ssf in 0..spec.n_ssc {
            for (ch, matrix) in matrices.iter_mut().enumerate() {
                for n in 0..spec.n_subs[ch] {
                    let scale = f64::from(RMS_6BIT[scales_index(n) as usize]);
                    for m in 0..8 {
                        let index = lcg.nfe_sample();
                        matrix[ssf * 8 + m][n] = step * scale * f64::from(index);
                    }
                }
            }
        }
        // §C.2.3 joint import.
        for ch in 0..2 {
            if spec.joinx[ch] == 0 {
                continue;
            }
            let src = (spec.joinx[ch] - 1) as usize;
            let (lo, hi) = (spec.n_subs[ch], spec.n_subs[src]);
            if hi <= lo {
                continue;
            }
            let source = matrices[src].clone();
            for (dst_row, src_row) in matrices[ch].iter_mut().zip(&source) {
                for (k, &sym) in spec.join_symbols.iter().enumerate() {
                    let n = lo + k;
                    let factor = join_scale(sym + 64).expect("biased index inside §D.3");
                    dst_row[n] = factor * src_row[n];
                }
            }
        }

        // §C.2.5 synthesis (continuous across subframes), then the
        // §5.4.1 per-subframe RANGE gain on this subframe's PCM.
        let refs: Vec<&[[f64; NUM_SUBBAND]]> = matrices.iter().map(|m| m.as_slice()).collect();
        let mut block: Vec<Vec<i32>> = vec![Vec::new(); 2];
        qmf.synthesize_planar(
            &refs,
            &eff,
            header.filter_bank_selection(),
            header.output_r_scale().unwrap(),
            &mut block,
        )
        .expect("analytic QMF synthesis");
        if let Some(g) = gain {
            if g != 1.0 {
                for plane in block.iter_mut() {
                    for s in plane.iter_mut() {
                        let scaled = (*s as f64 * g).round();
                        *s = if scaled >= i32::MAX as f64 {
                            i32::MAX
                        } else if scaled <= i32::MIN as f64 {
                            i32::MIN
                        } else {
                            scaled as i32
                        };
                    }
                }
            }
        }
        for ch in 0..2 {
            pcm[ch].extend(&block[ch]);
        }
    }
    pcm
}

fn decode_spec(spec: &JointFrameSpec) -> Result<Vec<Vec<i32>>, CoreFrameDecodeError> {
    let frame = build_frame_from_spec(&template(), spec);
    let header = parse_frame_header(&frame).expect("spec frame header parses");
    decode_core_frame(&frame, &header)
}

fn assert_matches_analytic(spec: &JointFrameSpec) {
    let frame = build_frame_from_spec(&template(), spec);
    let header = parse_frame_header(&frame).expect("spec frame header parses");
    let ours = decode_core_frame(&frame, &header).expect("spec frame decodes");
    let expect = analytic_pcm(spec, &header);
    assert_eq!(
        ours, expect,
        "decode must equal the analytic reconstruction"
    );
    let peak = ours[1].iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 1000, "non-silent (peak {peak})");
}

/// `JOINX` pointing **forward**: channel 0 (16 sub-bands) imports from
/// channel 1 (32 sub-bands). The §C.2.3 copy runs after the whole §5.5
/// walk, so a source that appears *later* in channel order works
/// identically — and the JOIN_SHUFF/JOIN_SCALES tail is read for
/// channel 0 rather than channel 1.
#[test]
fn forward_source_joint_decodes() {
    let spec = JointFrameSpec {
        n_subs: [16, 32],
        joinx: [2, 0],
        seed: 0xA1B2_C3D4,
        ..JointFrameSpec::default_joint(0)
    };
    assert_matches_analytic(&spec);
}

/// Huffman `JOIN_SHUFF = 0` (SA129 → §D.5.3 Table A5): the joint scale
/// symbols are entropy-coded signed differences around zero, each
/// independently biased by +64 into the §D.3 table (no running
/// accumulator, unlike the SCALES walk).
#[test]
fn huffman_join_shuff_a5_decodes() {
    let symbols: Vec<i32> = (0..16).map(|k| [0, 1, -1, 2, -2][k % 5]).collect();
    let spec = JointFrameSpec {
        join_shuff: 0,
        join_symbols: symbols,
        seed: 0xB005_0005,
        ..JointFrameSpec::default_joint(0)
    };
    assert_matches_analytic(&spec);
}

/// `JOINX` + `DYNF` + `CPF` together: the Table 5-28 tail carries all
/// four fields in spec order — JOIN_SHUFF, JOIN_SCALES, RANGE, SICRC —
/// and the header/ACH grow their HCRC/AHCRC words. The signed-Q2 gain
/// (+4 dB, code 16) applies to the PCM after synthesis. Any cursor
/// drift in that tail walk would break the DSYNC or the equality.
#[test]
fn joint_with_dynf_and_cpf_tail_order() {
    let spec = JointFrameSpec {
        dynf_code: Some(16), // +4 dB (signed Q2)
        cpf: true,
        // HCRC + AHCRC + RANGE + SICRC add ~7 bytes over the default.
        frame_bytes: 640,
        seed: 0xD1A7_0001,
        ..JointFrameSpec::default_joint(0)
    };
    assert_matches_analytic(&spec);
}

/// Two subframes (`SUBFS = 1`): each subframe re-reads its own
/// JOIN_SHUFF / JOIN_SCALES tail, and the §C.2.5 filter runs
/// continuously across the subframe boundary. NBLKS doubles to 31
/// (32 blocks = 1024 PCM samples per channel).
#[test]
fn multi_subframe_joint_frame_decodes() {
    let spec = JointFrameSpec {
        n_subframes: 2,
        frame_bytes: 1184, // two subframes' payload + slack
        seed: 0x5B00_0002,
        ..JointFrameSpec::default_joint(0)
    };
    let ours = decode_spec(&spec).expect("two-subframe joint frame decodes");
    assert_eq!(ours[0].len(), 1024);
    assert_eq!(ours[1].len(), 1024);
    assert_matches_analytic(&spec);
}

/// Reserved `JOIN_SHUFF = 7` is rejected as a typed side-info error,
/// not a panic or a misdecode.
#[test]
fn reserved_join_shuff_is_typed_error() {
    // The builder cannot encode symbols for the reserved book; write
    // none (the decoder must fail on the selector before any symbol).
    let spec = JointFrameSpec {
        join_shuff: 7,
        join_symbols: Vec::new(),
        ..JointFrameSpec::default_joint(0)
    };
    let err = decode_spec(&spec).expect_err("JOIN_SHUFF=7 must be rejected");
    match err {
        CoreFrameDecodeError::Bitstream(Error::InvalidSideInfo { field, value }) => {
            assert_eq!(field, "JOIN_SHUFF");
            assert_eq!(value, 7);
        }
        other => panic!("expected InvalidSideInfo(JOIN_SHUFF), got {other:?}"),
    }
}

/// `JOINX` naming a source channel beyond `nPCHS` is rejected as a
/// typed error (`nSourceCh` must exist).
#[test]
fn out_of_range_joinx_source_is_typed_error() {
    let spec = JointFrameSpec {
        joinx: [0, 5], // source channel 4 of a 2-channel frame
        join_symbols: Vec::new(),
        ..JointFrameSpec::default_joint(0)
    };
    let err = decode_spec(&spec).expect_err("JOINX=5 in a 2ch frame must be rejected");
    match err {
        CoreFrameDecodeError::Bitstream(Error::InvalidSideInfo { field, value }) => {
            assert_eq!(field, "JOINX");
            assert_eq!(value, 5);
        }
        other => panic!("expected InvalidSideInfo(JOINX), got {other:?}"),
    }
}

/// A biased `JOIN_SCALES` index outside the §D.3 table (Linear7Bit raw
/// 127 → biased 191 > 128) is rejected as a typed error.
#[test]
fn out_of_range_join_scale_index_is_typed_error() {
    let spec = JointFrameSpec {
        join_shuff: 6, // Linear7Bit
        join_symbols: vec![127; 16],
        ..JointFrameSpec::default_joint(0)
    };
    let err = decode_spec(&spec).expect_err("biased index 191 must be rejected");
    match err {
        CoreFrameDecodeError::Bitstream(Error::InvalidSideInfo { field, value }) => {
            assert_eq!(field, "JOIN_SCALES");
            assert_eq!(value, 191);
        }
        other => panic!("expected InvalidSideInfo(JOIN_SCALES), got {other:?}"),
    }
}

/// Linear7Bit `JOIN_SHUFF = 6` with in-range symbols decodes — the
/// upper half of the §D.3 table (indexes ≥ 64+63) stays reachable
/// through the 7-bit book without an accumulator.
#[test]
fn linear7_join_shuff_decodes() {
    let symbols: Vec<i32> = (0..16).map(|k| (k * 4) % 64).collect();
    let spec = JointFrameSpec {
        join_shuff: 6,
        join_symbols: symbols,
        seed: 0x7B17_0006,
        ..JointFrameSpec::default_joint(0)
    };
    assert_matches_analytic(&spec);
}

/// The whole default battery frame also decodes with `frame_bytes`
/// exactly at the packed payload (no padding slack): the decoder never
/// reads past the last DSYNC bit.
#[test]
fn zero_slack_frame_decodes() {
    // Find the exact payload size by building with generous slack and
    // trimming: rebuild at the smallest FSIZE that still fits.
    let mut spec = JointFrameSpec::default_joint(0x00_5EED);
    let frame = build_frame_from_spec(&template(), &spec);
    let used = frame
        .iter()
        .rposition(|&b| b != 0)
        .expect("frame has content")
        + 1;
    spec.frame_bytes = used.max(96); // FSIZE floor is 95+1
    assert_matches_analytic(&spec);
}
