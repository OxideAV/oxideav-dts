//! DTS Coherent Acoustics — LFE channel **decimation** (encoder side),
//! the adjoint of the §C.2.6 `InterpolationFIR()` driver plus an
//! in-band equalizer.
//!
//! §5.5 (`LFE`, PDF p.33) says the LFE channel is carried as decimated
//! samples — `2·LFF·nSSC` per subframe, i.e. one per 64 (`LFF=2`) or
//! 128 (`LFF=1`) core PCM samples — and the decoder rebuilds the full-
//! rate channel with the 512-tap §D.8 LFE interpolation FIR
//! (`raCoeff64` / `raCoeff128`) through §C.2.6:
//!
//! ```text
//! naCh[nDeciFactor·j + k] = Σ_{J < 512/nDeciFactor} rLFE[j − J] · prCoeff[k + J·nDeciFactor]
//! ```
//!
//! The matching decimator is the adjoint of that map — the same
//! prototype used as the anti-alias low-pass ahead of the decimation:
//!
//! ```text
//! rLFE[q] = (1 / nDeciFactor) · Σ_{n < 512} prCoeff[n] · pcm[nDeciFactor·q + n]
//! ```
//!
//! The `1/nDeciFactor` restores unit DC gain (the §D.8 LFE columns sum
//! to the decimation factor). The decimator for output sample `q`
//! looks 512 PCM samples ahead of `nDeciFactor·q`, which exactly
//! cancels the interpolator's group delay: the decimate→interpolate
//! pair has **zero** net delay ([`LFE_ANALYSIS_SYNTHESIS_DELAY`]).
//!
//! # In-band equalizer
//!
//! The §D.8 LFE prototype is not flat over the LFE band: its
//! measured response (identical for the 64× and 128× columns, taken
//! from the staged `tables/raCoeffLfe*.csv` data) is ≈ 0.93 at 40 Hz,
//! 0.75 at 80 Hz, 0.52 at 120 Hz and 0.30 at 160 Hz (at 48 kHz).
//! Decimating with the adjoint applies that droop a second time, so a
//! plain adjoint round trip sags ≈ 5 dB at 80 Hz. [`LfeAnalysis`]
//! therefore applies a short symmetric FIR equalizer **in the
//! decimated domain**, least-squares fitted at construction to the
//! Wiener-style target `|H|² / (|H|⁴ + ε)` so the decimate→interpolate
//! chain is ≈ flat where the prototype passes signal and rolls off
//! where it doesn't. The equalizer is linear-phase and centred, so
//! the zero-delay property is preserved.

use crate::lfe_fir_coeff::LFE_FIR_COEFF_LEN;
use crate::lfe_interp::LfeInterpolationSelection;

/// End-to-end delay, in PCM samples, of the §C.2.6 interpolator
/// applied to the output of [`LfeAnalysis::decimate`]: zero (the
/// decimator's 512-sample lookahead cancels the interpolator's group
/// delay, and the equalizer is centred).
pub const LFE_ANALYSIS_SYNTHESIS_DELAY: usize = 0;

/// Half-order of the decimated-domain equalizer: the FIR has
/// `2·LFE_EQ_HALF + 1` taps, symmetric around its centre.
pub const LFE_EQ_HALF: usize = 12;

/// LFE decimator matching the decoder's §C.2.6 interpolator.
#[derive(Debug, Clone)]
pub struct LfeAnalysis {
    selection: LfeInterpolationSelection,
    /// Symmetric equalizer taps `e[0..=2·LFE_EQ_HALF]` (decimated
    /// domain), centred at `LFE_EQ_HALF`.
    eq: [f64; 2 * LFE_EQ_HALF + 1],
}

impl LfeAnalysis {
    /// Build a decimator (with in-band equalizer) for the given `LFF`
    /// interpolation selection.
    #[must_use]
    pub fn new(selection: LfeInterpolationSelection) -> Self {
        Self {
            selection,
            eq: design_equalizer(selection),
        }
    }

    /// The interpolation selection (decimation factor) in use.
    #[must_use]
    pub fn selection(&self) -> LfeInterpolationSelection {
        self.selection
    }

    /// Decimation factor (64 or 128).
    #[must_use]
    pub fn factor(&self) -> usize {
        self.selection.decimation_factor() as usize
    }

    /// The equalizer taps (symmetric, centred at [`LFE_EQ_HALF`]).
    #[must_use]
    pub fn equalizer(&self) -> &[f64; 2 * LFE_EQ_HALF + 1] {
        &self.eq
    }

    /// One **unequalized** (plain adjoint) decimated sample from
    /// `window[0 .. 512]` (the 512 PCM samples starting at the
    /// decimated sample's own position).
    ///
    /// # Panics
    ///
    /// If `window.len() < 512`.
    #[must_use]
    pub fn decimate_one_raw(&self, window: &[f64]) -> f64 {
        assert!(window.len() >= LFE_FIR_COEFF_LEN);
        let coeff = self.selection.coefficients();
        let acc: f64 = coeff.iter().zip(window).map(|(c, x)| c * x).sum();
        acc / self.factor() as f64
    }

    /// Decimate a PCM buffer (adjoint + equalizer). Output sample `q`
    /// is computed from
    /// `pcm[factor·(q − LFE_EQ_HALF) .. factor·(q + LFE_EQ_HALF) + 512]`;
    /// the buffer is treated as zero outside its bounds (so the
    /// caller supplies `factor·LFE_EQ_HALF + 512 − factor` samples of
    /// lookahead past the last wanted output, or zero-pads at end of
    /// stream), and `count` outputs are produced.
    #[must_use]
    pub fn decimate_count(&self, pcm: &[f64], count: usize) -> Vec<f64> {
        let f = self.factor();
        // Raw adjoint outputs for q in −LFE_EQ_HALF .. count+LFE_EQ_HALF,
        // reading zeros outside `pcm`.
        let raw: Vec<f64> = (-(LFE_EQ_HALF as isize)..(count + LFE_EQ_HALF) as isize)
            .map(|q| {
                let start = q * f as isize;
                let coeff = self.selection.coefficients();
                let mut acc = 0.0_f64;
                for (n, c) in coeff.iter().enumerate() {
                    let idx = start + n as isize;
                    if idx >= 0 && (idx as usize) < pcm.len() {
                        acc += c * pcm[idx as usize];
                    }
                }
                acc / f as f64
            })
            .collect();
        (0..count)
            .map(|q| {
                self.eq
                    .iter()
                    .enumerate()
                    .map(|(m, e)| e * raw[q + m])
                    .sum()
            })
            .collect()
    }

    /// Decimate a PCM buffer, producing every output whose full raw
    /// (unequalized) window lies inside `pcm`.
    #[must_use]
    pub fn decimate(&self, pcm: &[f64]) -> Vec<f64> {
        if pcm.len() < LFE_FIR_COEFF_LEN {
            return Vec::new();
        }
        let f = self.factor();
        let count = (pcm.len() - LFE_FIR_COEFF_LEN) / f + 1;
        self.decimate_count(pcm, count)
    }
}

/// Least-squares fit of the symmetric decimated-domain equalizer for
/// one decimation factor (see the module docs).
fn design_equalizer(selection: LfeInterpolationSelection) -> [f64; 2 * LFE_EQ_HALF + 1] {
    let coeff = selection.coefficients();
    let factor = selection.decimation_factor() as f64;
    // Frequency grid over the decimated-domain band 0..π, weighted
    // toward the LFE band proper (the fit must be tight where LFE
    // content lives, loose where the prototype already rejects).
    const GRID: usize = 257;
    let mut target = [0.0_f64; GRID];
    let mut weight = [0.0_f64; GRID];
    // Decimated-domain sample rate at the 48 kHz core rate; only used
    // to place the weighting knee, so the exact core rate is
    // immaterial (the response scales with it).
    let dec_rate = 48_000.0 / factor;
    // Cap the boost at ~16 dB so the equalizer never amplifies the
    // prototype's stop band (or quantization noise) unboundedly.
    let cap = 6.0_f64;
    for i in 0..GRID {
        let omega_dec = std::f64::consts::PI * i as f64 / (GRID - 1) as f64;
        // PCM-domain angular frequency of this decimated-domain bin.
        let omega_pcm = omega_dec / factor;
        // |H| of the §D.8 prototype at that frequency (normalized to
        // unit DC gain).
        let (mut re, mut im) = (0.0_f64, 0.0_f64);
        for (n, c) in coeff.iter().enumerate() {
            re += c * (omega_pcm * n as f64).cos();
            im += c * (omega_pcm * n as f64).sin();
        }
        let h = (re * re + im * im).sqrt() / factor;
        let h2 = (h * h).max(1.0 / cap);
        target[i] = 1.0 / h2;
        let hz = omega_dec / (2.0 * std::f64::consts::PI) * dec_rate;
        weight[i] = if hz <= 180.0 {
            1.0
        } else if hz <= 300.0 {
            0.25
        } else {
            0.05
        };
    }
    // Linear-phase FIR: E(ω) = a0 + 2·Σ a_m cos(mω). Solve the
    // weighted normal equations for a (LFE_EQ_HALF+1 unknowns).
    const NA: usize = LFE_EQ_HALF + 1;
    let mut ata = [[0.0_f64; NA]; NA];
    let mut atb = [0.0_f64; NA];
    for (i, &t) in target.iter().enumerate() {
        let omega = std::f64::consts::PI * i as f64 / (GRID - 1) as f64;
        let w = weight[i];
        let mut basis = [0.0_f64; NA];
        basis[0] = 1.0;
        for (m, b) in basis.iter_mut().enumerate().skip(1) {
            *b = 2.0 * (m as f64 * omega).cos();
        }
        for r in 0..NA {
            for c in 0..NA {
                ata[r][c] += w * basis[r] * basis[c];
            }
            atb[r] += w * basis[r] * t;
        }
    }
    let a = solve(&mut ata, &mut atb);
    let mut eq = [0.0_f64; 2 * LFE_EQ_HALF + 1];
    eq[LFE_EQ_HALF] = a[0];
    for m in 1..NA {
        eq[LFE_EQ_HALF - m] = a[m];
        eq[LFE_EQ_HALF + m] = a[m];
    }
    eq
}

/// Solve the small symmetric positive-definite system `ata·x = atb`
/// by Gaussian elimination with partial pivoting (in place).
fn solve<const N: usize>(ata: &mut [[f64; N]; N], atb: &mut [f64; N]) -> [f64; N] {
    for col in 0..N {
        let pivot = (col..N)
            .max_by(|&r1, &r2| ata[r1][col].abs().partial_cmp(&ata[r2][col].abs()).unwrap())
            .unwrap();
        ata.swap(col, pivot);
        atb.swap(col, pivot);
        let d = ata[col][col];
        assert!(d.abs() > 1e-12, "singular equalizer system");
        for v in ata[col].iter_mut() {
            *v /= d;
        }
        atb[col] /= d;
        for r in 0..N {
            if r != col {
                let f = ata[r][col];
                if f != 0.0 {
                    let pivot_row = ata[col];
                    for (v, pv) in ata[r].iter_mut().zip(pivot_row.iter()) {
                        *v -= f * pv;
                    }
                    atb[r] -= f * atb[col];
                }
            }
        }
    }
    *atb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lfe_synth::LfeInterpolator;

    /// Band-limited test signal: a sum of low tones (an LFE channel is
    /// ≤ ~120 Hz at 48 kHz).
    fn tones(n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                0.5 * (2.0 * std::f64::consts::PI * 40.0 * t).sin()
                    + 0.3 * (2.0 * std::f64::consts::PI * 83.0 * t).sin()
                    + 0.2 * (2.0 * std::f64::consts::PI * 117.0 * t).cos()
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

    fn round_trip(sel: LfeInterpolationSelection) -> (Vec<f64>, Vec<f64>) {
        let n = 48_000;
        let pcm = tones(n);
        let dec = LfeAnalysis::new(sel);
        let decimated = dec.decimate_count(&pcm, n / dec.factor());
        assert_eq!(decimated.len(), n / dec.factor());
        let mut interp = LfeInterpolator::new();
        let out = interp.process_to_vec(&decimated, sel);
        (pcm, out)
    }

    #[test]
    fn decimation_64_round_trips_low_tones() {
        let (pcm, out) = round_trip(LfeInterpolationSelection::Decimation64);
        let start = 4096;
        let end = pcm.len() - 4096;
        let snr = snr_db(&pcm[start..end], &out[start..end]);
        assert!(snr > 25.0, "64x LFE round-trip SNR {snr:.1} dB");
    }

    #[test]
    fn decimation_128_round_trips_low_tones() {
        let (pcm, out) = round_trip(LfeInterpolationSelection::Decimation128);
        let start = 4096;
        let end = pcm.len() - 4096;
        let snr = snr_db(&pcm[start..end], &out[start..end]);
        assert!(snr > 20.0, "128x LFE round-trip SNR {snr:.1} dB");
    }

    #[test]
    fn zero_delay_is_the_best_alignment() {
        for sel in [
            LfeInterpolationSelection::Decimation64,
            LfeInterpolationSelection::Decimation128,
        ] {
            let (pcm, out) = round_trip(sel);
            let start = 4096;
            let end = pcm.len() - 4096;
            let mut best = (0usize, f64::MIN);
            for d in 0..600 {
                let snr = snr_db(&pcm[start - d..end - d], &out[start..end]);
                if snr > best.1 {
                    best = (d, snr);
                }
            }
            assert_eq!(best.0, LFE_ANALYSIS_SYNTHESIS_DELAY, "{sel:?}");
        }
    }

    #[test]
    fn dc_gain_is_near_unity() {
        for sel in [
            LfeInterpolationSelection::Decimation64,
            LfeInterpolationSelection::Decimation128,
        ] {
            let dec = LfeAnalysis::new(sel);
            let raw = dec.decimate_one_raw(&[0.25; LFE_FIR_COEFF_LEN]);
            assert!((raw - 0.25).abs() < 1e-4, "{sel:?}: raw DC {raw}");
            // Equalized DC: constant input, well inside the buffer.
            let pcm = vec![0.25_f64; 64 * 128 + 512];
            let d = dec.decimate(&pcm);
            let mid = d[d.len() / 2];
            assert!((mid - 0.25).abs() < 0.01, "{sel:?}: eq DC {mid}");
        }
    }

    #[test]
    fn equalizer_is_symmetric() {
        let dec = LfeAnalysis::new(LfeInterpolationSelection::Decimation64);
        let eq = dec.equalizer();
        for m in 0..LFE_EQ_HALF {
            assert_eq!(eq[m], eq[2 * LFE_EQ_HALF - m]);
        }
    }
}
