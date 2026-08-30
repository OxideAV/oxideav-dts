//! DTS Coherent Acoustics — 32-band cosine-modulated QMF **analysis**
//! bank (the encoder-side front end), built as the exact adjoint of
//! this crate's §C.2.5 `QMFInterpolation()` synthesis driver.
//!
//! ETSI TS 102 114 V1.3.1 specifies the decoder only: §6.1.1 states
//! that the core PCM "is processed in a 32-band analysis cosine-
//! modulated filter bank (QMF)" producing the core subband samples,
//! and §C.2.5 gives the normative-by-example synthesis
//! (`PreCalCosMod()` + `QMFInterpolation()`) whose 512-tap §D.8
//! prototypes come in a *Perfect Reconstruction* (`FILTS=1`) and a
//! *Non-Perfect Reconstruction* (`FILTS=0`) flavour (Table 5-15). A
//! cosine-modulated PR bank is paraunitary: its analysis bank is the
//! time-reversed (adjoint) synthesis bank up to one scalar gain. This
//! module therefore derives the analysis bank **mechanically from the
//! synthesis structure the decoder already implements**, so the pair
//! is perfect-reconstruction by construction against
//! [`crate::QmfSynthesis`] (pinned in the tests below to > 90 dB SNR
//! for the `FILTS=1` prototype).
//!
//! # Derivation
//!
//! Writing one subband row as `s_r` (32 subband samples at subband
//! time `r`), the §C.2.5 driver computes `X_r = M·s_r` (the cosine-
//! modulation stage, [`crate::cos_mod_stage`]), keeps the 16 most
//! recent `X` rows in `raX`, and emits the PCM row `out_r` as
//!
//! ```text
//! out_r[i] = Σ_t p[i+64t]     ( X_{r-2t}[i]   − X_{r-2t}[31−i]   )
//!          + Σ_t p[32+i+64t]  ( −X_{r-1-2t}[i] − X_{r-1-2t}[31−i] )     t = 0..8
//! ```
//!
//! (the second sum is the `raZ[32+i]` partial carried into the next
//! row by `shift_z_output`). The FIR half of that map is (up to one
//! scalar) paraunitary for the §D.8 prototypes, so its inverse is its
//! adjoint (time reversal). The modulation half is **not** orthogonal
//! — the §C.2.5 `PreCalCosMod()` Block-3/Block-4 vectors put
//! per-index scalings `±0.25/(2·cos|sin((2k+1)π/128))` on `X` — so
//! the analysis inverts `M` exactly rather than transposing it:
//!
//! ```text
//! G_q[i]    = Σ_t p[i+64t]·x_{q+2t}[i]    − Σ_t p[32+i+64t]·x_{q+1+2t}[i]
//! G_q[31−i] = −Σ_t p[i+64t]·x_{q+2t}[i]   − Σ_t p[32+i+64t]·x_{q+1+2t}[i]
//! y_q       = c · M⁻¹·G_q
//! ```
//!
//! The scalar `c` (the inverse of the FIR pair's energy gain,
//! calibrated once at construction against the crate's own
//! [`crate::QmfSynthesis`]) makes `synthesis(analysis(x))` reproduce
//! `x` delayed by [`QMF_ANALYSIS_SYNTHESIS_DELAY`] samples. Each subband
//! row `q` needs PCM rows `q ..= q+16`, i.e. the analysis looks
//! [`QMF_ANALYSIS_LOOKAHEAD`] PCM samples ahead of the row it emits;
//! the encoder buffers accordingly.
//!
//! # Domain
//!
//! The bank is linear; the encoder feeds PCM in the decoder's subband
//! domain units (see [`crate::encoder`]) and the decoder's
//! `int(rScale·raZ)` output step maps back. Nothing here assumes a
//! particular integer resolution.

use crate::cos_mod::{cos_mod_stage, precal_cos_mod, NUM_SUBBAND};
use crate::filter_bank::FilterBankSelection;
use crate::qmf_assemble::X_HISTORY_LEN;

/// PCM samples the analysis bank must see **beyond** the 32 samples
/// of the row it produces (rows `q+1 ..= q+16` of the derivation).
pub const QMF_ANALYSIS_LOOKAHEAD: usize = 16 * NUM_SUBBAND;

/// PCM rows (of 32 samples) the analysis of one subband row spans:
/// the row itself plus [`QMF_ANALYSIS_LOOKAHEAD`].
pub const QMF_ANALYSIS_SPAN_ROWS: usize = 17;

/// End-to-end delay, in PCM samples, of `QmfSynthesis::synthesize`
/// applied to the output of [`QmfAnalysis::analyze`]: **zero** — the
/// analysis window's 512-sample lookahead exactly cancels the
/// synthesis filter's group delay, so reconstructed sample `n`
/// corresponds to input sample `n`. Pinned by the module tests.
pub const QMF_ANALYSIS_SYNTHESIS_DELAY: usize = 0;

/// 32-band cosine-modulated analysis filter bank.
///
/// Stateless apart from the precomputed modulation matrix and gain;
/// the caller keeps the PCM history/lookahead (see
/// [`Self::analyze`]).
#[derive(Debug, Clone)]
pub struct QmfAnalysis {
    /// `M⁻¹`, the inverse of the §C.2.5 cosine-modulation matrix
    /// (`M[i][k]` = `X[i]` produced by a unit sample in subband `k`).
    /// Row `k` of this array maps a `G` vector to subband `k`.
    m_inverse: [[f64; NUM_SUBBAND]; NUM_SUBBAND],
    /// §D.8 prototype (selected by `FILTS`).
    filter: FilterBankSelection,
    /// Output gain `c` (see the module docs).
    gain: f64,
}

impl QmfAnalysis {
    /// Build the analysis bank matching the decoder's synthesis for
    /// the given §D.8 prototype selection.
    #[must_use]
    pub fn new(filter: FilterBankSelection) -> Self {
        let cos_mod = precal_cos_mod();
        // Build M column-by-column from the decoder's own
        // cos_mod_stage, then invert it (Gauss-Jordan with partial
        // pivoting; M is a well-conditioned modulation matrix).
        let mut m = [[0.0_f64; NUM_SUBBAND]; NUM_SUBBAND];
        for k in 0..NUM_SUBBAND {
            let mut unit = [0.0_f64; NUM_SUBBAND];
            unit[k] = 1.0;
            let column = cos_mod_stage(&unit, &cos_mod);
            for i in 0..NUM_SUBBAND {
                m[i][k] = column[i];
            }
        }
        let m_inverse = invert(&m);
        let mut bank = Self {
            m_inverse,
            filter,
            gain: 1.0,
        };
        bank.gain = 1.0 / bank.round_trip_peak().1;
        bank
    }

    /// Measure the analysis→synthesis impulse response by pushing one
    /// unit impulse through the pair (deterministic; used once at
    /// construction to calibrate [`Self::gain`]). Returns
    /// `(delay, peak)`: the offset of the largest-magnitude response
    /// sample relative to the impulse, and its value.
    fn round_trip_peak(&self) -> (isize, f64) {
        let pos = 24 * NUM_SUBBAND;
        let mut pcm = vec![0.0_f64; 96 * NUM_SUBBAND + QMF_ANALYSIS_LOOKAHEAD];
        pcm[pos] = 1.0;
        let rows = self.analyze(&pcm);
        let mut synth = crate::qmf_synth::QmfSynthesis::new();
        let mut out_i = Vec::new();
        // Keep 30 fractional bits: the synthesis output is i32, so a
        // calibrated peak of 1.0 must stay well below i32::MAX.
        let scale = (1u32 << 30) as f64;
        synth
            .synthesize(&rows, NUM_SUBBAND, self.filter, scale, &mut out_i)
            .expect("row shape is fixed");
        let (arg, &peak) = out_i
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| v.unsigned_abs())
            .expect("non-empty output");
        (arg as isize - pos as isize, peak as f64 / scale)
    }

    /// The prototype this bank was built for.
    #[must_use]
    pub fn filter(&self) -> FilterBankSelection {
        self.filter
    }

    /// The scalar normalisation `c` applied to every output row.
    #[must_use]
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// Analyse one subband row from a window of
    /// [`QMF_ANALYSIS_SPAN_ROWS`]`·32` PCM samples: `window[0..32]` is
    /// the row being produced and the rest is lookahead.
    ///
    /// Returns the 32 subband samples of that row.
    ///
    /// # Panics
    ///
    /// If `window.len() < QMF_ANALYSIS_SPAN_ROWS * 32`.
    #[must_use]
    pub fn analyze_row(&self, window: &[f64]) -> [f64; NUM_SUBBAND] {
        assert!(
            window.len() >= QMF_ANALYSIS_SPAN_ROWS * NUM_SUBBAND,
            "analysis window needs {} samples, got {}",
            QMF_ANALYSIS_SPAN_ROWS * NUM_SUBBAND,
            window.len()
        );
        let p = self.filter.coefficients();
        let mut g = [0.0_f64; NUM_SUBBAND];
        for i in 0..NUM_SUBBAND {
            let mut fwd = 0.0_f64;
            let mut carry = 0.0_f64;
            for t in 0..(X_HISTORY_LEN / (2 * NUM_SUBBAND)) {
                fwd += p[i + 64 * t] * window[(2 * t) * NUM_SUBBAND + i];
                carry += p[NUM_SUBBAND + i + 64 * t] * window[(1 + 2 * t) * NUM_SUBBAND + i];
            }
            g[i] += fwd - carry;
            g[NUM_SUBBAND - 1 - i] += -fwd - carry;
        }
        let mut y = [0.0_f64; NUM_SUBBAND];
        for (slot, row) in y.iter_mut().zip(self.m_inverse.iter()) {
            let acc: f64 = row.iter().zip(g.iter()).map(|(a, b)| a * b).sum();
            *slot = acc * self.gain;
        }
        y
    }

    /// Analyse a whole PCM buffer into subband rows. Row `q` is
    /// computed from `pcm[32q .. 32q + 17·32]`; only rows whose full
    /// window lies inside `pcm` are produced (so the caller supplies
    /// [`QMF_ANALYSIS_LOOKAHEAD`] samples of lookahead past the last
    /// row it wants, or zero-pads at end of stream).
    #[must_use]
    pub fn analyze(&self, pcm: &[f64]) -> Vec<[f64; NUM_SUBBAND]> {
        let span = QMF_ANALYSIS_SPAN_ROWS * NUM_SUBBAND;
        if pcm.len() < span {
            return Vec::new();
        }
        let rows = (pcm.len() - span) / NUM_SUBBAND + 1;
        (0..rows)
            .map(|q| self.analyze_row(&pcm[q * NUM_SUBBAND..q * NUM_SUBBAND + span]))
            .collect()
    }
}

/// Invert a 32×32 matrix by Gauss-Jordan elimination with partial
/// pivoting. The §C.2.5 modulation matrix is well-conditioned; a
/// singular input would be a programming error, so it panics.
fn invert(m: &[[f64; NUM_SUBBAND]; NUM_SUBBAND]) -> [[f64; NUM_SUBBAND]; NUM_SUBBAND] {
    let n = NUM_SUBBAND;
    let mut a = *m;
    let mut inv = [[0.0_f64; NUM_SUBBAND]; NUM_SUBBAND];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&r1, &r2| a[r1][col].abs().partial_cmp(&a[r2][col].abs()).unwrap())
            .unwrap();
        assert!(a[pivot][col].abs() > 1e-12, "modulation matrix singular");
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..n {
                        a[r][j] -= f * a[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qmf_synth::QmfSynthesis;

    /// Deterministic noise in ±1.
    fn noise(n: usize, seed: u32) -> Vec<f64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f64 / (1u32 << 24) as f64 * 2.0 - 1.0
            })
            .collect()
    }

    fn snr_db(reference: &[f64], test: &[f64]) -> f64 {
        let sig: f64 = reference.iter().map(|v| v * v).sum();
        let err: f64 = reference
            .iter()
            .zip(test)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        10.0 * (sig / err.max(1e-300)).log10()
    }

    fn round_trip(filter: FilterBankSelection) -> (Vec<f64>, Vec<f64>) {
        let n = 32 * 400;
        let pcm = noise(n, 7);
        let bank = QmfAnalysis::new(filter);
        let mut padded = pcm.clone();
        padded.resize(n + QMF_ANALYSIS_LOOKAHEAD, 0.0);
        let rows = bank.analyze(&padded);
        assert_eq!(rows.len(), n / 32);
        let mut synth = QmfSynthesis::new();
        let mut out = Vec::new();
        // Keep 24 fractional bits so the integer output step of the
        // decoder does not dominate the measurement.
        let scale = (1u32 << 24) as f64;
        synth
            .synthesize(&rows, NUM_SUBBAND, filter, scale, &mut out)
            .unwrap();
        let out: Vec<f64> = out.iter().map(|&v| v as f64 / scale).collect();
        (pcm, out)
    }

    #[test]
    fn perfect_reconstruction_prototype_reconstructs_above_90_db() {
        let (pcm, out) = round_trip(FilterBankSelection::PerfectReconstruction);
        let d = QMF_ANALYSIS_SYNTHESIS_DELAY;
        // Skip the filter warm-up (first 512 samples of output are the
        // bank's own start-up transient against a zero history).
        let start = 1024;
        let reference = &pcm[start - d..pcm.len() - d];
        let test = &out[start..];
        let snr = snr_db(reference, test);
        assert!(snr > 90.0, "PR prototype round-trip SNR {snr:.1} dB");
    }

    #[test]
    fn non_perfect_prototype_reconstructs_above_40_db() {
        let (pcm, out) = round_trip(FilterBankSelection::NonPerfectReconstruction);
        let d = QMF_ANALYSIS_SYNTHESIS_DELAY;
        let start = 1024;
        let reference = &pcm[start - d..pcm.len() - d];
        let test = &out[start..];
        let snr = snr_db(reference, test);
        assert!(snr > 40.0, "non-PR prototype round-trip SNR {snr:.1} dB");
    }

    #[test]
    fn delay_constant_is_the_best_alignment() {
        let (pcm, out) = round_trip(FilterBankSelection::PerfectReconstruction);
        let start = 1024;
        let mut best = (0usize, f64::MIN);
        for d in 0..600 {
            let reference = &pcm[start - d..pcm.len() - d];
            let snr = snr_db(reference, &out[start..]);
            if snr > best.1 {
                best = (d, snr);
            }
        }
        assert_eq!(best.0, QMF_ANALYSIS_SYNTHESIS_DELAY);
    }

    #[test]
    fn impulse_peak_sits_at_zero_delay_with_unit_gain() {
        for filter in [
            FilterBankSelection::PerfectReconstruction,
            FilterBankSelection::NonPerfectReconstruction,
        ] {
            let bank = QmfAnalysis::new(filter);
            let (delay, peak) = bank.round_trip_peak();
            assert_eq!(delay, 0, "{filter:?}: peak {peak}");
            // gain() is calibrated as 1/peak of the *uncalibrated*
            // bank; after calibration the peak measured here is the
            // uncalibrated one again (round_trip_peak re-runs the
            // whole chain, including gain), so it must now be ~1.
            assert!((peak - 1.0).abs() < 1e-6, "{filter:?}: peak {peak}");
        }
    }

    #[test]
    fn unit_gain_after_round_trip() {
        let (pcm, out) = round_trip(FilterBankSelection::PerfectReconstruction);
        let d = QMF_ANALYSIS_SYNTHESIS_DELAY;
        let start = 1024;
        let reference = &pcm[start - d..pcm.len() - d];
        let test = &out[start..];
        let num: f64 = reference.iter().zip(test).map(|(a, b)| a * b).sum();
        let den: f64 = reference.iter().map(|a| a * a).sum();
        let gain = num / den;
        assert!((gain - 1.0).abs() < 1e-6, "round-trip gain {gain}");
    }

    #[test]
    fn subband_selectivity_places_tones_in_their_bands() {
        // A tone in the middle of band 5 must land (almost) entirely
        // in subband 5: the analysis bank is a real filter bank, not
        // just an algebraic inverse.
        let bank = QmfAnalysis::new(FilterBankSelection::PerfectReconstruction);
        let n = 32 * 256 + QMF_ANALYSIS_LOOKAHEAD;
        let f = (5.0 + 0.5) / 64.0; // centre of band 5, in cycles/sample
        let pcm: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64).sin())
            .collect();
        let rows = bank.analyze(&pcm);
        let mut energy = [0.0_f64; 32];
        for row in rows.iter().skip(32) {
            for (k, v) in row.iter().enumerate() {
                energy[k] += v * v;
            }
        }
        let total: f64 = energy.iter().sum();
        assert!(
            energy[5] / total > 0.99,
            "band-5 tone leaked: {:?}",
            energy.map(|e| (e / total * 1000.0).round() / 1000.0)
        );
    }

    #[test]
    fn analyze_needs_a_full_window() {
        let bank = QmfAnalysis::new(FilterBankSelection::PerfectReconstruction);
        assert!(bank.analyze(&[0.0; 32 * 16]).is_empty());
        assert_eq!(bank.analyze(&[0.0; 32 * 17]).len(), 1);
        assert_eq!(bank.analyze(&[0.0; 32 * 18]).len(), 2);
    }
}
