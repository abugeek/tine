//! Decibel <-> linear conversions with a −120 dB floor.

use super::math::{log10_f64, pow10_f64};

/// Minimum decibel floor returned by [`lin_to_db`].
pub const DB_FLOOR: f32 = -120.0;

/// Linear amplitude corresponding to [`DB_FLOOR`] (`10^(-120/20) = 1e-6`).
pub const LIN_FLOOR: f32 = 1e-6;

/// Fast `20 * log10(abs)` in `f32` for positive finite `abs` (no floor check).
#[inline]
#[must_use]
pub fn fast_lin_to_db_raw(abs: f32) -> f32 {
    let bits = abs.to_bits();
    let exp = (bits >> 23).cast_signed() - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let u = (m - 1.0) / (m + 1.0);
    let u2 = u * u;
    let ln_m = 2.0 * u * (1.0 + u2 * (1.0 / 3.0 + u2 * (1.0 / 5.0 + u2 * (1.0 / 7.0))));
    // 20 / ln(10) = 8.68589, 20 * log10(2) = 6.0206
    (exp as f32) * 6.020_6 + ln_m * 8.685_89
}

/// Fast `lin_to_db` in `f32` (max error < 0.01 dB over `[-119, +24] dB`), floored at [`DB_FLOOR`].
#[inline]
#[must_use]
pub fn fast_lin_to_db(lin: f32) -> f32 {
    let abs = lin.abs();
    if !abs.is_finite() || abs <= LIN_FLOOR {
        return DB_FLOOR;
    }
    fast_lin_to_db_raw(abs)
}

/// Fast `10 * log10(sq)` in `f32` for squared linear amplitude, floored at [`DB_FLOOR`].
#[inline]
#[must_use]
pub fn fast_sq_to_db(sq: f32) -> f32 {
    const LIN_FLOOR_SQ: f32 = LIN_FLOOR * LIN_FLOOR;
    if !sq.is_finite() || sq <= LIN_FLOOR_SQ {
        return DB_FLOOR;
    }
    0.5 * fast_lin_to_db_raw(sq)
}

/// Fast `db_to_lin` in `f32` (max error < 0.01 dB over `[-119, +24] dB`).
#[inline]
#[must_use]
pub fn fast_db_to_lin(db: f32) -> f32 {
    if !db.is_finite() || db < DB_FLOOR {
        return 0.0;
    }
    let db = db.min(48.0);
    // x = db * ln(10) / 20
    let x = db * 0.115_129_255;
    let k_f = x * core::f32::consts::LOG2_E;
    let k = (k_f + if k_f >= 0.0 { 0.5 } else { -0.5 }) as i32;
    let r = x - (k as f32) * core::f32::consts::LN_2;
    let poly = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0)))));
    let scale = f32::from_bits(((k + 127) as u32) << 23);
    poly * scale
}

/// Converts decibels to linear amplitude.
///
/// Values below [`DB_FLOOR`] or non-finite values return `0.0`.
#[inline]
#[must_use]
pub fn db_to_lin(db: f32) -> f32 {
    if !db.is_finite() || db < DB_FLOOR {
        return 0.0;
    }
    pow10_f64(f64::from(db) * 0.05) as f32
}

/// Converts a linear amplitude to decibels, floored at [`DB_FLOOR`] (−120 dB).
#[inline]
#[must_use]
pub fn lin_to_db(lin: f32) -> f32 {
    let abs = lin.abs();
    if !abs.is_finite() || abs <= LIN_FLOOR {
        return DB_FLOOR;
    }
    (20.0 * log10_f64(f64::from(abs))) as f32
}

/// Converts decibels to linear amplitude in `f64`.
#[inline]
#[must_use]
pub fn db_to_lin_f64(db: f64) -> f64 {
    if !db.is_finite() || db < f64::from(DB_FLOOR) {
        return 0.0;
    }
    pow10_f64(db * 0.05)
}

/// Converts a linear amplitude to decibels in `f64`, floored at −120 dB.
#[inline]
#[must_use]
pub fn lin_to_db_f64(lin: f64) -> f64 {
    let abs = lin.abs();
    if !abs.is_finite() || abs <= 1e-6 {
        return f64::from(DB_FLOOR);
    }
    20.0 * log10_f64(abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_floor() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_lin(-6.0) - 0.501_187_2).abs() < 1e-5);
        assert!((db_to_lin(20.0) - 10.0).abs() < 1e-5);
        assert_eq!(db_to_lin(-121.0), 0.0);
        assert_eq!(db_to_lin(f32::NAN), 0.0);

        assert_eq!(lin_to_db(0.0), DB_FLOOR);
        assert_eq!(lin_to_db(1e-8), DB_FLOOR);
        assert!((lin_to_db(1.0) - 0.0).abs() < 1e-5);
        assert!((lin_to_db(-0.5) - (-6.0206)).abs() < 1e-3);

        for db in [-100.0f32, -60.0, -20.0, -6.0, 0.0, 6.0, 24.0] {
            let back = lin_to_db(db_to_lin(db));
            assert!((back - db).abs() < 1e-4, "db={db}, back={back}");
        }
    }

    #[test]
    fn fast_db_conversions_accurate_and_finite() {
        for i in -1190..=240 {
            let db = (i as f32) * 0.1;
            let exact_lin = db_to_lin(db);
            let fast_lin = fast_db_to_lin(db);
            let err_db = (lin_to_db(fast_lin) - db).abs();
            assert!(
                err_db < 0.01,
                "fast_db_to_lin({db}): err={err_db} dB (exact={exact_lin}, fast={fast_lin})"
            );

            let exact_db = lin_to_db(exact_lin);
            let fast_db = fast_lin_to_db(exact_lin);
            assert!(
                (fast_db - exact_db).abs() < 0.01,
                "fast_lin_to_db({exact_lin}): exact={exact_db}, fast={fast_db}"
            );

            let fast_sq = fast_sq_to_db(exact_lin * exact_lin);
            assert!(
                (fast_sq - exact_db).abs() < 0.01,
                "fast_sq_to_db: exact={exact_db}, fast={fast_sq}"
            );
        }

        // Non-finite, zero, and out-of-range inputs are handled safely and finitely.
        for zero in [0.0f32, -0.0] {
            assert_eq!(fast_lin_to_db(zero), DB_FLOOR);
            assert_eq!(fast_sq_to_db(zero), DB_FLOOR);
            assert!((fast_db_to_lin(zero) - 1.0).abs() < 1e-6);
        }
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(fast_lin_to_db(bad), DB_FLOOR);
            assert_eq!(fast_sq_to_db(bad), DB_FLOOR);
            assert_eq!(fast_db_to_lin(bad), 0.0);
        }
        assert_eq!(fast_db_to_lin(-121.0), 0.0);
        assert!(fast_db_to_lin(100.0).is_finite());
    }
}
