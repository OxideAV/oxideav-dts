//! DTS Coherent Acoustics — §5.5 Primary Audio Data Arrays (`Audio
//! Data`) decode walk (ETSI TS 102 114 V1.3.1, Table 5-29, staged PDF
//! p.31-33).
//!
//! Round 340 (2026-06-19) composes the already-landed per-subband
//! primitives into the §5.5 `Audio Data` block: the per-subsubframe
//! nested loop that extracts the eight `AUDIO[m]` quantization indices
//! for every `(ch, n)` subband (dispatching on the round-258
//! [`AudioQuantType`] resolved from the `(ABITS, SEL)` pair), applies
//! the round-293 §5.5 `rScale · AUDIO[m]` transient-aware
//! dequantization, runs the round-228 §C.2.2 inverse-ADPCM predictor
//! where `PMODE != 0`, and consumes the §5.5 `DSYNC` trailers — all the
//! way to the per-channel subband-sample matrix
//! `aPrmCh[ch].aSubband[n].aSample[m]` the §C.2.5 QMF synthesis
//! consumes.
//!
//! The Table 5-29 `Audio Data` pseudocode (staged PDF p.31-32),
//! transcribed verbatim:
//!
//! ```text
//! for (nSubSubFrame=0; nSubSubFrame<nSSC; nSubSubFrame++) {
//!   for (ch=0; ch<nPCHS; ch++)
//!     for (n=0; n<nVQSUB[ch]; n++) {       // Not high-frequency VQ
//!       nABITS = ABITS[ch][n];
//!       nNumQ  = pCQGroupAUDIO[nABITS-1].nNumQ-1;
//!       nSEL   = SEL[ch][nABITS-1];
//!       nQType = 1;                         // Huffman by default
//!       if (nSEL == nNumQ) { nQType = (nABITS<=7) ? 3 : 2; }
//!       if (nABITS == 0)    nQType = 0;
//!       switch (nQType) {
//!         case 0: AUDIO[0..8] = 0;
//!         case 1: AUDIO[m] = Huffman(SEL);                  // ×8
//!         case 2: AUDIO[m] = SignExtension(Binary(width));  // ×8
//!         case 3: for (nBlock=0;nBlock<2;nBlock++)          // 2×4
//!                   BlockCode(nCode) -> AUDIO[m..m+4];
//!       }
//!       // dequant: rScale = rStepSize·SCALES[ch][n][transient];
//!       //          rScale *= arADJ[ch][SEL[ch][nABITS-1]];
//!       nSample = 8*nSubSubFrame;
//!       aSample[nSample+m] = rScale * AUDIO[m];             // m<8
//!       if (PMODE[ch][n] != 0) InverseADPCM();
//!     }
//!     if ((nSubSubFrame==nSSC-1) || (ASPF==1)) {
//!       DSYNC = ExtractBits(16);
//!       if (DSYNC != 0xffff) "DSYNC error";
//!     }
//! }
//! ```
//!
//! # Scope and blockers
//!
//! Two §5.5 sub-paths require Annex D VQ code books that are **not yet
//! transcribed** into `docs/audio/dts/` and so are surfaced as typed
//! "blocked" errors rather than guessed:
//!
//! * The **high-frequency VQ subbands** loop (`n ∈ [nVQSUB, nSUBS)`,
//!   `nVQIndex = ExtractBits(10); HFreqVQ.LookUp(...)`) needs the §D.10.2
//!   "High Frequency Subbands" 32-sample VQ code book. A subband whose
//!   `nVQSUB < nSUBS` cannot be reconstructed without it.
//! * The **inverse-ADPCM coefficient lookup** (`PMODE != 0`, the §5.4.1
//!   `ADPCMCoeffVQ.LookUp(nVQIndex, PVQ[ch][n])`) needs the §D.10.1
//!   ADPCM-coefficient VQ code book to turn the captured 12-bit
//!   `pvq_index` into the four predictor taps; the §C.2.2 predictor
//!   itself is landed but cannot run without the coefficients.
//!
//! Both surface [`AudioArrayError::VqCodebookUnavailable`]. A frame
//! whose primary channels are all linearly / Huffman / block coded with
//! `PMODE == 0` and `nVQSUB == nSUBS` (the common Core case) decodes to
//! PCM end-to-end with the landed primitives.

use crate::audio_data::{audio_quant_type, AudioQuantType};
use crate::audio_huff::{decode_audio_huff_at, AudioHuffCodebook};
use crate::bitreader::BitReader;
use crate::block_code::decode_block_code;
use crate::cos_mod::NUM_SUBBAND;
use crate::dsync::DSYNC_WORD;
use crate::side_info::ScaleFactorAdjustment;
use crate::step_size::{transient_scale_index, StepSizeTable, SAMPLES_PER_SUBSUBFRAME};
use crate::subframe::ChannelSideInfo;
use crate::{Error, Result};

/// Errors specific to the §5.5 audio-data array walk that are not
/// already covered by the crate-level [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioArrayError {
    /// A subband required an Annex D VQ code book that is not yet
    /// transcribed into `docs/audio/dts/`: either the §D.10.2 high-
    /// frequency VQ book (a `nVQSUB < nSUBS` subband) or the §D.10.1
    /// ADPCM-coefficient VQ book (a `PMODE != 0` subband). Carries the
    /// channel/subband that hit the blocker and which book is missing.
    VqCodebookUnavailable {
        /// 0-based channel index.
        ch: usize,
        /// 0-based subband index.
        n: usize,
        /// `true` = high-frequency VQ (§D.10.2); `false` = ADPCM
        /// coefficient VQ (§D.10.1).
        high_frequency_vq: bool,
    },
    /// The §5.5 LFE phase (§2.2) dequant failed — a reserved §D.1.2
    /// `RMS_7BIT` scale index or an absent LFE channel
    /// ([`crate::LfeChannelError`]).
    LfePhase(crate::LfeChannelError),
}

impl core::fmt::Display for AudioArrayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AudioArrayError::VqCodebookUnavailable {
                ch,
                n,
                high_frequency_vq,
            } => {
                let book = if *high_frequency_vq {
                    "§D.10.2 high-frequency VQ"
                } else {
                    "§D.10.1 ADPCM-coefficient VQ"
                };
                write!(
                    f,
                    "oxideav-dts: channel {ch} subband {n} needs the {book} code \
                     book, which is not yet transcribed into docs/audio/dts/"
                )
            }
            AudioArrayError::LfePhase(e) => write!(f, "oxideav-dts: §5.5 LFE phase: {e}"),
        }
    }
}

impl std::error::Error for AudioArrayError {}

/// The §D.6 `V…` block-code-book word width (in bits) for the `ABITS`
/// family `1..=7`, read off the §D.6 table titles (staged PDF
/// p.231-236): `V3` 7-bit, `V5` 10-bit, `V7` 12-bit, `V9` 13-bit,
/// `V13` 15-bit, `V17` 17-bit, `V25` 19-bit. Each block-code word
/// expands to four samples.
fn block_code_word_bits(abits: u8) -> Option<u32> {
    Some(match abits {
        1 => 7,
        2 => 10,
        3 => 12,
        4 => 13,
        5 => 15,
        6 => 17,
        7 => 19,
        _ => return None,
    })
}

/// The §5.5 "No Further Encoding" (NFE) binary-code word width (in
/// bits) for an `ABITS` index, sign-extended on read. Table 5-26's
/// even "or 2ⁿ" level forms (PDF p.27) give `2^(ABITS-3)` levels for
/// `ABITS ∈ 8..=26` (e.g. ABITS 8 → 32 = 2⁵, ABITS 26 → 2²³), so the
/// binary code carries `ABITS - 3` bits. For `ABITS > 26` (the
/// no-SEL-transmitted region) the same `ABITS - 3` width holds up to
/// the 32-bit reader bound.
fn nfe_word_bits(abits: u8) -> Option<u32> {
    if abits < 8 {
        return None;
    }
    let bits = u32::from(abits) - 3;
    if (1..=32).contains(&bits) {
        Some(bits)
    } else {
        None
    }
}

/// Sign-extend a `width`-bit two's-complement field read as an
/// unsigned integer (`pCQGroup->ppQ[nSEL]->SignExtension(nCode)`).
fn sign_extend(value: u32, width: u32) -> i32 {
    debug_assert!((1..=32).contains(&width));
    let shift = 32 - width;
    ((value << shift) as i32) >> shift
}

/// Extract one subband's eight `AUDIO[m]` quantization indices for one
/// subsubframe from `br`, dispatching on the `(abits, sel)` pair per
/// the §5.5 Table 5-29 `switch (nQType)`.
///
/// * [`AudioQuantType::NoBits`] — eight zeros, no bits read.
/// * [`AudioQuantType::Huffman`] — eight §D.5 Huffman-coded indices.
/// * [`AudioQuantType::NoEncoding`] — eight sign-extended binary-code
///   fields of [`nfe_word_bits`] width.
/// * [`AudioQuantType::BlockCode`] — two [`block_code_word_bits`]-wide
///   block-code words, each expanded to four samples.
fn extract_subband_audio(
    br: &mut BitReader<'_>,
    abits: u8,
    sel: u8,
) -> Result<[i32; SAMPLES_PER_SUBSUBFRAME]> {
    let mut audio = [0_i32; SAMPLES_PER_SUBSUBFRAME];
    match audio_quant_type(abits, sel) {
        AudioQuantType::NoBits => {}
        AudioQuantType::Huffman => {
            // SEL selects the §D.5 book within the ABITS group.
            let codebook = AudioHuffCodebook::from_abits_sel(abits, sel)
                .ok_or(Error::HuffmanDecodeFailed { table: "AUDIO" })?;
            for slot in audio.iter_mut() {
                let level = decode_audio_huff_in(br, codebook)?;
                *slot = i32::from(level);
            }
        }
        AudioQuantType::NoEncoding => {
            let width = nfe_word_bits(abits).ok_or(Error::InvalidStepSize { abits })?;
            for slot in audio.iter_mut() {
                let raw = br.read_bits(width)?;
                *slot = sign_extend(raw, width);
            }
        }
        AudioQuantType::BlockCode => {
            let width = block_code_word_bits(abits).ok_or(Error::InvalidStepSize { abits })?;
            let n_levels = u32::from(crate::audio_data::QUANT_LEVELS[abits as usize]);
            let mut m = 0usize;
            for _ in 0..2 {
                let code = br.read_bits(width)?;
                decode_block_code(code, n_levels, &mut audio[m..m + 4])?;
                m += 4;
            }
        }
    }
    Ok(audio)
}

/// Decode one §D.5 Huffman `AUDIO[m]` index through a `BitReader`
/// already positioned mid-stream (the [`decode_audio_huff_at`]
/// byte-offset entry point re-seeks from a byte boundary, which the
/// per-subsubframe walk cannot do because it shares one running
/// reader). This re-walks the book bit-at-a-time from `br`.
fn decode_audio_huff_in(br: &mut BitReader<'_>, codebook: AudioHuffCodebook) -> Result<i16> {
    // Bridge through the byte-offset API by re-reading from the
    // current absolute bit position over the same backing buffer.
    // `decode_audio_huff_at` borrows the buffer immutably and reports
    // bits_consumed; we then advance `br` by that many bits.
    let pos = br.absolute_bit_position();
    let bytes = br.backing_bytes();
    let (level, consumed) = decode_audio_huff_at(bytes, pos, codebook)?;
    br.skip_bits(consumed as u32)?;
    Ok(level)
}

/// Per-channel decoded subband-sample matrix for one subframe: row `s`
/// (`s ∈ 0..n_ssc*8`) is the §C.2.5 per-sample subband vector
/// `[aSubband[0].aSample[s], …, aSubband[31].aSample[s]]` for one
/// channel. The QMF synthesis consumes this directly.
pub type SubbandSampleMatrix = Vec<[f64; NUM_SUBBAND]>;

/// Decode the §5.5 LFE phase (the `if (LFF > 0) { … }` block of the
/// `docs/audio/dts/dts-lfe-interpolation-and-audio-walker.md` §2.2
/// walker) for one subframe, returning the interpolated LFE PCM and the
/// number of bits consumed.
///
/// The LFE phase sits between the high-frequency-VQ phase (§2.1, empty
/// for the accepted Core case where `nVQSUB == nSUBS`) and the
/// per-subsubframe audio-data phase (§2.3). It reads `2·LFF·nSSC` 8-bit
/// two's-complement decimated LFE samples followed by an 8-bit
/// `LFEscaleIndex`, dequantises (`rLFE[n] = LFE[n]·nScale·0.035` with the
/// §D.1.2 `RMS_7BIT` scale), then upsamples via the §C.2.6
/// `InterpolationFIR(LFF)` polyphase convolution ([`crate::LfeChannel`]).
///
/// * `bytes` / `bit_offset` — positioned at the first LFE-phase bit.
/// * `lff` — the frame header's non-zero `LFF` (1 → 128×, 2 → 64×).
/// * `n_ssc` — the subframe's subsubframe count (`SSC + 1`).
/// * `lfe` — the persistent per-channel [`crate::LfeChannel`] whose
///   §C.2.6 history carries across subframes.
///
/// Returns `(lfe_pcm, bits_consumed)`. The PCM length is
/// `2·LFF·nSSC·(64 | 128)`.
///
/// # Errors
///
/// * [`Error::UnexpectedEof`] on a truncated LFE region;
/// * [`AudioArrayError::LfePhase`] wrapping a [`crate::LfeChannelError`]
///   (a reserved §D.1.2 scale index, or `lff == 0`).
pub fn decode_lfe_phase_at(
    bytes: &[u8],
    bit_offset: usize,
    lff: u8,
    n_ssc: usize,
    lfe: &mut crate::LfeChannel,
) -> core::result::Result<(Vec<i32>, usize), AudioArrayDecodeError> {
    let byte_offset = bit_offset / 8;
    let intra_byte = bit_offset % 8;
    let mut br = BitReader::from_byte_offset(bytes, byte_offset);
    if intra_byte > 0 {
        br.read_bits(intra_byte as u32)?;
    }

    // 2·LFF·nSSC 8-bit two's-complement decimated LFE samples.
    let n_lfe = 2 * (lff as usize) * n_ssc;
    let mut samples: Vec<i8> = Vec::with_capacity(n_lfe);
    for _ in 0..n_lfe {
        // ExtractBits(8) read as a signed char.
        samples.push(br.read_bits(8)? as u8 as i8);
    }

    // 8-bit LFEscaleIndex.
    let scale_index = br.read_bits(8)? as u8;

    let bits_consumed = br.absolute_bit_position() - bit_offset;

    let pcm = lfe
        .decode_subframe(&samples, scale_index, lff)
        .map_err(AudioArrayError::LfePhase)?;

    Ok((pcm, bits_consumed))
}

/// Decode the §5.5 `Audio Data` block for one subframe, given the
/// already-decoded §5.4.1 side information and §5.3.2 header context.
///
/// Walks the Table 5-29 `nSubSubFrame × ch × n` loop, extracting and
/// dequantizing every primary subband, running inverse-ADPCM where
/// `PMODE != 0`, and consuming the `DSYNC` trailers. Returns one
/// [`SubbandSampleMatrix`] per channel (length `n_ssc * 8` rows).
///
/// * `bytes` / `bit_offset` — the bit stream positioned at the first
///   §5.5 `Audio Data` bit (after the §5.4.1 side-info block).
/// * `side` — the per-channel [`ChannelSideInfo`] (round-281).
/// * `sel` — `|ch, abits| -> u8`, the §5.3.2 `SEL[ch][nABITS-1]`
///   selector ([`crate::AudioCodingHeader::sel`]).
/// * `adj` — `|ch, abits| -> ScaleFactorAdjustment`, the §5.5
///   `arADJ[ch][SEL[ch][nABITS-1]]` multiplier
///   ([`crate::AudioCodingHeader::adj`]).
/// * `n_vqsub` / `n_subs` — per-channel loop bounds.
/// * `n_ssc` — the subframe's subsubframe count (`SSC + 1`).
/// * `table` — the §5.5 `RATE`-selected step-size table.
/// * `aspf` — the §5.3.1 Audio Sync-Word Insertion Flag (a `DSYNC`
///   trailer follows every subsubframe when set, else only the last).
///
/// Returns `(Vec<SubbandSampleMatrix>, bits_consumed)`.
///
/// # Errors
///
/// * [`Error::InvalidStepSize`] for an out-of-range `ABITS`;
/// * [`Error::HuffmanDecodeFailed`] on a corrupt audio Huffman prefix
///   or an `(ABITS, SEL)` pair with no §D.5 book;
/// * [`Error::DsyncMismatch`] when a `DSYNC` trailer is not `0xffff`;
/// * [`Error::UnexpectedEof`] on a truncated array.
///
/// VQ / ADPCM-coefficient blockers surface
/// [`AudioArrayError::VqCodebookUnavailable`] wrapped through the
/// [`AudioArrayDecodeError`] return type.
#[allow(clippy::too_many_arguments)]
pub fn decode_audio_data_subframe_at(
    bytes: &[u8],
    bit_offset: usize,
    side: &[ChannelSideInfo],
    sel: impl Fn(usize, u8) -> u8,
    adj: impl Fn(usize, u8) -> ScaleFactorAdjustment,
    n_vqsub: &[usize],
    n_subs: &[usize],
    n_ssc: usize,
    table: StepSizeTable,
    aspf: bool,
) -> core::result::Result<(Vec<SubbandSampleMatrix>, usize), AudioArrayDecodeError> {
    let n_pchs = side.len();

    // Reject the VQ / ADPCM blockers up front so a partially-decoded
    // matrix is never returned.
    for (ch, ch_side) in side.iter().enumerate() {
        if n_vqsub[ch] < n_subs[ch] {
            return Err(AudioArrayError::VqCodebookUnavailable {
                ch,
                n: n_vqsub[ch],
                high_frequency_vq: true,
            }
            .into());
        }
        if let Some(n) = ch_side.pmode[..n_vqsub[ch]].iter().position(|&p| p != 0) {
            return Err(AudioArrayError::VqCodebookUnavailable {
                ch,
                n,
                high_frequency_vq: false,
            }
            .into());
        }
    }

    let rows = n_ssc * SAMPLES_PER_SUBSUBFRAME;
    let mut matrices: Vec<SubbandSampleMatrix> = vec![vec![[0.0_f64; NUM_SUBBAND]; rows]; n_pchs];

    let byte_offset = bit_offset / 8;
    let intra_byte = bit_offset % 8;
    let mut br = BitReader::from_byte_offset(bytes, byte_offset);
    if intra_byte > 0 {
        br.read_bits(intra_byte as u32)?;
    }

    for subsubframe in 0..n_ssc {
        let base = subsubframe * SAMPLES_PER_SUBSUBFRAME;
        for (ch, ch_side) in side.iter().enumerate() {
            let matrix = &mut matrices[ch];
            // `n` is the subband index, used to address ch_side.abits /
            // tmode / scales and matrix[row][n]; an enumerate() over any
            // single one would not capture the cross-array indexing.
            #[allow(clippy::needless_range_loop)]
            for n in 0..n_vqsub[ch] {
                let abits = ch_side.abits[n];
                let sel_val = sel(ch, abits);
                let audio = extract_subband_audio(&mut br, abits, sel_val)?;

                // §5.5 transient-aware rScale composition.
                let scale_idx = transient_scale_index(ch_side.tmode[n], n_ssc, subsubframe);
                let scale = ch_side.scales[n][scale_idx];
                let step = table.step_size(abits)?;
                let r_scale = step * f64::from(scale) * adj(ch, abits).multiplier_f64();

                for (m, &index) in audio.iter().enumerate() {
                    matrix[base + m][n] = r_scale * f64::from(index);
                }
                // PMODE != 0 subbands were already rejected above, so no
                // inverse-ADPCM runs here in this round's scope.
            }
        }
        // DSYNC trailer: present after the last subsubframe always, and
        // after every subsubframe when ASPF == 1.
        if subsubframe == n_ssc - 1 || aspf {
            let dsync = br.read_bits(16)? as u16;
            if dsync != DSYNC_WORD {
                return Err(Error::DsyncMismatch {
                    found: dsync,
                    n_subsubframe: subsubframe as u8,
                }
                .into());
            }
        }
    }

    let bits_consumed = br.absolute_bit_position() - bit_offset;
    Ok((matrices, bits_consumed))
}

/// Composite error for the §5.5 audio-data walk: either a crate-level
/// bit-stream [`Error`] or an [`AudioArrayError`] VQ/ADPCM blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioArrayDecodeError {
    /// A bit-stream-level decode error (EOF, bad Huffman prefix,
    /// invalid step size, DSYNC mismatch, …).
    Bitstream(Error),
    /// A subband needed an Annex D VQ code book not yet in `docs/`.
    Blocked(AudioArrayError),
}

impl From<Error> for AudioArrayDecodeError {
    fn from(e: Error) -> Self {
        AudioArrayDecodeError::Bitstream(e)
    }
}

impl From<AudioArrayError> for AudioArrayDecodeError {
    fn from(e: AudioArrayError) -> Self {
        AudioArrayDecodeError::Blocked(e)
    }
}

impl core::fmt::Display for AudioArrayDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AudioArrayDecodeError::Bitstream(e) => write!(f, "{e}"),
            AudioArrayDecodeError::Blocked(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AudioArrayDecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn sign_extend_round_trips() {
        assert_eq!(sign_extend(0b011, 3), 3);
        assert_eq!(sign_extend(0b111, 3), -1);
        assert_eq!(sign_extend(0b100, 3), -4);
        assert_eq!(sign_extend(0, 5), 0);
    }

    #[test]
    fn nfe_and_block_widths() {
        assert_eq!(nfe_word_bits(8), Some(5)); // 32 levels
        assert_eq!(nfe_word_bits(11), Some(8)); // 256 levels
        assert_eq!(nfe_word_bits(26), Some(23));
        assert_eq!(nfe_word_bits(7), None);
        assert_eq!(block_code_word_bits(1), Some(7)); // V3
        assert_eq!(block_code_word_bits(7), Some(19)); // V25
        assert_eq!(block_code_word_bits(8), None);
    }

    /// A single-channel, single-subsubframe, no-bits subband stream
    /// decodes to an all-zero matrix and a single DSYNC trailer.
    #[test]
    fn no_bits_subband_zeroes_matrix() {
        // nSSC = 1, one channel, nVQSUB = nSUBS = 1, ABITS = 0.
        let side = vec![ChannelSideInfo::cleared()];
        let stream = pack_fields(&[(0xffff, 16)]); // just the DSYNC
        let (mats, bits) = decode_audio_data_subframe_at(
            &stream,
            0,
            &side,
            |_, _| 0,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            1,
            StepSizeTable::Lossy,
            false,
        )
        .unwrap();
        assert_eq!(mats.len(), 1);
        assert_eq!(mats[0].len(), 8);
        assert!(mats[0].iter().all(|row| row.iter().all(|&v| v == 0.0)));
        assert_eq!(bits, 16);
    }

    /// A NoEncoding (NFE) subband with ABITS 8 reads eight 5-bit
    /// sign-extended fields and scales them by the dequant rScale.
    #[test]
    fn nfe_subband_dequantizes() {
        let mut ch = ChannelSideInfo::cleared();
        ch.abits[0] = 8; // NFE width 5; lossy step for 8 = 796918/2^22
        ch.scales[0][0] = 4;
        let side = vec![ch];

        // Eight 5-bit values: 1,-1,2,-2,3,-3,4,-4 (two's complement).
        let vals = [1i32, -1, 2, -2, 3, -3, 4, -4];
        let mut fields: Vec<(u32, u8)> = vals.iter().map(|&v| ((v as u32) & 0x1f, 5u8)).collect();
        fields.push((0xffff, 16)); // DSYNC
        let stream = pack_fields(&fields);

        // SEL must select the terminal NFE entry for ABITS 8 (group of
        // 8 -> top SEL 7).
        let (mats, _) = decode_audio_data_subframe_at(
            &stream,
            0,
            &side,
            |_, _| 7,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            1,
            StepSizeTable::Lossy,
            false,
        )
        .unwrap();
        let step = StepSizeTable::Lossy.step_size(8).unwrap();
        let r = step * 4.0;
        for (m, &v) in vals.iter().enumerate() {
            assert!((mats[0][m][0] - r * f64::from(v)).abs() < 1e-9);
        }
    }

    /// A bad DSYNC surfaces a typed mismatch.
    #[test]
    fn bad_dsync_rejected() {
        let side = vec![ChannelSideInfo::cleared()];
        let stream = pack_fields(&[(0x1234, 16)]);
        let err = decode_audio_data_subframe_at(
            &stream,
            0,
            &side,
            |_, _| 0,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            1,
            StepSizeTable::Lossy,
            false,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AudioArrayDecodeError::Bitstream(Error::DsyncMismatch { found: 0x1234, .. })
        ));
    }

    /// A subband with high-frequency VQ (nVQSUB < nSUBS) is blocked.
    #[test]
    fn high_frequency_vq_blocked() {
        let side = vec![ChannelSideInfo::cleared()];
        let err = decode_audio_data_subframe_at(
            &[0u8; 8],
            0,
            &side,
            |_, _| 0,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1], // nVQSUB
            &[3], // nSUBS > nVQSUB -> VQ subbands
            1,
            StepSizeTable::Lossy,
            false,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AudioArrayDecodeError::Blocked(AudioArrayError::VqCodebookUnavailable {
                high_frequency_vq: true,
                ..
            })
        ));
    }

    /// A PMODE-active subband is blocked on the §D.10.1 coefficient VQ.
    #[test]
    fn adpcm_subband_blocked() {
        let mut ch = ChannelSideInfo::cleared();
        ch.pmode[0] = 1;
        let side = vec![ch];
        let err = decode_audio_data_subframe_at(
            &[0u8; 8],
            0,
            &side,
            |_, _| 0,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            1,
            StepSizeTable::Lossy,
            false,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AudioArrayDecodeError::Blocked(AudioArrayError::VqCodebookUnavailable {
                high_frequency_vq: false,
                ..
            })
        ));
    }

    /// ASPF == 1 inserts a DSYNC after every subsubframe; two
    /// subsubframes therefore carry two trailers.
    #[test]
    fn aspf_inserts_dsync_each_subsubframe() {
        let side = vec![ChannelSideInfo::cleared()];
        // nSSC = 2, ABITS 0 -> no audio bits, two DSYNC trailers.
        let stream = pack_fields(&[(0xffff, 16), (0xffff, 16)]);
        let (_, bits) = decode_audio_data_subframe_at(
            &stream,
            0,
            &side,
            |_, _| 0,
            |_, _| ScaleFactorAdjustment::Adj0,
            &[1],
            &[1],
            2,
            StepSizeTable::Lossy,
            true,
        )
        .unwrap();
        assert_eq!(bits, 32);
    }

    // -----------------------------------------------------------
    // §5.5 LFE phase walker (§2.2).
    // -----------------------------------------------------------

    /// The LFE phase consumes `2·LFF·nSSC` 8-bit samples + an 8-bit
    /// scale index, and reports exactly that many bits.
    #[test]
    fn lfe_phase_consumes_samples_plus_scale_index() {
        let lff = 1u8; // 128×
        let n_ssc = 2usize;
        let n_lfe = 2 * (lff as usize) * n_ssc; // 4 samples
                                                // 4 sample bytes (all 0) + 1 scale-index byte (10).
        let mut fields: Vec<(u32, u8)> = vec![(0, 8); n_lfe];
        fields.push((10, 8));
        let stream = pack_fields(&fields);
        let mut lfe = crate::LfeChannel::new();
        let (pcm, bits) = decode_lfe_phase_at(&stream, 0, lff, n_ssc, &mut lfe).unwrap();
        assert_eq!(bits, (n_lfe + 1) * 8);
        // Each decimated sample expands to 128 PCM samples.
        assert_eq!(pcm.len(), n_lfe * 128);
        // All-zero LFE samples decode to silence.
        assert!(pcm.iter().all(|&s| s == 0));
    }

    /// 8-bit two's-complement LFE samples are read as signed: a 0xFF byte
    /// is -1, which (with a non-zero scale) produces non-zero PCM of the
    /// correct sign at phase 0.
    #[test]
    fn lfe_phase_reads_signed_samples() {
        let lff = 2u8; // 64×
        let n_ssc = 1usize;
        let n_lfe = 2 * (lff as usize) * n_ssc; // 4 samples
        let scale_index = 60u8;
        // First sample = 0xFF (= -1), rest 0.
        let mut fields: Vec<(u32, u8)> = vec![(0xFF, 8)];
        fields.extend(vec![(0, 8); n_lfe - 1]);
        fields.push((u32::from(scale_index), 8));
        let stream = pack_fields(&fields);

        let mut lfe = crate::LfeChannel::new();
        let (pcm, _) = decode_lfe_phase_at(&stream, 0, lff, n_ssc, &mut lfe).unwrap();

        // Reference: phase-0 first output = (int)((-1)·nScale·0.035·c0).
        let n_scale = crate::side_info::RMS_7BIT[scale_index as usize] as f64;
        let r_scale = n_scale * crate::LFE_SCALE_STEP;
        let sel = crate::LfeInterpolationSelection::Decimation64;
        let c0 = sel.coefficients()[0];
        let expected0 = (-(r_scale * c0)) as i32;
        assert_eq!(pcm[0], expected0);
    }

    /// A reserved §D.1.2 scale index surfaces the typed LFE-phase blocker.
    #[test]
    fn lfe_phase_rejects_reserved_scale_index() {
        let lff = 1u8;
        let n_ssc = 1usize;
        let n_lfe = 2 * (lff as usize) * n_ssc;
        let mut fields: Vec<(u32, u8)> = vec![(0, 8); n_lfe];
        fields.push((126, 8)); // reserved
        let stream = pack_fields(&fields);
        let mut lfe = crate::LfeChannel::new();
        let err = decode_lfe_phase_at(&stream, 0, lff, n_ssc, &mut lfe).unwrap_err();
        assert!(matches!(
            err,
            AudioArrayDecodeError::Blocked(AudioArrayError::LfePhase(
                crate::LfeChannelError::ReservedScaleIndex { index: 126 }
            ))
        ));
    }
}
