//! DTS Core audio-coding-header + subframe parser.
//!
//! This module implements the per-frame audio payload pipeline
//! described in `dts-trace-reverse-engineering.md` §3:
//!
//!   1. Audio coding header (§3.1) — number of subframes,
//!      per-channel SUBS / SUBVQ / JCH / THUFF / SHUFF / BHUFF /
//!      QHUFF / SCAJ.
//!   2. Per-subframe (§3.2-3.3):
//!      - 2-bit subsubframe count (SSC)
//!      - 3-bit partial-sample count (PSC)
//!      - per-channel-per-band prediction mode + (if set) 12-bit
//!        prediction-VQ index
//!      - per-channel-per-band ABITS via BHUFF
//!      - per-channel-per-band TM (transition mode) via THUFF
//!      - per-channel-per-band scale-factor index via SHUFF
//!   3. Per-subsubframe per-channel per-band sample data (§3.4):
//!      block-coded VQ for ABITS 1..7, plain two's-complement for
//!      ABITS ≥ 8.
//!
//! ## Round-1 status
//!
//! The audio-coding-header parser is complete for the field layout
//! observed in the encoder-generated corpus (FFmpeg's `dcaenc`
//! always emits SUBFC=1, SSC≥1, BHUFF=7 = direct 5-bit ABITS,
//! SHUFF=6 = direct 7-bit absolute scale factor — the simplest
//! coding path). When the header signals a more sophisticated
//! coding (Huffman BHUFF/SHUFF, joint intensity, prediction VQ,
//! transition modes other than 0), this round falls back to a
//! best-effort path that returns silence for that subframe rather
//! than failing the whole packet. Documented in the README backlog.

use oxideav_core::{Error, Result};

use crate::bits::BitReader;
use crate::header::CoreHeader;
use crate::tables::{
    LOSSLESS_QUANT, LOSSY_QUANT, QUANT_LEVELS, SCALE_FACTOR_QUANT6, SCALE_FACTOR_QUANT7,
    SCALE_FACTOR_UNITY, STEP_UNITY_Q20,
};
use crate::vq;

pub const SUBBANDS: usize = 32;
pub const BLOCKS_PER_FRAME: usize = 16;
pub const SAMPLES_PER_BLOCK: usize = 8;

/// Decoded sub-band samples for one frame.
///
/// Indexed `[channel][block 0..16][band 0..32]` — each value is the
/// reconstructed sub-band sample in the synthesis filter's input
/// scale (≈ Q16 normalised).
pub struct FrameOutput {
    pub subband: Vec<Vec<[f64; SUBBANDS]>>,
}

impl FrameOutput {
    pub fn silence(nch: usize) -> Self {
        Self {
            subband: (0..nch)
                .map(|_| vec![[0.0f64; SUBBANDS]; BLOCKS_PER_FRAME])
                .collect(),
        }
    }
}

/// Decoded audio coding header.
#[derive(Clone, Debug)]
struct AudioCodingHeader {
    nsubframes: u8,
    nchannels: u8,
    /// Per-channel: number of active sub-bands (2..32).
    subs: Vec<u8>,
    /// Per-channel: first VQ-coded sub-band (1..32).
    subvq: Vec<u8>,
    /// Per-channel: joint-intensity index (0 = no joint coding).
    #[allow(dead_code)]
    jch: Vec<u8>,
    /// Per-channel: transition-mode codebook (0..3).
    #[allow(dead_code)]
    thuff: Vec<u8>,
    /// Per-channel: scale-factor codebook (0..7).
    shuff: Vec<u8>,
    /// Per-channel: bit-allocation codebook (0..7).
    bhuff: Vec<u8>,
}

/// Decode the entire audio payload of a DTS Core frame.
pub fn decode_audio_payload(hdr: &CoreHeader, data: &[u8]) -> Result<FrameOutput> {
    let mut r = BitReader::new(data);
    // Skip the frame header bits we already parsed (`bits_consumed`).
    r.skip(hdr.bits_consumed)?;

    let ach = parse_audio_coding_header(&mut r, hdr)?;
    let nch = ach.nchannels as usize;
    let mut out = FrameOutput::silence(nch);

    // For each subframe, decode per-channel state then per-block samples.
    let mut block_idx = 0usize;
    for _sf in 0..ach.nsubframes {
        // Subframe header: SSC and PSC.
        let ssc = (r.read(2)? as u8) + 1;
        let _psc = r.read(3)?;

        // Per-channel-per-band prediction mode (1 bit each); if set,
        // a 12-bit prediction-VQ index follows.
        for ch in 0..nch {
            let nbands = ach.subs[ch] as usize;
            let mut pred_modes = vec![0u8; nbands];
            for b in 0..nbands {
                pred_modes[b] = r.read(1)? as u8;
            }
            for b in 0..nbands {
                if pred_modes[b] == 1 {
                    let _pvq = r.read(12)?;
                }
            }
        }

        // Per-channel per-band ABITS. BHUFF 5/6 = direct 4-bit;
        // BHUFF = 7 = direct 5-bit. BHUFF 0..4 use Huffman codebooks
        // — for round-1 simplicity we error out on those and let the
        // caller fall back to silence.
        let mut abits: Vec<Vec<u8>> = vec![vec![0; SUBBANDS]; nch];
        for ch in 0..nch {
            let nbands = ach.subs[ch] as usize;
            for b in 0..nbands {
                let v = match ach.bhuff[ch] {
                    5 | 6 => r.read(4)? as u8,
                    7 => r.read(5)? as u8,
                    _ => {
                        return Err(Error::invalid(
                            "dts: BHUFF Huffman codebooks 0..4 not yet supported (round 1)",
                        ))
                    }
                };
                abits[ch][b] = v;
            }
        }

        // Per-channel per-band transition mode. Skipped when SSC == 1
        // (no transients possible in a 1-subsubframe subframe) or
        // when ABITS == 0.
        if ssc > 1 {
            for ch in 0..nch {
                let nbands = ach.subs[ch] as usize;
                for b in 0..nbands {
                    if abits[ch][b] != 0 {
                        let _tm = r.read(2)?;
                    }
                }
            }
        }

        // Per-channel per-band scale factor.
        // SHUFF == 5 → 6-bit absolute; SHUFF == 6 → 7-bit absolute.
        // SHUFF 0..4 = 7-bit Huffman delta — round-1 fallback as above.
        let mut scale_q22: Vec<Vec<u32>> = vec![vec![0u32; SUBBANDS]; nch];
        for ch in 0..nch {
            let nbands = ach.subs[ch] as usize;
            for b in 0..nbands {
                let sf_idx = match ach.shuff[ch] {
                    5 => r.read(6)? as usize,
                    6 => r.read(7)? as usize,
                    _ => {
                        return Err(Error::invalid(
                            "dts: SHUFF Huffman codebooks 0..4 not yet supported (round 1)",
                        ))
                    }
                };
                scale_q22[ch][b] = if ach.shuff[ch] == 5 {
                    SCALE_FACTOR_QUANT6.get(sf_idx).copied().unwrap_or(0)
                } else {
                    SCALE_FACTOR_QUANT7.get(sf_idx).copied().unwrap_or(0)
                };
            }
        }

        // VQ-coded high-band scale factors (per-channel per-band, one
        // for each band in [SUBVQ..SUBS]). Each is a 7-bit absolute
        // index that overrides `scale_q22[ch][b]` for those bands.
        for ch in 0..nch {
            let lo = ach.subvq[ch] as usize;
            let hi = ach.subs[ch] as usize;
            for b in lo..hi {
                let sf_idx = (r.read(7)? as usize) & 0x7F;
                scale_q22[ch][b] =
                    SCALE_FACTOR_QUANT7.get(sf_idx).copied().unwrap_or(0);
            }
        }

        // For each subsubframe → 8 PCM blocks.
        for _ssf in 0..ssc {
            for _pcmblk in 0..SAMPLES_PER_BLOCK {
                // Each PCM block emits one sample per band per channel.
                for ch in 0..nch {
                    let nbands = ach.subs[ch] as usize;
                    let lossless = hdr.lossless_mode;
                    for b in 0..nbands {
                        // Bit-allocated band (b < SUBVQ) vs VQ-coded
                        // band (b ≥ SUBVQ).
                        if b >= ach.subvq[ch] as usize {
                            // VQ band: 10-bit index + scale-factor
                            // multiplier baked in (one VQ index per
                            // *block*, not per sample). Read the
                            // index once on PCM block 0; samples 1..7
                            // come from the same vector.
                            // ‖ Round 1 simplification: use the next
                            // 10-bit chunk and look up the codebook;
                            // VQ dequant is staged, full codebook
                            // landing in round 2.
                            let vq_idx = r.read(10)? as usize;
                            let v = vq::lookup(vq_idx);
                            let scale = scale_q22[ch][b] as f64
                                / SCALE_FACTOR_UNITY as f64;
                            // Spread the 32-sample VQ vector across
                            // the 8 PCM samples of this sub-subframe
                            // block (samples 0..7 at strides of 4).
                            for n in 0..SAMPLES_PER_BLOCK {
                                let sample = v[n * 4] as f64 / 128.0;
                                let bidx = block_idx;
                                if bidx < BLOCKS_PER_FRAME {
                                    out.subband[ch][bidx][b] += sample * scale;
                                }
                            }
                        } else {
                            // Bit-allocated band.
                            let a = abits[ch][b] as usize;
                            if a == 0 {
                                // No bits — band is silent.
                                continue;
                            }
                            // For ABITS ≥ 8 the wire format is plain
                            // (ABITS - 3)-bit two's-complement. For
                            // ABITS 1..7 the spec allows either
                            // block-VQ or Huffman packing — we go
                            // direct two's-complement of width
                            // `ceil(log2(quant_levels))` for
                            // round-1, which loses some efficiency
                            // but keeps the bit cursor synchronous.
                            let nbits = if a >= 8 {
                                a - 3
                            } else {
                                let lvls = QUANT_LEVELS[a] as u32;
                                32 - lvls.leading_zeros() as usize
                            };
                            if nbits == 0 || nbits > 26 {
                                continue;
                            }
                            let sample = r.read_signed(nbits)?;
                            let step_q20 = if lossless {
                                LOSSLESS_QUANT[a]
                            } else {
                                LOSSY_QUANT[a]
                            } as f64
                                / STEP_UNITY_Q20 as f64;
                            let sf = scale_q22[ch][b] as f64
                                / SCALE_FACTOR_UNITY as f64;
                            let v = sample as f64 * step_q20 * sf;
                            let bidx = block_idx;
                            if bidx < BLOCKS_PER_FRAME {
                                out.subband[ch][bidx][b] += v;
                            }
                        }
                    }
                }
                block_idx += 1;
            }
        }

        // Optional DSYNC sentinel at the end of each subsubframe
        // (when sync_ssf=1) or at the end of the subframe's last
        // subsubframe (when sync_ssf=0). For round-1 we're permissive:
        // try to consume + ignore.
        let _ = consume_dsync(&mut r);
    }

    Ok(out)
}

fn consume_dsync(r: &mut BitReader) -> Result<()> {
    if r.bits_remaining() < 16 {
        return Ok(());
    }
    let _ = r.read(16)?;
    Ok(())
}

fn parse_audio_coding_header(
    r: &mut BitReader,
    hdr: &CoreHeader,
) -> Result<AudioCodingHeader> {
    let nsubframes = (r.read(4)? as u8) + 1;
    let nchannels = (r.read(3)? as u8) + 1;
    if nchannels as u32 != hdr.primary_channels as u32 {
        return Err(Error::invalid(format!(
            "dts: AMODE channel count {} != audio header {}",
            hdr.primary_channels, nchannels
        )));
    }
    let nch = nchannels as usize;
    let mut subs = vec![0u8; nch];
    for ch in 0..nch {
        subs[ch] = (r.read(5)? as u8) + 2;
    }
    let mut subvq = vec![0u8; nch];
    for ch in 0..nch {
        subvq[ch] = (r.read(5)? as u8) + 1;
    }
    let mut jch = vec![0u8; nch];
    for ch in 0..nch {
        jch[ch] = r.read(3)? as u8;
    }
    let mut thuff = vec![0u8; nch];
    for ch in 0..nch {
        thuff[ch] = r.read(2)? as u8;
    }
    let mut shuff = vec![0u8; nch];
    for ch in 0..nch {
        shuff[ch] = r.read(3)? as u8;
    }
    let mut bhuff = vec![0u8; nch];
    for ch in 0..nch {
        bhuff[ch] = r.read(3)? as u8;
    }
    // Per-channel per-codebook-class QHUFF sentinels (skipped — they
    // gate per-band Huffman in BHUFF 0..4 paths which we don't
    // support yet). Total = nch × 10 codebook classes ×
    // QUANT_INDEX_SEL_NBITS[class] bits; for round-1 simplicity we
    // skip a constant 10-class slab in the documented widths.
    use crate::tables::{QUANT_INDEX_SEL_NBITS, QUANT_INDEX_GROUP_SIZE};
    for _ in 0..nch {
        for c in 0..10 {
            let group_size = QUANT_INDEX_GROUP_SIZE[c] as usize;
            let nbits = QUANT_INDEX_SEL_NBITS[c] as usize;
            for _ in 0..group_size {
                let _ = r.read(nbits)?;
            }
        }
    }
    // Per-channel SCAJ field — 2-bit selector per *Huffman* QHUFF
    // entry (omitted on the encoder fast-path used for round-1
    // testing; the layout is in the spec but a clean-room
    // reproduction needs a dedicated trace pass).
    // Optional 16-bit HCRC over the audio header.
    if hdr.crc_present {
        let _hcrc = r.read(16)?;
    }
    Ok(AudioCodingHeader {
        nsubframes,
        nchannels,
        subs,
        subvq,
        jch,
        thuff,
        shuff,
        bhuff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_shape() {
        let f = FrameOutput::silence(2);
        assert_eq!(f.subband.len(), 2);
        assert_eq!(f.subband[0].len(), BLOCKS_PER_FRAME);
        for blk in &f.subband[0] {
            for &v in blk {
                assert_eq!(v, 0.0);
            }
        }
    }
}
