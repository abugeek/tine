//! Frequency response curve evaluation for the UI canvas.

use core::f64::consts::FRAC_1_SQRT_2;

use tine_protocol::params::{DspParams, EQ_BAND_COUNT, FilterType};

use crate::primitives::biquad::Coeffs;

const SKIP_GAIN_DB: f32 = 0.01;
const TOTAL_FILTERS: usize = 1 + EQ_BAND_COUNT;

#[inline]
const fn is_gain_filter(ft: FilterType) -> bool {
    matches!(ft, FilterType::LowShelf | FilterType::HighShelf | FilterType::Peaking)
}

#[inline]
fn sanitize_finite(val: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if val.is_finite() { val.clamp(min, max) } else { fallback }
}

fn collect_coeffs(params: &DspParams, fs_f64: f64) -> [Option<Coeffs>; TOTAL_FILTERS] {
    let mut out = [None; TOTAL_FILTERS];

    if params.hpf_enabled
        && let Some(slot) = out.first_mut()
    {
        let hpf_freq = sanitize_finite(params.hpf_freq_hz, 20.0, 200.0, 80.0);
        *slot = Some(Coeffs::design(
            FilterType::HighPass,
            fs_f64,
            f64::from(hpf_freq),
            FRAC_1_SQRT_2,
            0.0,
        ));
    }

    for (slot, (((&ft, &freq), &gain), &q)) in out.iter_mut().skip(1).zip(
        params
            .eq_type
            .iter()
            .zip(params.eq_freq_hz.iter())
            .zip(params.eq_gain_db.iter())
            .zip(params.eq_q.iter()),
    ) {
        let gain_c = sanitize_finite(gain, -18.0, 18.0, 0.0);
        if is_gain_filter(ft) && gain_c.abs() < SKIP_GAIN_DB {
            continue;
        }
        let freq_c = sanitize_finite(freq, 20.0, 20_000.0, 1_000.0);
        let q_c = sanitize_finite(q, 0.1, 10.0, 1.0);
        *slot = Some(Coeffs::design(
            ft,
            fs_f64,
            f64::from(freq_c),
            f64::from(q_c),
            f64::from(gain_c),
        ));
    }

    out
}

#[inline]
fn eval_coeffs(coeffs: &[Option<Coeffs>; TOTAL_FILTERS], freq_hz: f64, fs_f64: f64) -> f32 {
    let mut sum_db = 0.0f64;
    for c in coeffs.iter().flatten() {
        sum_db += c.magnitude_db(freq_hz, fs_f64);
    }
    sum_db as f32
}

/// Combined magnitude of stage 1 HPF + 5 EQ bands for the current `params`, in dB.
#[must_use]
pub fn response_db(params: &DspParams, fs: f32, freq_hz: f32) -> f32 {
    if !fs.is_finite() || fs <= 0.0 || !freq_hz.is_finite() || freq_hz <= 0.0 {
        return 0.0;
    }
    let fs_f64 = f64::from(fs);
    let coeffs = collect_coeffs(params, fs_f64);
    eval_coeffs(&coeffs, f64::from(freq_hz), fs_f64)
}

/// Fills `out` with the combined frequency response (in dB) at `freqs`, with zero allocations.
pub fn response_curve(params: &DspParams, fs: f32, freqs: &[f32], out: &mut [f32]) {
    if !fs.is_finite() || fs <= 0.0 {
        out.fill(0.0);
        return;
    }
    let fs_f64 = f64::from(fs);
    let coeffs = collect_coeffs(params, fs_f64);
    for (&f, slot) in freqs.iter().zip(out.iter_mut()) {
        *slot = if f.is_finite() && f > 0.0 {
            eval_coeffs(&coeffs, f64::from(f), fs_f64)
        } else {
            0.0
        };
    }
}
