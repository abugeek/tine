//! Shared Tine Deck 2.3 building blocks: brand lockup, line icons, buttons, segmented
//! controls, toggle switches, status chips, and Decky-style hardware mixer primitives
//! (rotary dials with radial tick rings & spring-rotated caps, CUE pads, and deck switches).
//!
//! Every view composes these so a control looks and behaves the same wherever it
//! appears. Keys, roles and labels are always the caller's, so focus order and the
//! accessibility tree stay owned by the views.

use rune::{El, Icon, LineCap, Shape, Spring, col, el, icon, row, text};

use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

/// The fork of the app icon (single color; the plate and capsule are composed around it
/// in [`brand_mark`]). Same geometry as `assets/brand/tine-icon.svg`.
pub static FORK: Icon = Icon {
    name: "tine-fork",
    view: 256.0,
    body: r#"<path d="M84 58V128A44 44 0 0 0 172 128V58M128 172V214" fill="none" stroke="currentColor" stroke-width="28" stroke-linecap="round"/>"#,
};

/// The app icon at `size` points: gradient plate, ivory fork and ember capsule, built
/// from native elements (no SVG rasterizer needed) to match `assets/brand/`.
#[must_use]
pub fn brand_mark(key: &'static str, size: f32, t: TineTheme) -> El {
    let u = size / 256.0;
    // `assets/brand` plate gradient: #352753 → #120D1F.
    let plate_top = rune::color::oklch(0.30, 0.075, 298.0);
    let plate_bottom = rune::color::oklch(0.17, 0.035, 300.0);
    el().key(key)
        .size(size, size)
        .radius(60.0 * u)
        .fill(plate_bottom)
        .linear_gradient(
            std::f32::consts::FRAC_PI_4 * 3.0,
            [(0.0, plate_top), (1.0, plate_bottom)],
        )
        .border(1.0, t.border_strong)
        .child(
            icon(&FORK, size)
                .key("fork")
                .absolute(0.0, 0.0)
                .color(rune::color::oklch(0.955, 0.012, 70.0)),
        )
        .child(
            el().key("capsule")
                .absolute(106.0 * u, 44.0 * u)
                .size(44.0 * u, 84.0 * u)
                .radius(22.0 * u)
                .fill(t.accent)
                .gradient(t.accent_hi, t.accent_lo),
        )
}

/// Line icons on a 24-unit grid, stroked in the element's color.
pub mod glyph {
    use rune::Icon;

    /// Microphone (mic selector).
    pub static MIC: Icon = Icon {
        name: "tine-mic",
        view: 24.0,
        body: r#"<g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0"/><path d="M12 17.5V21"/></g>"#,
    };

    /// Headphones ("Hear Myself" playback device).
    pub static HEADPHONES: Icon = Icon {
        name: "tine-headphones",
        view: 24.0,
        body: r#"<g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 16v-4a8 8 0 0 1 16 0v4"/><rect x="3.5" y="14" width="4" height="6.5" rx="1.6"/><rect x="16.5" y="14" width="4" height="6.5" rx="1.6"/></g>"#,
    };

    /// Down chevron (dropdowns, collapsed drawer).
    pub static CHEVRON_DOWN: Icon = Icon {
        name: "tine-chevron-down",
        view: 24.0,
        body: r#"<path d="M7 10l5 5 5-5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>"#,
    };

    /// Up chevron (expanded drawer).
    pub static CHEVRON_UP: Icon = Icon {
        name: "tine-chevron-up",
        view: 24.0,
        body: r#"<path d="M7 14l5-5 5 5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>"#,
    };

    /// Four-point sparkle (Auto-Tune).
    pub static SPARKLE: Icon = Icon {
        name: "tine-sparkle",
        view: 24.0,
        body: r#"<path d="M11 3.5l1.9 5.6 5.6 1.9-5.6 1.9L11 18.5l-1.9-5.6L3.5 11l5.6-1.9z M18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z" fill="currentColor"/>"#,
    };

    /// Mixer sliders (Pro Controls).
    pub static SLIDERS: Icon = Icon {
        name: "tine-sliders",
        view: 24.0,
        body: r#"<g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2.2"/><circle cx="9" cy="17" r="2.2"/></g>"#,
    };

    /// Power standby glyph (Decky Deck POWER / Tine On button).
    pub static POWER: Icon = Icon {
        name: "tine-power",
        view: 24.0,
        body: r#"<g fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round"><path d="M12 3.5v7.5"/><path d="M7.2 6.8a7 7 0 1 0 9.6 0"/></g>"#,
    };

    /// Sun glyph (Light mode).
    pub static SUN: Icon = Icon {
        name: "tine-sun",
        view: 24.0,
        body: r#"<g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2.5 12h2M19.5 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></g>"#,
    };

    /// Crescent moon glyph (Dark mode).
    pub static MOON: Icon = Icon {
        name: "tine-moon",
        view: 24.0,
        body: r#"<path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5a7.5 7.5 0 1 0 10 10z" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/>"#,
    };
}

/// Standard control height (buttons, pills, selectors).
pub const CONTROL_H: f32 = 32.0;

/// Visual weight of a [`button`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The single ember-filled call to action of a view.
    Primary,
    /// Neutral raised button.
    Secondary,
    /// Accent-tinted button for an "on" state that is not the view's main action.
    Selected,
}

/// The brand lockup: the app mark beside the "Tine" wordmark and subtle "DECK 2.3" tag.
#[must_use]
pub fn brand(t: TineTheme) -> El {
    row()
        .key("brand")
        .gap(spacing::SM)
        .center()
        .child(brand_mark("brand/mark", 28.0, t))
        .child(
            text("Tine")
                .key("brand/wordmark")
                .font(typography::WORDMARK)
                .color(t.text),
        )
        .child(
            row()
                .key("brand/deck-tag")
                .h(18.0)
                .px(6.0)
                .center()
                .radius(radii::SM)
                .fill(t.deck_blue_soft)
                .border(1.0, t.border)
                .child(text("DECK 2.3").font(typography::EYEBROW).color(t.deck_blue_text)),
        )
}

/// A small line icon in `color`.
#[must_use]
pub fn glyph_el(g: &'static Icon, size: f32, color: rune::Color) -> El {
    icon(g, size).color(color)
}

/// A focusable button. `lead` is an optional leading glyph.
#[must_use]
pub fn button(
    key: impl Into<String>,
    role: &'static str,
    label: impl Into<String>,
    tone: Tone,
    lead: Option<&'static Icon>,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let label = label.into();
    let fg = match tone {
        Tone::Primary => t.text_on_accent,
        Tone::Secondary => t.text,
        Tone::Selected => t.accent_text,
    };
    let mut b = row()
        .key(key)
        .role(role)
        .label(label.clone())
        .focusable(true)
        .h(CONTROL_H)
        .px(spacing::MD + 2.0)
        .gap(6.0)
        .center()
        .radius(radii::PILL)
        .press_scale(0.97)
        .cursor(rune::Cursor::Pointer)
        .on_press(on_press);
    b = match tone {
        Tone::Primary => b
            .fill(t.accent)
            .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, t.accent_hi), (1.0, t.accent_lo)])
            .shadow(4.0, 14.0, t.accent_glow)
            .hover(rune::color::oklcha(1.0, 0.0, 0.0, 0.12))
            .press(rune::color::oklcha(0.0, 0.0, 0.0, 0.12)),
        Tone::Secondary => b
            .fill(t.surface_raised)
            .border(1.0, t.border)
            .hover(t.surface_hover)
            .press(t.surface_hover),
        Tone::Selected => b
            .fill(t.accent_soft)
            .border(1.0, t.accent_glow)
            .hover(t.accent_soft)
            .press(t.accent_glow),
    };
    if let Some(g) = lead {
        b = b.child(glyph_el(g, 15.0, fg));
    }
    b.child(text(label).font(typography::LABEL).color(fg))
}

/// A recessed segmented-control track; children come from [`segment`].
#[must_use]
pub fn segmented(key: &'static str, role: &'static str, label: &'static str, t: TineTheme) -> El {
    row()
        .key(key)
        .role(role)
        .label(label)
        .h(CONTROL_H)
        .px(3.0)
        .gap(2.0)
        .center()
        .radius(radii::PILL)
        .fill(t.well)
        .border(1.0, t.border)
}

/// One option of a [`segmented`] control.
#[must_use]
pub fn segment(
    key: &'static str,
    role: &'static str,
    label: &'static str,
    selected: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let is_light = t.bg.r > 0.5;
    let s = row()
        .key(key)
        .role(role)
        .label(label)
        .checked(selected)
        .focusable(true)
        .h(CONTROL_H - 8.0)
        .px(spacing::MD)
        .center()
        .radius(radii::PILL)
        .press_scale(0.97)
        .cursor(rune::Cursor::Pointer)
        .on_press(on_press);
    if selected {
        s.fill(if is_light { t.surface } else { t.surface_hover })
            .border(1.0, t.border_strong)
            .shadow(1.0, 4.0, oklcha_black(if is_light { 0.12 } else { 0.35 }))
            .child(text(label).font(typography::LABEL).color(t.text))
    } else {
        s.hover(t.surface_raised)
            .child(text(label).font(typography::LABEL).color(t.text_dim))
    }
}

/// The track + knob of a toggle switch (no label), for embedding in a switch row.
#[must_use]
pub fn switch_track(key: impl Into<String>, on: bool, t: TineTheme) -> El {
    const W: f32 = 34.0;
    const H: f32 = 20.0;
    const KNOB: f32 = 14.0;
    let is_light = t.bg.r > 0.5;
    let knob = el()
        .key("knob")
        .size(KNOB, KNOB)
        .radius(KNOB / 2.0)
        .fill(if on { t.knob_outer } else { t.text_dim })
        .shadow(1.0, 3.0, oklcha_black(if is_light { 0.18 } else { 0.4 }));
    let pad = el().key("pad").grow(1.0);
    let mut track = row()
        .key(key)
        .size(W, H)
        .px(3.0)
        .center()
        .radius(H / 2.0)
        .animate_layout();
    track = if on {
        track
            .fill(t.accent)
            .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, t.accent_hi), (1.0, t.accent_lo)])
            .child(pad)
            .child(knob)
    } else {
        track.fill(t.surface_hover).border(1.0, t.border).child(knob).child(pad)
    };
    track
}

/// A labelled toggle switch: track + caption, `role="switch"` and `checked` set.
#[must_use]
pub fn switch(
    key: impl Into<String>,
    a11y_label: impl Into<String>,
    caption: impl Into<String>,
    on: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let key = key.into();
    let caption = caption.into();
    row()
        .key(key.clone())
        .role("switch")
        .checked(on)
        .label(a11y_label)
        .focusable(true)
        .h(CONTROL_H)
        .px(spacing::SM)
        .gap(spacing::SM)
        .center()
        .radius(radii::PILL)
        .press_scale(0.97)
        .cursor(rune::Cursor::Pointer)
        .hover(t.surface_raised)
        .on_press(on_press)
        .child(switch_track(format!("{key}/track"), on, t))
        .child(
            text(caption)
                .font(typography::LABEL)
                .color(if on { t.text } else { t.text_dim }),
        )
}

/// A status chip: colored dot + label on a quiet surface.
#[must_use]
pub fn status_chip(key: &'static str, label: impl Into<String>, dot: rune::Color, t: TineTheme) -> El {
    let label = label.into();
    row()
        .key(key)
        .role("status")
        .label(label.clone())
        .h(CONTROL_H)
        .px(spacing::MD)
        .gap(spacing::SM)
        .center()
        .radius(radii::PILL)
        .fill(t.well)
        .border(1.0, t.border)
        .child(status_dot(format!("{key}/dot"), dot))
        .child(text(label).font(typography::LABEL).color(t.text))
}

/// A glowing status dot.
#[must_use]
pub fn status_dot(key: impl Into<String>, color: rune::Color) -> El {
    el().key(key)
        .size(8.0, 8.0)
        .radius(4.0)
        .fill(color)
        .shadow(0.0, 8.0, rune::Color { a: 0.55, ..color })
}

/// A section card: the standard panel surface.
#[must_use]
pub fn card(key: &'static str, t: TineTheme) -> El {
    let is_light = t.bg.r > 0.5;
    rune::col()
        .key(key)
        .w_full()
        .radius(radii::LG)
        .continuous_corners()
        .fill(t.surface)
        .border(1.0, t.border)
        .shadow(8.0, 24.0, oklcha_black(if is_light { 0.08 } else { 0.28 }))
}

/// Black at `alpha` (shadows).
#[must_use]
pub fn oklcha_black(alpha: f32) -> rune::Color {
    rune::color::oklcha(0.0, 0.0, 0.0, alpha)
}

// ---------------------------------------------------------------------------
// Tine Deck 2.3 Hardware Mixer Primitives (Decky Deck 2.3 visual language)
// ---------------------------------------------------------------------------

/// Generates polygon vertices for a filled circle centered at `(cx, cy)`.
fn circle_points(cx: f32, cy: f32, r: f32, steps: usize) -> Vec<[f32; 2]> {
    let n = steps.max(8);
    (0..n)
        .map(|i| {
            let a = (i as f32 / n as f32) * std::f32::consts::TAU;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

/// Generates polyline points for an arc along the rotary dial (`norm` in `0.0..=1.0`).
fn dial_arc_points(cx: f32, cy: f32, r: f32, from_norm: f32, to_norm: f32, steps: usize) -> Vec<[f32; 2]> {
    let (a0, a1) = if from_norm <= to_norm {
        (from_norm, to_norm)
    } else {
        (to_norm, from_norm)
    };
    let n = steps.max(4);
    (0..=n)
        .map(|i| {
            let f = a0 + (a1 - a0) * (i as f32 / n as f32);
            let angle = -std::f32::consts::FRAC_PI_2 + (-135.0_f32 + f * 270.0_f32).to_radians();
            [cx + r * angle.cos(), cy + r * angle.sin()]
        })
        .collect()
}

/// Vector dial surround for a rotary knob: 11 radial perimeter tick dots, diagonal
/// end-stop ticks, and optional electric cobalt rim arc.
#[must_use]
pub fn rotary_dial_backdrop(size: f32, norm: f32, bipolar: bool, show_arc: bool, t: TineTheme) -> Shape {
    let norm = norm.clamp(0.0, 1.0);
    let c = size * 0.5;
    let mut shape = Shape::new();

    // Optional active cobalt sweep arc hugging the outer rim of the knob cap.
    if show_arc {
        let r_arc = size * 0.355;
        let anchor = if bipolar { 0.5 } else { 0.0 };
        if (norm - anchor).abs() > 0.015 {
            shape = shape.stroke_with(
                dial_arc_points(c, c, r_arc, anchor, norm, 24),
                false,
                2.5,
                LineCap::Round,
                t.deck_blue,
            );
        }
    }

    // Diagonal end-stop ticks (-135° and +135°).
    for deg in [-135.0_f32, 135.0_f32] {
        let angle = -std::f32::consts::FRAC_PI_2 + deg.to_radians();
        let r_in = size * 0.36;
        let r_out = size * 0.44;
        shape = shape.line(
            [c + r_in * angle.cos(), c + r_in * angle.sin()],
            [c + r_out * angle.cos(), c + r_out * angle.sin()],
            1.4,
            LineCap::Round,
            t.knob_tick_off,
        );
    }

    // 11 radial LED tick dots around the outer collar.
    let r_dot = size * 0.455;
    for i in 0..=10 {
        let f = i as f32 / 10.0;
        let angle = -std::f32::consts::FRAC_PI_2 + (-135.0_f32 + f * 270.0_f32).to_radians();
        let lit = if bipolar {
            (norm >= 0.5 && f >= 0.49 && f <= norm + 0.02) || (norm < 0.5 && f <= 0.51 && f >= norm - 0.02)
        } else {
            f <= norm + 0.02
        };
        let dot_r = if lit {
            (size * 0.032).clamp(1.8, 2.5)
        } else {
            (size * 0.022).clamp(1.3, 1.8)
        };
        let color = if lit { t.knob_tick_on } else { t.knob_tick_off };
        shape = shape.fill(
            circle_points(c + r_dot * angle.cos(), c + r_dot * angle.sin(), dot_r, 10),
            color,
        );
    }

    shape
}

/// The tactile porcelain-white rotating knob cap with inner powder-blue concave disc and
/// orbiting electric-blue rotor indicator dot, animated with Rune spring physics.
///
/// Sized at `0.66 * size` so even at 45° rotation + 1.04 hover scale (`0.66 * 1.04 * √2 = 0.970`),
/// its upright bounding box stays strictly inside the parent `size × size` dial box.
#[must_use]
pub fn rotary_cap(key: impl Into<String>, size: f32, norm: f32, active: bool, hovered: bool, t: TineTheme) -> El {
    let is_light = t.bg.r > 0.5;
    let cap_d = (size * 0.66).round();
    let offset = (size - cap_d) * 0.5;
    let inner_d = (cap_d * 0.66).round();
    let inner_off = (cap_d - inner_d) * 0.5;
    let dot_d = (cap_d * 0.185).clamp(5.5, 9.0);
    let dot_x = (cap_d - dot_d) * 0.5;
    let dot_y = cap_d * 0.19;
    let theta = (-135.0_f32 + norm.clamp(0.0, 1.0) * 270.0_f32).to_radians();
    let ring = if active || hovered {
        t.knob_ring_active
    } else {
        t.knob_ring
    };

    el().key(key)
        .size(cap_d, cap_d)
        .absolute(offset, offset)
        .radius(cap_d * 0.5)
        .fill(t.knob_outer)
        .border(if active || hovered { 2.0 } else { 1.5 }, ring)
        .shadow(2.0, 7.0, oklcha_black(if is_light { 0.14 } else { 0.36 }))
        .rotate(theta)
        .rotate_motion(Spring::SNAPPY)
        .scale(if hovered { 1.04 } else { 1.0 })
        .scale_motion(Spring::BOUNCY)
        .child(
            el().key("inner-disc")
                .size(inner_d, inner_d)
                .absolute(inner_off, inner_off)
                .radius(inner_d * 0.5)
                .fill(t.knob_inner),
        )
        .child(
            el().key("rotor-dot")
                .size(dot_d, dot_d)
                .absolute(dot_x, dot_y)
                .radius(dot_d * 0.5)
                .fill(t.knob_dot),
        )
}

/// Decky-Deck top channel `CUE` rectangular pad button (warm cue-orange when active, cool
/// porcelain-slate keycap when inactive). Pointer-clickable (`focusable(false)` so keyboard Tab order stays exact).
#[must_use]
pub fn cue_pad(
    key: impl Into<String>,
    a11y_label: impl Into<String>,
    caption: &'static str,
    active: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let is_light = t.bg.r > 0.5;
    let mut b = row()
        .key(key)
        .role("switch")
        .checked(active)
        .label(a11y_label)
        .focusable(false)
        .w_full()
        .h(26.0)
        .center()
        .radius(radii::SM + 1.0)
        .continuous_corners()
        .press_scale(0.96)
        .cursor(rune::Cursor::Pointer)
        .on_press(on_press);
    if active {
        b = b
            .fill(t.accent)
            .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, t.accent_hi), (1.0, t.accent_lo)])
            .shadow(2.0, 10.0, t.accent_glow)
            .hover(rune::color::oklcha(1.0, 0.0, 0.0, 0.10))
            .child(text(caption).font(typography::CAPTION).color(t.text_on_accent));
    } else if is_light {
        b = b
            .fill(t.surface_raised)
            .border(1.0, t.border_strong)
            .hover(t.surface_hover)
            .child(text(caption).font(typography::CAPTION).color(t.text_dim));
    } else {
        let keycap_ink = rune::color::oklch(0.24, 0.02, 260.0);
        b = b
            .fill(t.knob_inner)
            .border(1.0, t.border_strong)
            .hover(t.knob_outer)
            .child(text(caption).font(typography::CAPTION).color(keycap_ink));
    }
    b
}

/// Tactile Decky-Deck `SOUND FX` / mode pill (`focusable(false)`). Active pills glow electric blue.
#[must_use]
pub fn deck_pill(
    key: impl Into<String>,
    role: &'static str,
    a11y_label: impl Into<String>,
    caption: impl Into<String>,
    active: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let is_light = t.bg.r > 0.5;
    let caption = caption.into();
    let mut p = row()
        .key(key)
        .role(role)
        .label(a11y_label)
        .focusable(false)
        .h(24.0)
        .px(spacing::SM)
        .center()
        .radius(radii::SM + 2.0)
        .continuous_corners()
        .press_scale(0.96)
        .cursor(rune::Cursor::Pointer)
        .on_press(on_press);
    if active {
        p = p
            .fill(t.deck_blue)
            .shadow(2.0, 8.0, rune::color::oklcha(0.54, 0.22, 262.0, 0.35))
            .hover(rune::color::oklcha(1.0, 0.0, 0.0, 0.10))
            .child(text(caption).font(typography::CAPTION).color(rune::Color::WHITE));
    } else if is_light {
        p = p
            .fill(t.surface_raised)
            .border(1.0, t.border)
            .hover(t.surface_hover)
            .child(text(caption).font(typography::CAPTION).color(t.text));
    } else {
        let keycap_ink = rune::color::oklch(0.22, 0.02, 260.0);
        p = p
            .fill(t.knob_inner)
            .border(1.0, t.border_strong)
            .hover(t.knob_outer)
            .child(text(caption).font(typography::CAPTION).color(keycap_ink));
    }
    p
}

/// Decky-Deck `SEND / RETURN` chunky rectangular toggle switch (`focusable(false)`).
#[must_use]
pub fn deck_slide_switch(
    key: impl Into<String>,
    a11y_label: impl Into<String>,
    title: &'static str,
    (left_label, right_label): (&'static str, &'static str),
    right_on: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let key = key.into();
    let is_light = t.bg.r > 0.5;
    let thumb = el()
        .key(format!("{key}/thumb"))
        .w(20.0)
        .h(14.0)
        .radius(5.0)
        .fill(t.deck_blue)
        .shadow(1.0, 4.0, oklcha_black(if is_light { 0.18 } else { 0.35 }));
    let pad = el().key(format!("{key}/pad")).grow(1.0);

    let mut well = row()
        .key(format!("{key}/well"))
        .w(46.0)
        .h(20.0)
        .px(3.0)
        .center()
        .radius(7.0)
        .fill(t.well)
        .border(1.0, t.border_strong)
        .animate_layout();
    well = if right_on {
        well.child(pad).child(thumb)
    } else {
        well.child(thumb).child(pad)
    };

    col()
        .key(key.clone())
        .role("switch")
        .checked(right_on)
        .label(a11y_label)
        .focusable(false)
        .w_full()
        .gap(3.0)
        .center()
        .py(3.0)
        .radius(radii::SM)
        .press_scale(0.97)
        .hover(t.surface_raised)
        .cursor(rune::Cursor::Pointer)
        .on_press(on_press)
        .child(text(title).font(typography::EYEBROW).color(t.text_dim))
        .child(well)
        .child(
            row()
                .key(format!("{key}/labels"))
                .w(88.0)
                .center()
                .child(
                    text(left_label)
                        .font(typography::CAPTION)
                        .color(if right_on { t.text_dim } else { t.text }),
                )
                .child(el().key(format!("{key}/lbl-gap")).grow(1.0))
                .child(
                    text(right_label)
                        .font(typography::CAPTION)
                        .color(if right_on { t.text } else { t.text_dim }),
                ),
        )
}
