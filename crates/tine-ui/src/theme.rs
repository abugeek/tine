//! Dark-mode visual tokens, spacing/radius/type scales, and WCAG contrast helpers.

use rune::Color;
use rune::color::{oklch, oklcha};
use rune::tok::{self, radius, space, type_};

/// Tine dark-mode color palette (v0.1).
///
/// Built on [`rune::tok::theme(true)`] with audio-specific meter and accent-glow tokens.
#[derive(Clone, Copy, Debug)]
pub struct TineTheme {
    /// Window / root background.
    pub bg: Color,
    /// Primary card / panel surface.
    pub surface: Color,
    /// Elevated surface (headers, popovers, modal cards).
    pub surface_raised: Color,
    /// Hairline 1 pt border.
    pub border: Color,
    /// Primary high-contrast foreground text.
    pub text: Color,
    /// Secondary / muted foreground text (still ≥ 4.5:1 against `surface_raised`).
    pub text_dim: Color,
    /// Primary interactive accent (Golden Line / active controls).
    pub accent: Color,
    /// Translucent accent halo for the Golden Line spline glow.
    pub accent_glow: Color,
    /// Dark text/icon color used on bright accent-filled pills.
    pub text_on_accent: Color,
    /// Safe signal meter green (`< -12 dBFS`).
    pub meter_green: Color,
    /// Nominal speech / warm meter amber (`-12..-3 dBFS`).
    pub meter_amber: Color,
    /// Peak / clipping meter red (`> -3 dBFS`).
    pub meter_red: Color,
    /// Destructive / error / APO-missing indicator.
    pub danger: Color,
}

impl Default for TineTheme {
    fn default() -> Self {
        Self::dark()
    }
}

impl TineTheme {
    /// Returns Tine's v0.1 dark theme tokens.
    #[must_use]
    pub fn dark() -> Self {
        let base = tok::theme(true);
        Self {
            bg: base.bg,
            surface: base.card,
            surface_raised: oklch(0.275, 0.006, 265.0),
            border: oklcha(1.0, 0.0, 0.0, 0.10),
            text: base.text,
            // Slightly brighter than Rune's default secondary so contrast >= 4.5:1 even on surface_raised.
            text_dim: oklch(0.76, 0.008, 265.0),
            // Warm golden-amber accent for "The Golden Line"
            accent: oklch(0.78, 0.15, 78.0),
            accent_glow: oklcha(0.78, 0.15, 78.0, 0.28),
            text_on_accent: oklch(0.16, 0.01, 265.0),
            meter_green: oklch(0.72, 0.16, 148.0),
            meter_amber: oklch(0.78, 0.16, 78.0),
            meter_red: oklch(0.65, 0.21, 25.0),
            danger: oklch(0.66, 0.20, 25.0),
        }
    }
}

/// Spacing scale re-exported from [`rune::tok::space`].
pub mod spacing {
    use super::space;

    /// 4 pt micro gap.
    pub const XS: f32 = space::S1;
    /// 8 pt compact gap.
    pub const SM: f32 = space::S2;
    /// 12 pt standard gap.
    pub const MD: f32 = space::S3;
    /// 16 pt section padding.
    pub const LG: f32 = 16.0;
    /// 24 pt outer margin.
    pub const XL: f32 = space::S6;
}

/// Corner radius scale re-exported from [`rune::tok::radius`].
pub mod radii {
    use super::radius;

    /// 6 pt small control radius.
    pub const SM: f32 = radius::SM;
    /// 8 pt standard card / button radius.
    pub const MD: f32 = radius::MD;
    /// 12 pt panel / modal radius.
    pub const LG: f32 = radius::LG;
    /// Pill radius.
    pub const PILL: f32 = 999.0;
}

/// Type scale re-exported from [`rune::tok::type_`].
pub mod typography {
    use rune::TextStyle;

    use super::type_;

    /// Window / section title style.
    pub const TITLE: TextStyle = type_::TITLE;
    /// Standard body copy.
    pub const BODY: TextStyle = type_::BODY;
    /// Compact control label.
    pub const LABEL: TextStyle = type_::LABEL;
    /// Caption / numeric readout.
    pub const CAPTION: TextStyle = type_::MONO_CAPTION;
}

/// WCAG 2.1 relative luminance of a linear-light [`Color`].
///
/// [`rune::Color`] stores linear RGB channels (`0.0..=1.0`), so no sRGB decoding is needed.
#[must_use]
pub fn relative_luminance(c: Color) -> f32 {
    let r = c.r.clamp(0.0, 1.0);
    let g = c.g.clamp(0.0, 1.0);
    let b = c.b.clamp(0.0, 1.0);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Computes the WCAG 2.1 contrast ratio (`1.0..=21.0`) between `fg` and `bg`.
#[must_use]
pub fn contrast_ratio(fg: Color, bg: Color) -> f32 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_text_surfaces_meet_wcag_aa() {
        let t = TineTheme::dark();
        let surfaces = [t.bg, t.surface, t.surface_raised];
        for bg in surfaces {
            assert!(
                contrast_ratio(t.text, bg) >= 4.5,
                "text contrast against surface below 4.5:1: {}",
                contrast_ratio(t.text, bg)
            );
            assert!(
                contrast_ratio(t.text_dim, bg) >= 4.5,
                "text_dim contrast against surface below 4.5:1: {}",
                contrast_ratio(t.text_dim, bg)
            );
        }
        assert!(
            contrast_ratio(t.text_on_accent, t.accent) >= 4.5,
            "text_on_accent contrast against accent below 4.5:1: {}",
            contrast_ratio(t.text_on_accent, t.accent)
        );
    }
}
