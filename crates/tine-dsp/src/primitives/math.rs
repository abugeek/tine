//! Pure-Rust `no_std` floating-point math helpers for coefficient design and DSP.
//!
//! All `f64` functions target near-machine-precision (~1e-13+) so biquad
//! designs and smoother coefficients match `std` without pulling in `libm`.

use core::f64::consts::{LN_2, LN_10, LOG10_E, PI};

const FRAC_PI_2: f64 = PI * 0.5;
const FRAC_PI_4: f64 = PI * 0.25;
const TWO_PI: f64 = PI * 2.0;

/// Square root for `f64` using IEEE-754 initial guess + Newton-Raphson.
#[inline]
pub fn sqrt_f64(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return if x == 0.0 { 0.0 } else { f64::NAN };
    }
    // Fast initial estimate via bit-level exponent halving.
    let mut y = f64::from_bits((x.to_bits() >> 1) + 0x1ff8_0000_0000_0000);
    // 5 Newton-Raphson iterations suffice for full 53-bit f64 precision.
    y = f64::midpoint(y, x / y);
    y = f64::midpoint(y, x / y);
    y = f64::midpoint(y, x / y);
    y = f64::midpoint(y, x / y);
    y = f64::midpoint(y, x / y);
    y
}

/// Square root for `f32`.
#[inline]
pub fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 || !x.is_finite() {
        return if x == 0.0 { 0.0 } else { f32::NAN };
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1fc0_0000);
    y = f32::midpoint(y, x / y);
    y = f32::midpoint(y, x / y);
    y = f32::midpoint(y, x / y);
    y
}

/// Floor for `f64` without `std`.
#[inline]
pub fn floor_f64(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let trunc = (x as i64) as f64;
    if x < trunc { trunc - 1.0 } else { trunc }
}

/// Natural exponential `e^x` for `f64`.
#[inline]
pub fn exp_f64(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.78 {
        return f64::INFINITY;
    }
    if x < -745.13 {
        return 0.0;
    }

    // Range reduction: x = k * ln(2) + r, |r| <= ln(2)/2
    let kf = floor_f64(x / LN_2 + 0.5);
    let k = kf as i64;
    let r = x - kf * LN_2;

    // Degree-11 Taylor polynomial for exp(r) on [-ln(2)/2, ln(2)/2]
    let r2 = r * r;
    let poly = 1.0
        + r * (1.0
            + r * (0.5
                + r * (1.0 / 6.0
                    + r * (1.0 / 24.0
                        + r * (1.0 / 120.0
                            + r * (1.0 / 720.0
                                + r * (1.0 / 5040.0
                                    + r * (1.0 / 40320.0
                                        + r * (1.0 / 362_880.0
                                            + r * (1.0 / 3_628_800.0 + r * (1.0 / 39_916_800.0)))))))))));
    let _ = r2;

    // Scale by 2^k
    if (-1022..=1023).contains(&k) {
        let scale = f64::from_bits(((k + 1023) as u64) << 52);
        poly * scale
    } else {
        // Subnormal range
        let half_k = k / 2;
        let rem_k = k - half_k;
        let s1 = f64::from_bits(((half_k + 1023) as u64) << 52);
        let s2 = f64::from_bits(((rem_k + 1023) as u64) << 52);
        (poly * s1) * s2
    }
}

/// Natural logarithm `ln(x)` for `f64` (`x > 0`).
#[inline]
pub fn ln_f64(x: f64) -> f64 {
    if x <= 0.0 || x.is_nan() {
        return if x == 0.0 { f64::NEG_INFINITY } else { f64::NAN };
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }

    let bits = x.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff).cast_signed() - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);

    // Adjust mantissa to [sqrt(0.5), sqrt(2)] for faster convergence
    if m > core::f64::consts::SQRT_2 {
        m *= 0.5;
        exp += 1;
    }

    let u = (m - 1.0) / (m + 1.0);
    let u2 = u * u;
    let series = u
        * (1.0
            + u2 * (1.0 / 3.0
                + u2 * (1.0 / 5.0
                    + u2 * (1.0 / 7.0
                        + u2 * (1.0 / 9.0
                            + u2 * (1.0 / 11.0 + u2 * (1.0 / 13.0 + u2 * (1.0 / 15.0 + u2 * (1.0 / 17.0)))))))));

    2.0 * series + (exp as f64) * LN_2
}

/// Base-10 logarithm `log10(x)` for `f64`.
#[inline]
pub fn log10_f64(x: f64) -> f64 {
    ln_f64(x) * LOG10_E
}

/// Base-10 exponential `10^x` for `f64`.
#[inline]
pub fn pow10_f64(x: f64) -> f64 {
    exp_f64(x * LN_10)
}

/// Simultaneous `(sin(x), cos(x))` for `f64`.
#[inline]
pub fn sin_cos_f64(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }

    // Reduce x into [-pi, pi]
    let mut xr = x - TWO_PI * floor_f64((x + PI) / TWO_PI);
    // Reduce into [-pi/2, pi/2] by quadrant
    let q = floor_f64(xr / FRAC_PI_2 + 0.5) as i64;
    xr -= (q as f64) * FRAC_PI_2;

    // Now |xr| <= pi/4. Evaluate sin(xr) and cos(xr) via Taylor series.
    let x2 = xr * xr;
    let s = xr
        * (1.0
            - x2 * (1.0 / 6.0
                - x2 * (1.0 / 120.0
                    - x2 * (1.0 / 5040.0
                        - x2 * (1.0 / 362_880.0 - x2 * (1.0 / 39_916_800.0 - x2 * (1.0 / 6_227_020_800.0)))))));
    let c = 1.0
        - x2 * (0.5
            - x2 * (1.0 / 24.0
                - x2 * (1.0 / 720.0
                    - x2 * (1.0 / 40320.0
                        - x2 * (1.0 / 3_628_800.0 - x2 * (1.0 / 479_001_600.0 - x2 * (1.0 / 87_178_291_200.0)))))));
    let _ = FRAC_PI_4;

    match q.rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_matches_std() {
        for &x in &[0.001, 0.25, 1.0, 2.0, 10.0, 48000.0, 1e-6, 1e6] {
            let rel = (sqrt_f64(x) - x.sqrt()).abs() / x.sqrt();
            assert!(rel < 1e-13, "sqrt_f64({x}) rel={rel}");
        }
        for &x in &[0.001f32, 0.5, 1.0, 3.5, 100.0] {
            let rel = (sqrt_f32(x) - x.sqrt()).abs() / x.sqrt();
            assert!(rel < 1e-6, "sqrt_f32({x}) rel={rel}");
        }
        for &x in &[-20.0, -5.0, -0.5, 0.0, 0.5, 5.0, 20.0] {
            let rel = (exp_f64(x) - x.exp()).abs() / x.exp();
            assert!(rel < 1e-13, "exp_f64({x}) rel={rel}");
        }
        for &x in &[1e-6, 0.1, 0.5, 1.0, 2.0, 10.0, 1000.0] {
            let err = (ln_f64(x) - x.ln()).abs();
            assert!(err < 1e-13, "ln_f64({x}) err={err}");
            let err10 = (log10_f64(x) - x.log10()).abs();
            assert!(err10 < 1e-13, "log10_f64({x}) err={err10}");
        }
        for i in -200..=200 {
            let x = (i as f64) * 0.05;
            let (s, c) = sin_cos_f64(x);
            assert!((s - x.sin()).abs() < 1e-13, "sin({x})");
            assert!((c - x.cos()).abs() < 1e-13, "cos({x})");
        }
    }
}
