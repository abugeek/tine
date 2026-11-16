//! Decibel <-> linear conversions with a −120 dB floor.

use super::math::{log10_f64, pow10_f64};

/// Minimum decibel floor returned by [`lin_to_db`].
pub const DB_FLOOR: f32 = -120.0;

/// Linear amplitude corresponding to [`DB_FLOOR`] (`10^(-120/20) = 1e-6`).
pub const LIN_FLOOR: f32 = 1e-6;

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
}
