//! Shared DSP test harness: signal generators, acoustic measurements, WAV I/O, and RT assertions.

#![allow(dead_code, clippy::missing_docs_in_private_items)]

use std::f32::consts::PI;
use std::path::Path;

use realfft::RealFftPlanner;
use tine_dsp::primitives::db::{DB_FLOOR, lin_to_db};

/// Asserts that the enclosed block performs zero heap allocations.
#[macro_export]
macro_rules! rt {
    ($($body:tt)*) => {
        assert_no_alloc::assert_no_alloc(|| { $($body)* })
    };
}

/// Asserts that `actual_db` is within `tol_db` of `expected_db`.
#[macro_export]
macro_rules! assert_close_db {
    ($actual_db:expr, $expected_db:expr, $tol_db:expr) => {{
        let actual = $actual_db;
        let expected = $expected_db;
        let tol = $tol_db;
        assert!(
            (actual - expected).abs() <= tol,
            "dB mismatch: actual={actual:.3} dB, expected={expected:.3} dB (tol=±{tol:.3} dB)"
        );
    }};
}

/// Generates a mono sine wave of length `len` at `freq_hz` and `amplitude`.
pub fn sine(freq_hz: f32, fs: f32, len: usize, amplitude: f32) -> Vec<f32> {
    let w = 2.0 * PI * freq_hz / fs;
    (0..len).map(|n| amplitude * (w * (n as f32)).sin()).collect()
}

/// Generates an exponential (logarithmic) sine sweep from `f_start` to `f_end`.
pub fn log_sweep(f_start: f32, f_end: f32, fs: f32, len: usize, amplitude: f32) -> Vec<f32> {
    if len == 0 {
        return Vec::new();
    }
    let duration = (len as f64) / f64::from(fs);
    let f0 = f64::from(f_start);
    let f1 = f64::from(f_end);
    let k = (f1 / f0).ln() / duration;
    (0..len)
        .map(|n| {
            let t = (n as f64) / f64::from(fs);
            let phase = 2.0 * std::f64::consts::PI * f0 * ((k * t).exp() - 1.0) / k;
            amplitude * (phase.sin() as f32)
        })
        .collect()
}

/// Generates deterministic white noise in `[-amplitude, amplitude]`.
pub fn white_noise(len: usize, amplitude: f32, seed: u64) -> Vec<f32> {
    let mut state = seed.max(1);
    (0..len)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let u = ((state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / 16_777_216.0;
            (u * 2.0 - 1.0) * amplitude
        })
        .collect()
}

/// Generates deterministic pink noise (Paul Kellet 7-pole approximation), normalized to `amplitude`.
pub fn pink_noise(len: usize, amplitude: f32, seed: u64) -> Vec<f32> {
    let white = white_noise(len, 1.0, seed);
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) = (0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let mut out = Vec::with_capacity(len);
    let mut max_abs = 1e-6f32;
    for w in white {
        b0 = 0.99886 * b0 + w * 0.055_517_9;
        b1 = 0.99332 * b1 + w * 0.075_075_9;
        b2 = 0.96900 * b2 + w * 0.153_852;
        b3 = 0.86650 * b3 + w * 0.310_485_6;
        b4 = 0.55000 * b4 + w * 0.532_952_2;
        b5 = -0.7616 * b5 - w * 0.016_898_0;
        let pink = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
        b6 = w * 0.115_926;
        max_abs = max_abs.max(pink.abs());
        out.push(pink);
    }
    let scale = amplitude / max_abs;
    for s in &mut out {
        *s *= scale;
    }
    out
}

/// Generates a unit impulse at `delay_samples` with total length `len`.
pub fn impulse(len: usize, delay_samples: usize, amplitude: f32) -> Vec<f32> {
    let mut out = vec![0.0; len];
    if let Some(slot) = out.get_mut(delay_samples) {
        *slot = amplitude;
    }
    out
}

/// Generates a square wave at `freq_hz`.
pub fn square(freq_hz: f32, fs: f32, len: usize, amplitude: f32) -> Vec<f32> {
    let w = 2.0 * PI * freq_hz / fs;
    (0..len)
        .map(|n| {
            if (w * (n as f32)).sin() >= 0.0 {
                amplitude
            } else {
                -amplitude
            }
        })
        .collect()
}

/// Generates alternating silence/sine bursts for dynamics testing.
pub fn bursts(
    freq_hz: f32,
    fs: f32,
    burst_samples: usize,
    silence_samples: usize,
    repeats: usize,
    amplitude: f32,
) -> Vec<f32> {
    let mut out = Vec::with_capacity((burst_samples + silence_samples) * repeats);
    let w = 2.0 * PI * freq_hz / fs;
    for _ in 0..repeats {
        for n in 0..burst_samples {
            out.push(amplitude * (w * (n as f32)).sin());
        }
        out.resize(out.len() + silence_samples, 0.0);
    }
    out
}

/// Computes the RMS level in dBFS.
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return DB_FLOOR;
    }
    let sum_sq: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let rms = (sum_sq / (samples.len() as f64)).sqrt() as f32;
    lin_to_db(rms)
}

/// Computes the peak level in dBFS.
pub fn peak_db(samples: &[f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
    lin_to_db(peak)
}

/// Computes the crest factor (peak dB - RMS dB).
pub fn crest_db(samples: &[f32]) -> f32 {
    peak_db(samples) - rms_db(samples)
}

/// Computes Total Harmonic Distortion (THD) in dB relative to the fundamental bin.
pub fn thd_db(samples: &[f32], fundamental_hz: f32, fs: f32) -> f32 {
    let mags = fft_magnitude_linear(samples);
    if mags.is_empty() {
        return DB_FLOOR;
    }
    let n = (mags.len() - 1) * 2;
    let bin_hz = fs / (n as f32);
    let fund_bin = (fundamental_hz / bin_hz).round() as usize;
    let fund_mag = mags.get(fund_bin).copied().unwrap_or(0.0);
    if fund_mag <= 1e-12 {
        return DB_FLOOR;
    }
    let mut harm_sq = 0.0f64;
    let mut k = 2usize;
    while k * fund_bin < mags.len() {
        let m = mags.get(k * fund_bin).copied().unwrap_or(0.0);
        harm_sq += f64::from(m) * f64::from(m);
        k += 1;
    }
    let ratio = (harm_sq.sqrt() as f32) / fund_mag;
    lin_to_db(ratio)
}

/// Computes single-sided FFT magnitude spectrum in dBFS (Hann-windowed).
pub fn fft_magnitude_db(samples: &[f32]) -> Vec<f32> {
    fft_magnitude_linear(samples).into_iter().map(lin_to_db).collect()
}

fn fft_magnitude_linear(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    if n < 2 {
        return Vec::new();
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);
    let mut input: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / ((n - 1) as f32)).cos());
            x * w
        })
        .collect();
    let mut spectrum = r2c.make_output_vec();
    r2c.process(&mut input, &mut spectrum).expect("fft");
    let scale = 2.0 / (n as f32);
    spectrum.into_iter().map(|c| c.norm() * scale).collect()
}

/// Reads a mono/multi-channel WAV file (downmixed to mono `f32` in `-1.0..=1.0`).
pub fn read_wav_mono(path: &Path) -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.expect("sample")).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| (s.expect("sample") as f32) / max)
                .collect()
        }
    };
    let mono = if ch <= 1 {
        raw
    } else {
        raw.chunks_exact(ch)
            .map(|frame| frame.iter().sum::<f32>() / (ch as f32))
            .collect()
    };
    (mono, spec.sample_rate)
}

/// Writes mono `f32` samples to a 32-bit float WAV file.
pub fn write_wav_mono(path: &Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for &s in samples {
        writer.write_sample(s).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
}
