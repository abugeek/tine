//! Light & Dark "Tine Deck 2.3" visual tokens, spacing/radius/type scales, and WCAG contrast helpers.

use rune::Color;
use rune::color::{oklch, oklcha};
use rune::tok::{radius, space, type_};

/// Active visual appearance mode for the Tine mixer console.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Matte charcoal DJ mixer faceplate with porcelain-white knobs ("Decky Midnight").
    #[default]
    Dark,
    /// Clean porcelain-white studio mixer faceplate with cobalt-blue accents ("Decky Daylight").
    Light,
}

impl ThemeMode {
    /// Toggles between [`ThemeMode::Dark`] and [`ThemeMode::Light`].
    #[must_use]
    pub const fn toggle(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    /// `true` when [`ThemeMode::Light`] is active.
    #[must_use]
    pub const fn is_light(self) -> bool {
        matches!(self, Self::Light)
    }
}

/// Tine Deck 2.3 color palette.
///
/// Supports both a matte dark console mode (`Decky Midnight`) and a clean white hardware
/// faceplate mode (`Decky Daylight`), pairing tactile cue-orange pads with electric cobalt
/// rotary indicators and crisp porcelain-white knob caps.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Window / root chassis background.
    pub bg: Color,
    /// Primary card / mixer faceplate surface.
    pub surface: Color,
    /// Elevated surface (controls, popovers, modal cards).
    pub surface_raised: Color,
    /// Hover / pressed tint for neutral controls.
    pub surface_hover: Color,
    /// Recessed well (OLED scope background, slider tracks, switch wells).
    pub well: Color,
    /// Hairline 1 pt border / channel divider.
    pub border: Color,
    /// Emphasized border (focused cards, popovers, hardware keycaps).
    pub border_strong: Color,
    /// Primary high-contrast foreground text.
    pub text: Color,
    /// Secondary / muted foreground text (still ≥ 4.5:1 against `bg`, `surface`, `surface_raised`).
    pub text_dim: Color,
    /// Primary interactive / active CUE accent: warm cue orange.
    pub accent: Color,
    /// Bright end of the cue-orange gradient.
    pub accent_hi: Color,
    /// Deep end of the cue-orange gradient.
    pub accent_lo: Color,
    /// Translucent accent halo for the EQ curve glow and focus rings.
    pub accent_glow: Color,
    /// Quiet accent wash for selected-but-secondary states.
    pub accent_soft: Color,
    /// Accent-colored text that stays ≥ 4.5:1 on `surface` / `surface_raised`.
    pub accent_text: Color,
    /// High-contrast text/icon color used on `accent` / cue-orange controls.
    pub text_on_accent: Color,
    /// Live spectrum bars inside the OLED scope.
    pub spectrum: Color,
    /// Safe signal meter green (`< -12 dBFS`).
    pub meter_green: Color,
    /// Nominal speech / warm meter amber (`-12..-3 dBFS`).
    pub meter_amber: Color,
    /// Peak / clipping meter red (`> -3 dBFS`).
    pub meter_red: Color,
    /// Healthy status (virtual mic ready).
    pub success: Color,
    /// Destructive / error / cable-missing indicator.
    pub danger: Color,
    /// Rotary knob outer porcelain cap fill.
    pub knob_outer: Color,
    /// Rotary knob inner concave powder-blue disc fill.
    pub knob_inner: Color,
    /// Rotary knob subtle resting rim border.
    pub knob_ring: Color,
    /// Rotary knob active electric-blue outer ring.
    pub knob_ring_active: Color,
    /// Rotary knob orbiting cobalt-blue position dot.
    pub knob_dot: Color,
    /// Lit radial perimeter tick dot on a rotary dial.
    pub knob_tick_on: Color,
    /// Unlit radial perimeter tick dot on a rotary dial.
    pub knob_tick_off: Color,
    /// Electric cobalt blue for active FX pills, switch thumbs, and secondary rings.
    pub deck_blue: Color,
    /// Soft translucent cobalt-blue wash for selected secondary pills.
    pub deck_blue_soft: Color,
    /// Cobalt-blue text meeting ≥ 4.5:1 contrast on `surface` / `surface_raised`.
    pub deck_blue_text: Color,
    /// Backdrop scrim behind modal overlays.
    pub scrim: Color,
}

/// A handle to a [`Palette`]: one static palette shared by every view, passed
/// around by value (8 bytes) and read through `Deref` (`t.accent`).
#[derive(Clone, Copy, Debug)]
pub struct TineTheme(&'static Palette);

impl std::ops::Deref for TineTheme {
    type Target = Palette;

    fn deref(&self) -> &Palette {
        self.0
    }
}

impl Default for TineTheme {
    fn default() -> Self {
        Self::dark()
    }
}

static EMBER: std::sync::LazyLock<Palette> = std::sync::LazyLock::new(|| Palette {
    bg: oklch(0.165, 0.012, 260.0),
    surface: oklch(0.215, 0.015, 260.0),
    surface_raised: oklch(0.265, 0.018, 260.0),
    surface_hover: oklch(0.31, 0.022, 260.0),
    well: oklch(0.14, 0.010, 260.0),
    border: oklcha(0.92, 0.015, 260.0, 0.12),
    border_strong: oklcha(0.92, 0.015, 260.0, 0.22),
    text: oklch(0.96, 0.008, 255.0),
    text_dim: oklch(0.76, 0.015, 255.0),
    accent: oklch(0.74, 0.165, 48.0),
    accent_hi: oklch(0.85, 0.14, 75.0),
    accent_lo: oklch(0.67, 0.2, 28.0),
    accent_glow: oklcha(0.74, 0.165, 48.0, 0.30),
    accent_soft: oklcha(0.74, 0.165, 48.0, 0.16),
    accent_text: oklch(0.80, 0.14, 58.0),
    text_on_accent: oklch(0.2, 0.03, 35.0),
    spectrum: oklcha(0.62, 0.16, 260.0, 0.24),
    meter_green: oklch(0.78, 0.15, 158.0),
    meter_amber: oklch(0.85, 0.14, 75.0),
    meter_red: oklch(0.66, 0.21, 25.0),
    success: oklch(0.78, 0.15, 158.0),
    danger: oklch(0.68, 0.2, 25.0),
    knob_outer: oklch(0.985, 0.004, 255.0),
    knob_inner: oklch(0.942, 0.026, 258.0),
    knob_ring: oklcha(0.92, 0.015, 260.0, 0.24),
    knob_ring_active: oklch(0.56, 0.22, 262.0),
    knob_dot: oklch(0.52, 0.23, 262.0),
    knob_tick_on: oklch(0.62, 0.21, 262.0),
    knob_tick_off: oklcha(0.85, 0.015, 260.0, 0.28),
    deck_blue: oklch(0.52, 0.23, 262.0),
    deck_blue_soft: oklcha(0.56, 0.21, 262.0, 0.20),
    deck_blue_text: oklch(0.76, 0.13, 258.0),
    scrim: oklcha(0.08, 0.015, 260.0, 0.76),
});

static DAYLIGHT: std::sync::LazyLock<Palette> = std::sync::LazyLock::new(|| Palette {
    bg: oklch(0.945, 0.008, 255.0),
    surface: oklch(0.992, 0.003, 255.0),
    surface_raised: oklch(0.93, 0.010, 255.0),
    surface_hover: oklch(0.89, 0.014, 255.0),
    well: oklch(0.905, 0.012, 255.0),
    border: oklcha(0.25, 0.02, 255.0, 0.12),
    border_strong: oklcha(0.25, 0.02, 255.0, 0.22),
    text: oklch(0.21, 0.02, 255.0),
    text_dim: oklch(0.42, 0.02, 255.0),
    accent: oklch(0.72, 0.175, 45.0),
    accent_hi: oklch(0.80, 0.15, 65.0),
    accent_lo: oklch(0.67, 0.19, 32.0),
    accent_glow: oklcha(0.72, 0.175, 45.0, 0.32),
    accent_soft: oklcha(0.72, 0.175, 45.0, 0.18),
    accent_text: oklch(0.43, 0.14, 42.0),
    text_on_accent: oklch(0.18, 0.02, 35.0),
    spectrum: oklcha(0.54, 0.20, 262.0, 0.22),
    meter_green: oklch(0.62, 0.17, 158.0),
    meter_amber: oklch(0.72, 0.16, 75.0),
    meter_red: oklch(0.58, 0.21, 25.0),
    success: oklch(0.58, 0.16, 158.0),
    danger: oklch(0.56, 0.21, 25.0),
    knob_outer: oklch(0.995, 0.002, 255.0),
    knob_inner: oklch(0.945, 0.026, 258.0),
    knob_ring: oklcha(0.35, 0.02, 255.0, 0.20),
    knob_ring_active: oklch(0.52, 0.23, 262.0),
    knob_dot: oklch(0.50, 0.24, 262.0),
    knob_tick_on: oklch(0.50, 0.24, 262.0),
    knob_tick_off: oklcha(0.35, 0.02, 255.0, 0.22),
    deck_blue: oklch(0.50, 0.24, 262.0),
    deck_blue_soft: oklcha(0.52, 0.22, 262.0, 0.14),
    deck_blue_text: oklch(0.45, 0.21, 262.0),
    scrim: oklcha(0.20, 0.02, 255.0, 0.45),
});

impl TineTheme {
    /// Returns Tine's dark console palette ("Decky Midnight").
    #[must_use]
    pub fn dark() -> Self {
        Self(&EMBER)
    }

    /// Returns Tine's light studio faceplate palette ("Decky Daylight").
    #[must_use]
    pub fn light() -> Self {
        Self(&DAYLIGHT)
    }

    /// Returns the palette matching `mode`.
    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
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
    /// The "Tine" wordmark in the header lockup.
    pub const WORDMARK: TextStyle = TextStyle {
        tracking: -0.3,
        ..TextStyle::new(18.0, 700.0)
    };
    /// Card headings ("Golden Line", "Pro Controls").
    pub const HEADING: TextStyle = TextStyle {
        tracking: -0.1,
        ..TextStyle::new(15.0, 650.0)
    };
    /// Small uppercase-style eyebrow labels (zones, meter captions).
    pub const EYEBROW: TextStyle = TextStyle {
        tracking: 0.6,
        ..TextStyle::new(10.5, 600.0)
    };
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

    fn assert_palette_wcag_aa(name: &str, t: TineTheme) {
        let surfaces = [t.bg, t.surface, t.surface_raised];
        for bg in surfaces {
            assert!(
                contrast_ratio(t.text, bg) >= 4.5,
                "{name}: text contrast against surface below 4.5:1: {}",
                contrast_ratio(t.text, bg)
            );
            assert!(
                contrast_ratio(t.text_dim, bg) >= 4.5,
                "{name}: text_dim contrast against surface below 4.5:1: {}",
                contrast_ratio(t.text_dim, bg)
            );
        }
        for fill in [t.accent, t.accent_hi, t.accent_lo] {
            assert!(
                contrast_ratio(t.text_on_accent, fill) >= 4.5,
                "{name}: text_on_accent contrast against ember fill below 4.5:1: {}",
                contrast_ratio(t.text_on_accent, fill)
            );
        }
        for bg in [t.surface, t.surface_raised] {
            assert!(
                contrast_ratio(t.accent_text, bg) >= 4.5,
                "{name}: accent_text contrast below 4.5:1: {}",
                contrast_ratio(t.accent_text, bg)
            );
            assert!(
                contrast_ratio(t.deck_blue_text, bg) >= 4.5,
                "{name}: deck_blue_text contrast below 4.5:1: {}",
                contrast_ratio(t.deck_blue_text, bg)
            );
        }
        assert!(
            contrast_ratio(Color::WHITE, t.deck_blue) >= 4.5,
            "{name}: white text contrast on deck_blue below 4.5:1: {}",
            contrast_ratio(Color::WHITE, t.deck_blue)
        );
    }

    #[test]
    fn dark_and_light_theme_text_surfaces_meet_wcag_aa() {
        assert_palette_wcag_aa("dark", TineTheme::dark());
        assert_palette_wcag_aa("light", TineTheme::light());
    }
}
