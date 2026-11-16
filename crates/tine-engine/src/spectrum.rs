//! 32-band voice spectrum computed from the post-DSP audio tap.
//!
//! [`SpectrumAnalyzer`] runs a 2048-point real FFT with a Hann window and
//! 50 % overlap (1024-sample hops), aggregates the power spectrum into 32
//! logarithmically spaced bands from 20 Hz to 20 kHz, and applies
//! display ballistics (10 ms attack / 300 ms decay). Output is dBFS with a
//! −90 dBFS floor.
//!
//! Not real-time: it is driven by the [`crate::visualizer`] thread at 60 Hz.
//! All allocation happens in [`SpectrumAnalyzer::new`]; [`SpectrumAnalyzer::feed`]
//! is allocation-free (proven by `assert_no_alloc`).

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::num_traits::Zero;
use realfft::{RealFftPlanner, RealToComplex};

/// FFT length in samples (2048-pt).
pub const FFT_LEN: usize = 2048;
/// Analysis hop between frames: 50 % overlap.
pub const HOP: usize = FFT_LEN / 2;
/// Number of display bands.
pub const BANDS: usize = 32;
/// Lowest band edge in Hz.
pub const MIN_HZ: f32 = 20.0;
/// Highest band edge in Hz.
pub const MAX_HZ: f32 = 20_000.0;
/// Attack time constant for the band meters in milliseconds.
pub const ATTACK_MS: f32 = 10.0;
/// Decay time constant for the band meters in milliseconds.
pub const DECAY_MS: f32 = 300.0;
/// Display floor in dBFS.
pub const FLOOR_DB: f32 = -90.0;

/// Fallback sample rate when `fs` passed to [`SpectrumAnalyzer::new`] is not
/// a positive finite value.
const FALLBACK_FS: f32 = 48_000.0;

/// DB offset so that a full-scale sine reads 0 dBFS: with a Hann window, the
/// peak bin of a full-scale sine has magnitude `FFT_LEN / 4`.
const REF_OFFSET_DB: f32 = -54.124_978; // -20*log10(FFT_LEN / 4)

/// Precomputed FFT bin coverage of one band.
struct BandRange {
    /// First FFT bin (by center frequency) covered by this band.
    first_bin: u32,
    /// Fractional overlap weight of each covered bin (`0..=1`), aligned with
    /// `first_bin + index`. A band narrower than one bin still weights that
    /// bin fractionally, so its value linearly interpolates between neighbours.
    weights: Box<[f32]>,
    /// Total of `weights` (0 when the band lies entirely above Nyquist).
    weight_sum: f32,
}

/// 2048-point Hann-windowed real FFT feeding 32 log-spaced dBFS bands with
/// attack/decay ballistics.
///
/// Every buffer is allocated in [`SpectrumAnalyzer::new`]; [`SpectrumAnalyzer::feed`]
/// performs no allocation, locking, or syscalls.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn RealToComplex<f32> + Send + Sync>,
    /// Hann window, length [`FFT_LEN`].
    window: Box<[f32]>,
    /// FFT input scratch (windowed), length [`FFT_LEN`].
    scratch_in: Box<[f32]>,
    /// FFT output spectrum, length `FFT_LEN / 2 + 1`.
    spectrum: Box<[Complex<f32>]>,
    /// FFT internal scratch (preallocated so `feed` stays allocation-free).
    fft_scratch: Box<[Complex<f32>]>,
    /// Sample history ring (length [`FFT_LEN`], power of two).
    hist: Box<[f32]>,
    /// Write position in `hist` (masked with `FFT_LEN - 1`).
    hist_pos: usize,
    /// Number of valid samples in `hist` (capped at [`FFT_LEN`]).
    filled: usize,
    /// Samples remaining until the next hop boundary.
    till_hop: usize,
    /// Precomputed band -> bin coverage.
    bands_plan: Box<[BandRange]>,
    /// Per-analysis band power accumulators (dB-normalized after the walk).
    acc: Box<[f32; BANDS]>,
    /// Current smoothed band values in dBFS.
    values: Box<[f32; BANDS]>,
    /// Attack smoothing coefficient per hop.
    alpha_attack: f32,
    /// Decay smoothing coefficient per hop.
    alpha_decay: f32,
}

impl SpectrumAnalyzer {
    /// Constructs the analyzer for a tap sampled at `fs` Hz. Non-positive or
    /// non-finite `fs` falls back to 48 kHz (the native rate, D-006).
    #[must_use]
    pub fn new(fs: f32) -> Self {
        let fs = if fs.is_finite() && fs > 0.0 { fs } else { FALLBACK_FS };
        let mut planner = RealFftPlanner::new();
        let fft: Arc<dyn RealToComplex<f32> + Send + Sync> = planner.plan_fft_forward(FFT_LEN);
        let scratch_len = fft.get_scratch_len();

        let window: Box<[f32]> = (0..FFT_LEN)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / FFT_LEN as f32).cos()))
            .collect();
        let hop_ms = HOP as f32 * 1000.0 / fs;
        // One-pole ballistics coefficients per hop (size = time constant).
        // Float math only; exponential approximation error is far below the display resolution.
        let alpha_attack = 1.0 - (-hop_ms / ATTACK_MS).exp();
        let alpha_decay = 1.0 - (-hop_ms / DECAY_MS).exp();

        // Log-spaced band edges 20 Hz..20 kHz, expressed as fractional FFT-bin
        // positions (bin k == k * fs / FFT_LEN, spanning [k-0.5, k+0.5)).
        let bin_hz = fs / FFT_LEN as f32;
        let edges: Vec<f64> = (0..=BANDS)
            .map(|i| f64::from(MIN_HZ) * (f64::from(MAX_HZ) / f64::from(MIN_HZ)).powf(i as f64 / BANDS as f64))
            .collect();

        let bands_plan: Box<[BandRange]> = (0..BANDS)
            .map(|i| {
                let lo = edges[i] / f64::from(bin_hz);
                let hi = edges[i + 1] / f64::from(bin_hz);
                // Bins are indexed by center; band [lo, hi] overlaps bin k in
                // (lo - 0.5, hi + 0.5). DC (bin 0) is never counted.
                let max_bin = FFT_LEN / 2;
                let lo_minus = lo - 0.5;
                let hi_plus = hi + 0.5;
                let k_start = lo_minus.clamp(1.0, max_bin as f64 + 1.0).ceil() as u32;
                let k_end = hi_plus.clamp(0.0, max_bin as f64).floor() as u32;
                // (float clamps above keep the index math within 0..=max_bin)
                let mut weights: Vec<f32> = Vec::with_capacity((k_end.saturating_sub(k_start) + 1) as usize);
                let mut sum = 0.0_f32;
                for k in k_start..=k_end {
                    let overlap = (hi.min(f64::from(k) + 0.5) - lo.max(f64::from(k) - 0.5)).max(0.0);
                    let w = overlap as f32;
                    weights.push(w);
                    sum += w;
                }
                BandRange {
                    first_bin: k_start,
                    weights: weights.into_boxed_slice(),
                    weight_sum: sum,
                }
            })
            .collect();

        Self {
            fft,
            window,
            scratch_in: vec![0.0; FFT_LEN].into_boxed_slice(),
            spectrum: vec![Complex::zero(); FFT_LEN / 2 + 1].into_boxed_slice(),
            fft_scratch: vec![Complex::zero(); scratch_len].into_boxed_slice(),
            hist: vec![0.0; FFT_LEN].into_boxed_slice(),
            hist_pos: 0,
            filled: 0,
            till_hop: HOP,
            bands_plan,
            acc: Box::new([FLOOR_DB; BANDS]),
            values: Box::new([FLOOR_DB; BANDS]),
            alpha_attack,
            alpha_decay,
        }
    }

    /// Feeds tap samples into the analyzer.
    ///
    /// Buffers of any length are accepted; analysis runs once per
    /// [`HOP`] samples (non-finite input samples are treated as silence).
    /// Allocation-free.
    pub fn feed(&mut self, s: &[f32]) {
        for &x in s {
            let v = if x.is_finite() { x } else { 0.0 };
            if let Some(slot) = self.hist.get_mut(self.hist_pos & (FFT_LEN - 1)) {
                *slot = v;
            }
            self.hist_pos = self.hist_pos.wrapping_add(1);
            self.filled = (self.filled + 1).min(FFT_LEN);
            self.till_hop -= 1;
            if self.till_hop == 0 {
                self.till_hop = HOP;
                self.analyze();
            }
        }
    }

    /// The current smoothed band values in dBFS (−90 dBFS floor).
    #[must_use]
    pub fn bands(&self) -> &[f32; BANDS] {
        &self.values
    }

    /// Runs one windowed FFT over the newest [`FFT_LEN`] samples (zero-padded
    /// at the very start of the stream) and updates the ballistically smoothed
    /// band values.
    fn analyze(&mut self) {
        let n = self.filled;
        let pos = self.hist_pos;
        // Order the history so the newest sample is last (chronological).
        for j in 0..(FFT_LEN - n) {
            self.scratch_in[j] = 0.0;
        }
        for j in 0..n {
            self.scratch_in[FFT_LEN - n + j] = self
                .hist
                .get((pos + FFT_LEN - n + j) & (FFT_LEN - 1))
                .copied()
                .unwrap_or(0.0);
        }
        for i in 0..FFT_LEN {
            let w = self.window.get(i).copied().unwrap_or(0.0);
            if let Some(slot) = self.scratch_in.get_mut(i) {
                *slot *= w;
            }
        }

        let ok = self
            .fft
            .process_with_scratch(&mut self.scratch_in, &mut self.spectrum, &mut self.fft_scratch)
            .is_ok();
        if ok {
            let acc = &mut *self.acc;
            acc.fill(0.0);
            for (band, plan) in acc.iter_mut().zip(self.bands_plan.iter()) {
                let mut num = 0.0_f32;
                for (off, w) in plan.weights.iter().enumerate() {
                    let k = plan.first_bin as usize + off;
                    if let Some(c) = self.spectrum.get(k) {
                        num += w * (c.re * c.re + c.im * c.im);
                    }
                }
                *band = num;
            }
            for i in 0..BANDS {
                let plan = &self.bands_plan[i];
                let value = if plan.weight_sum > 0.0 && self.acc[i] > 0.0 {
                    // Band energy (fractional-weighted power sum): a full-scale
                    // sine reads 0 dBFS, and pink (equal energy per octave) is
                    // flat across the log-spaced bands.
                    (10.0 * self.acc[i].log10() + REF_OFFSET_DB).max(FLOOR_DB)
                } else {
                    FLOOR_DB
                };
                // Ballistics: one-pole smoothing per analysis hop.
                let cur = self.values[i];
                let alpha = if value > cur {
                    self.alpha_attack
                } else {
                    self.alpha_decay
                };
                self.values[i] += (value - cur) * alpha;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BANDS, FFT_LEN, FLOOR_DB, HOP, MAX_HZ, MIN_HZ, SpectrumAnalyzer};

    struct Xorshift(u64);

    impl Xorshift {
        fn next_f32(&mut self) -> f32 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            ((self.0 & 0xffff) as f32 / 32_768.0) - 1.0
        }
    }

    /// Expected (fractional) band index for a frequency.
    fn band_of(hz: f64) -> f64 {
        (hz / f64::from(MIN_HZ)).ln() / (f64::from(MAX_HZ) / f64::from(MIN_HZ)).ln() * BANDS as f64
    }

    #[test]
    fn silence_floors_and_decay_is_slow() {
        let fs = 48_000.0_f32;
        let mut an = SpectrumAnalyzer::new(fs);
        an.feed(&vec![0.0; HOP]);
        for b in an.bands() {
            assert!(*b <= FLOOR_DB + 1e-6, "silent bands must sit at the floor: {b}");
        }
        // Loud noise, then silence: decay time constant is 300 ms.
        let mut white = Xorshift(0x00ff_00ff_00ff_00ff);
        let loud: Vec<f32> = (0..(fs as usize * 2)).map(|_| white.next_f32() * 0.7).collect();
        an.feed(&loud);
        let peak = an.bands()[16];
        // One decay time constant of silence (~300 ms): expect ~63 % drop toward FLOOR.
        an.feed(&vec![0.0; fs as usize / 3]);
        let after = an.bands()[16];
        let one_tau = FLOOR_DB + (peak - FLOOR_DB) * 0.368;
        assert!(
            (after - one_tau).abs() < 4.0,
            "decay after ~1 tau: {after:.1} vs expected {one_tau:.1} (peak {peak:.1})"
        );
    }

    #[test]
    fn feed_is_allocation_free() {
        #[global_allocator]
        static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

        let mut an = SpectrumAnalyzer::new(48_000.0);
        let data = vec![0.25_f32; 5_000];
        assert_no_alloc::assert_no_alloc(|| {
            an.feed(&data);
        });
        let _ = an.bands();
    }

    #[test]
    fn constants_match_the_contract() {
        assert_eq!(BANDS, 32);
        assert_eq!(FFT_LEN, 2048);
        assert_eq!(HOP, FFT_LEN / 2);
        assert!((MAX_HZ / MIN_HZ - 1000.0).abs() < 1e-3);
        assert_eq!(band_of(20.0), 0.0);
        assert_eq!(band_of(20_000.0) as usize, BANDS);
    }
}
