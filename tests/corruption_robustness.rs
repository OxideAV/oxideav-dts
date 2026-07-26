//! Deterministic corruption-robustness sweep over the bundled real
//! fixtures: every decoder entry point must return a typed error (or
//! decode successfully) on damaged input — never panic, never hang,
//! never allocate absurdly.
//!
//! The §5.x walkers navigate by size/count fields read from the
//! stream, so single-byte damage exercises the structural bounds
//! (NBLKS/FSIZE gates, side-info selectors, Huffman prefix failures,
//! DSYNC mismatches, EOF checks) and — since round 408 — the Annex B
//! CRC gates on the §5.7 chunks. This is a fast in-CI net over the
//! whole raw-bytes-to-PCM surface; the sweep is exhaustive over
//! single-byte XORs at a fixed stride plus a pseudo-random multi-byte
//! pass, both fully deterministic.

use oxideav_dts::{iter_frames_resync, CoreStreamDecoder};

const STEREO: &[u8] = include_bytes!("fixtures/dts_5_frames.bin");
const FIVE_ONE: &[u8] = include_bytes!("fixtures/dts_51_lfe.bin");
/// The spec-built joint-intensity stream (round 429): corruption here
/// additionally exercises the Table 5-28 JOIN_SHUFF / JOIN_SCALES tail
/// error paths (reserved selector, out-of-range biased §D.3 index,
/// bad JOINX source) under damage.
const JOINT: &[u8] = include_bytes!("fixtures/dts_joint_5_frames.bin");
/// The spec-built termination-ended stream (round 430): corruption
/// here additionally exercises the §5.4.1 PSC paths under damage — a
/// flipped FTYPE bit trips the partial-in-normal-frame decline, a
/// damaged SSC/PSC prefix reshapes the partial subsubframe's bit
/// budget (DSYNC mismatch / EOF), and damage inside the truncated
/// last subsubframe hits the ceil(PSC/4) block-code tail.
const TERM: &[u8] = include_bytes!("fixtures/dts_term_5_frames.bin");

/// Drive the full decode surface over one (possibly damaged) buffer:
/// resync-tolerant framing, header parse, PCM decode, LFE plane, and
/// the §5.7 chunk parsers. Errors are fine; panics are the failure.
fn drive(bytes: &[u8], channels: usize) {
    let mut dec = CoreStreamDecoder::new(channels);
    for fv in iter_frames_resync(bytes) {
        let Ok(fv) = fv else { continue };
        // Frame-level decode: any typed error is acceptable.
        if dec.decode_frame(fv.data, &fv.header).is_ok() {
            let _ = dec.take_last_lfe_pcm();
        }
        // §5.7 chunk parsers walk the same damaged bytes.
        let _ = oxideav_dts::parse_aux_data(fv.data, &fv.header);
        let _ = oxideav_dts::parse_rev2_aux(fv.data, &fv.header);
    }
}

/// XOR a single byte at a fixed stride with two masks, on both
/// fixtures. Exhaustive-at-stride keeps the sweep CI-fast while still
/// hitting header, side-info, audio-data, and chunk bytes (each drive
/// re-decodes the whole multi-frame stream, so the per-case cost is
/// what bounds the stride).
#[test]
fn single_byte_corruption_never_panics() {
    for (fixture, channels) in [(STEREO, 2usize), (FIVE_ONE, 5), (JOINT, 2), (TERM, 2)] {
        for offset in (0..fixture.len()).step_by(37) {
            for mask in [0x80u8, 0xFF] {
                let mut damaged = fixture.to_vec();
                damaged[offset] ^= mask;
                drive(&damaged, channels);
            }
        }
    }
}

/// A deterministic xorshift-driven pass damaging several bytes at
/// once, plus truncations — the multi-error case single-byte sweeps
/// cannot reach.
#[test]
fn multi_byte_corruption_and_truncation_never_panic() {
    let mut state = 0x9E37_79B9_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for (fixture, channels) in [(STEREO, 2usize), (FIVE_ONE, 5), (JOINT, 2), (TERM, 2)] {
        for _ in 0..200 {
            let mut damaged = fixture.to_vec();
            let hits = 2 + (next() as usize % 6);
            for _ in 0..hits {
                let off = next() as usize % damaged.len();
                damaged[off] ^= (next() >> 24) as u8;
            }
            // Occasionally truncate as well.
            if next() % 3 == 0 {
                let keep = next() as usize % damaged.len();
                damaged.truncate(keep.max(1));
            }
            drive(&damaged, channels);
        }
    }
}
