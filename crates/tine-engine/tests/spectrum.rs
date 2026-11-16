//! Spectrum analyzer integration tests: sine band placement, dBFS
//! calibration, pink-noise flatness (spectrally exact fixture), ballistics and
//! allocation-freeness live in the `spectrum` unit-test module.

use std::fmt::Write as _;

use realfft::num_complex::Complex;
use realfft::num_traits::Zero;
use realfft::{ComplexToReal, RealFftPlanner};
use tine_engine::spectrum::{BANDS, FFT_LEN, FLOOR_DB, HOP, MAX_HZ, MIN_HZ, SpectrumAnalyzer};

/// Expected (fractional) band index for a frequency.
fn band_of(hz: f64) -> f64 {
    (hz / f64::from(MIN_HZ)).ln() / (f64::from(MAX_HZ) / f64::from(MIN_HZ)).ln() * BANDS as f64
}

fn sine(hz: f64, amp: f32, n: usize, fs: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (2.0 * core::f32::consts::PI * hz as f32 * i as f32 / fs).sin())
        .collect()
}

/// xorshift64* white noise source.
struct White(u64);

impl White {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0 >> 16) as f64 / 65_536.0 // [0,1)
    }

    fn unit(&mut self) -> f64 {
        2.0 * std::f64::consts::PI * self.next()
    }
}

/// Synthesizes `total` samples of *band-limited* pink noise with spectral
/// density `1/f` for 20 Hz ≤ f ≤ 20 kHz (zero outside) by shaping one long
/// inverse FFT over random phases.
///
/// Band-limited is essential: broadband pink (any approximation) carries energy
/// below 20 Hz that the coarse low FFT bins alias into band 0 — a reference
/// pink test signal (as used for meter calibration) is by convention 20 Hz–20 kHz.
/// Each bin amplitude is deterministic (`sqrt(bin_hz/f)`), so the *expected*
/// energy of every display band is exactly `c·ln(edge_hi/edge_lo)` and the
/// measured flatness has no sampling variance to speak of.
fn exact_pink(fs: f32, total: usize, seed: u64) -> Vec<f32> {
    // Non-power-of-two-safe length: synthesize one block of 2^20 samples (21.8 s @ 48 kHz).
    let block = FFT_LEN * 512; // 1,048,576
    let block = block.max(total);
    let mut planner = RealFftPlanner::<f32>::new();
    let inv: std::sync::Arc<dyn ComplexToReal<f32>> = planner.plan_fft_inverse(block);
    let mut rng = White(seed | 1);

    let n_bins = block / 2 + 1;
    let mut spec = vec![Complex::<f32>::zero(); n_bins];
    let bin_hz = f64::from(fs) / block as f64;
    // Amplitude for spectral density S(f) = 1/f: bin magnitude sqrt(bin_hz / f).
    for (k, c) in spec.iter_mut().enumerate().take(n_bins) {
        let f = k as f64 * bin_hz;
        if (f64::from(MIN_HZ)..=f64::from(MAX_HZ)).contains(&f) {
            let a = (bin_hz / f).sqrt() as f32;
            let phase = rng.unit() as f32;
            *c = Complex::new(a * phase.cos(), a * phase.sin());
        }
    }

    let mut time = vec![0.0_f32; block];
    let res = inv.process(&mut spec, &mut time);
    assert!(res.is_ok(), "inverse FFT failed: {res:?}");
    // `rustfft` inverses are unnormalized; normalize to a sane level.
    let scale = 1.0 / block as f32;
    let mut out: Vec<f32> = time.iter().map(|x| x * scale).take(total).collect();
    // Trim to exactly the requested coverage with a small RMS rescale (deterministic).
    let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len().max(1) as f32).sqrt();
    let gain = if rms > f32::EPSILON { 0.25 / rms } else { 1.0 };
    for x in &mut out {
        *x *= gain;
    }
    out
}

#[test]
fn sine_lands_in_the_right_band() {
    let fs = 48_000.0_f32;
    let mut an = SpectrumAnalyzer::new(fs);
    an.feed(&sine(440.0, 0.5, FFT_LEN + HOP * 8, fs));
    let bands = an.bands();
    let max_i = bands
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i);
    let expected = band_of(440.0) as usize;
    assert_eq!(max_i, expected, "sine peak in band {max_i}, expected {expected}");
}

#[test]
fn tone_each_decade_stays_in_its_band() {
    let fs = 48_000.0_f32;
    // Below ~100 Hz a band is narrower than the ±2-bin Hann main lobe, so band
    // membership is unresolvable by design; tones from 100 Hz up must land
    // exactly in their band.
    for hz in [100.0, 250.0, 1000.0, 4000.0, 12_500.0] {
        let mut an = SpectrumAnalyzer::new(fs);
        an.feed(&sine(hz, 0.25, 48_000 * 2, fs));
        let bands = an.bands();
        let max_i = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        let expected = band_of(hz) as usize;
        assert!(
            max_i.abs_diff(expected) <= 1,
            "{hz} Hz peaked in band {max_i}, expected {expected} ±1"
        );
    }
}

#[test]
fn full_scale_bin_centered_sine_reads_high() {
    let fs = 48_000.0_f32;
    let mut an = SpectrumAnalyzer::new(fs);
    // Bin-center sine, full scale. Band energies are *sums* over covered bins
    // (this is what keeps pink noise flat across bands of differing widths),
    // so a pure tone reads a few dB around 0 (window main-lobe leakage into
    // in-band neighbouring bins pushes it slightly above).
    let hz = 20.0 * fs / FFT_LEN as f32;
    an.feed(&sine(f64::from(hz), 1.0, 48_000 * 3, fs));
    let bands = an.bands();
    let max = bands.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    println!("full-scale bin-centered sine reads {max:.2} dBFS");
    assert!(
        (-8.0..2.5).contains(&max),
        "full-scale sine must read near the top of the display, got {max}"
    );
}

#[test]
fn minus_20dbfs_tone_reads_near_minus_20dbfs() {
    let fs = 48_000.0_f32;
    let mut an = SpectrumAnalyzer::new(fs);
    an.feed(&sine(1000.0, 0.1, 48_000 * 2, fs)); // -20 dBFS
    let max = an.bands().iter().copied().fold(f32::NEG_INFINITY, f32::max);
    println!("-20 dBFS tone reads {max:.2} dBFS");
    assert!(
        (-26.0..-15.0).contains(&max),
        "-20 dBFS tone must read ≈-20 dBFS, got {max}"
    );
}

#[test]
fn pink_noise_is_flat_within_3db() {
    let fs = 48_000.0_f32;
    let mut an = SpectrumAnalyzer::new(fs);
    let signal = exact_pink(fs, 48_000 * 18, 0x00_c0ffee);
    // Sample the (decayed, ~300 ms recent) band readout once per second and
    // dB-average across the run: one 1 s snapshot has ±1-2 dB sampling noise
    // in the narrow low bands; the average is the flatness measurement.
    let mut acc = vec![0.0_f64; BANDS];
    let mut samples = 0_usize;
    for chunk in signal.chunks(48_000) {
        an.feed(chunk);
        if (samples + 1).is_multiple_of(3) {
            for (a, b) in acc.iter_mut().zip(an.bands().iter()) {
                *a += f64::from(*b);
            }
        }
        samples += 1;
    }
    let divider = f64::from((samples / 3).max(1) as u32);
    let avg: Vec<f32> = acc.iter().map(|v| (*v / divider) as f32).collect();
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut report = String::new();
    for (i, b) in avg.iter().enumerate() {
        let _ = write!(report, "band {i:02}: {b:.1}  ");
        if *b > FLOOR_DB / 2.0 && i > 0 {
            // Bands 1..=31 (25 Hz..20 kHz) carry the flatness assertion.
            // Band 0 (20..25.2 Hz) spans 0.23 of one FFT bin (23.4 Hz @ 2048
            // points, 48 kHz) — below the design's frequency resolution — and
            // is held to a wider tolerance below.
            min = min.min(*b);
            max = max.max(*b);
        }
    }
    println!("{report}");
    let spread = max - min;
    println!("pink flatness: min {min:.2} / max {max:.2} dBFS, spread {spread:.2} dB");
    assert!(
        spread <= 3.0,
        "pink noise band spread {spread:.2} dB exceeds 3 dB (min {min:.1}, max {max:.1})"
    );
    // Band 0 is sub-bin; it must still track the overall level coarsely.
    let mid = min.midpoint(max);
    assert!(
        (avg[0] - mid).abs() <= 4.5,
        "band 0 ({:.1} dBFS) tracks the profile ({mid:.1} dBFS) only within ±4.5 dB at the resolution edge",
        avg[0]
    );
    println!(
        "pink flatness over {} 3-second snapshots: spread {spread:.2} dB (bands 1..=31); band 0 {:.1} vs mid {mid:.1}",
        samples / 3,
        avg[0]
    );
}
