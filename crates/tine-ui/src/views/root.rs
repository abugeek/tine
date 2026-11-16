//! Root Rune view assembling header, Golden Line canvas, Pro drawer, and modal overlays.

use rune::{Cx, El, col, el, row, text};

use crate::intent::Intent;
use crate::model::AutoTuneStep;
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

/// Renders the complete Tine UI tree.
#[must_use]
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = TineTheme::dark();
    let modal_active =
        !matches!(s.model.autotune.step, AutoTuneStep::Idle) || (s.model.first_run && !s.ui.onboarding_dismissed);

    let mut main = col()
        .key("main-shell")
        .w_full()
        .h_full()
        .fill(t.bg)
        .px(spacing::LG)
        .py(spacing::MD)
        .gap(spacing::MD)
        .child(crate::views::header::view(s, t))
        .child(crate::views::canvas::view(s, cx))
        .child(pro_drawer_placeholder(s, t));

    if modal_active {
        main = main.inert();
    }

    let mut root = el().key("root").w_full().h_full().fill(t.bg).child(main);

    if !matches!(s.model.autotune.step, AutoTuneStep::Idle) {
        root = root.child(autotune_modal_placeholder(s, t));
    } else if s.model.first_run && !s.ui.onboarding_dismissed {
        root = root.child(onboarding_modal_placeholder(s, t));
    }

    root
}

fn pill_button(
    key: &'static str,
    role: &'static str,
    label: impl Into<String>,
    active: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let label_str = label.into();
    let bg = if active { t.accent } else { t.surface_raised };
    let fg = if active { t.text_on_accent } else { t.text };

    row()
        .key(key)
        .role(role)
        .label(label_str.clone())
        .focusable(true)
        .h(30.0)
        .px(spacing::MD)
        .center()
        .radius(radii::PILL)
        .fill(bg)
        .border(1.0, t.border)
        .on_press(on_press)
        .child(text(label_str).font(typography::LABEL).color(fg))
}

fn pro_drawer_placeholder(s: &AppState, t: TineTheme) -> El {
    let toggle_label = if s.ui.pro_drawer_open {
        "Pro Controls ▲"
    } else {
        "Pro Controls ▾"
    };

    let toggle = pill_button("pro/toggle", "button", toggle_label, s.ui.pro_drawer_open, t, |s| {
        s.ui.pro_drawer_open = !s.ui.pro_drawer_open;
    });

    let mut container = col()
        .key("pro-drawer")
        .w_full()
        .px(spacing::MD)
        .py(spacing::MD)
        .gap(spacing::SM)
        .radius(radii::LG)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(
            row().key("pro/header").w_full().gap(spacing::MD).child(toggle).child(
                text(format!(
                    "Gate {:.1} dB  |  De-Ess {:.1} dB  |  Comp {:.1} dB",
                    s.live.telemetry.gate_attenuation_db,
                    s.live.telemetry.deesser_reduction_db,
                    s.live.telemetry.comp_gain_reduction_db,
                ))
                .font(typography::CAPTION)
                .color(t.text_dim),
            ),
        );

    if s.ui.pro_drawer_open {
        container = container.child(
            row()
                .key("pro/rack-placeholder")
                .w_full()
                .px(spacing::MD)
                .py(spacing::MD)
                .gap(spacing::MD)
                .radius(radii::MD)
                .fill(t.surface_raised)
                .border(1.0, t.border)
                .child(
                    text("Stages 1–6 Surgical Pro Rack (T20)")
                        .font(typography::BODY)
                        .color(t.text),
                )
                .child(el().key("pro/spacer").grow(1.0))
                .child(pill_button("pro/unpin-all", "button", "Unpin All", false, t, |s| {
                    s.model.pins.clear();
                    s.emit(Intent::UnpinAll);
                })),
        );
    }

    container
}

fn autotune_modal_placeholder(s: &AppState, t: TineTheme) -> El {
    let (step_title, detail) = match &s.model.autotune.step {
        AutoTuneStep::Idle => ("", String::new()),
        AutoTuneStep::Prepare => ("Auto-Tune (8s) — Prepare", s.model.autotune.prompt_text.clone()),
        AutoTuneStep::Silence { remaining_secs } => (
            "Step 1/2 — Room Silence",
            format!("{:.1}s remaining — {}", remaining_secs, s.model.autotune.prompt_text),
        ),
        AutoTuneStep::Speak { remaining_secs } => (
            "Step 2/2 — Speak Normally",
            format!(
                "{:.1}s remaining — \"{}\"",
                remaining_secs, s.model.autotune.prompt_text
            ),
        ),
        AutoTuneStep::Analyzing => ("Analyzing...", s.model.autotune.prompt_text.clone()),
        AutoTuneStep::Results => (
            "Auto-Tune Results",
            s.model.calibration.as_ref().map_or_else(String::new, |c| {
                format!(
                    "Noise {:.1} dBFS, Speech {:.1} dBFS ({} parameters tuned)",
                    c.noise_rms_db,
                    c.speech_rms_db,
                    c.changes.len()
                )
            }),
        ),
        AutoTuneStep::Failed(err) => ("Auto-Tune Failed", err.clone()),
    };

    let mut actions = row().key("autotune/actions").gap(spacing::SM);
    if matches!(s.model.autotune.step, AutoTuneStep::Results) {
        actions = actions
            .child(pill_button(
                "autotune/apply",
                "button",
                "Apply Calibration",
                true,
                t,
                |s| s.emit(Intent::ApplyCalibration { keep_pins: true }),
            ))
            .child(pill_button("autotune/discard", "button", "Discard", false, t, |s| {
                s.emit(Intent::DiscardCalibration);
            }));
    } else {
        actions = actions.child(pill_button("autotune/cancel", "button", "Cancel", false, t, |s| {
            s.emit(Intent::CancelAutoTune);
        }));
    }

    el().key("autotune-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .fill(rune::color::oklcha(0.0, 0.0, 0.0, 0.65))
        .child(
            col()
                .key("autotune-card")
                .w(520.0)
                .px(spacing::XL)
                .py(spacing::XL)
                .gap(spacing::MD)
                .radius(radii::LG)
                .fill(t.surface_raised)
                .border(1.0, t.accent)
                .child(text(step_title).font(typography::TITLE).color(t.text))
                .child(text(detail).font(typography::BODY).color(t.text_dim))
                .child(
                    text(format!("Live input: {:.1} dBFS", s.model.autotune.level_db))
                        .font(typography::CAPTION)
                        .color(t.text_dim),
                )
                .child(actions),
        )
}

fn onboarding_modal_placeholder(_s: &AppState, t: TineTheme) -> El {
    el().key("onboarding-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .fill(rune::color::oklcha(0.0, 0.0, 0.0, 0.65))
        .child(
            col()
                .key("onboarding-card")
                .w(520.0)
                .px(spacing::XL)
                .py(spacing::XL)
                .gap(spacing::MD)
                .radius(radii::LG)
                .fill(t.surface_raised)
                .border(1.0, t.accent)
                .child(text("Welcome to Tine").font(typography::TITLE).color(t.text))
                .child(
                    text(
                        "Install the system-wide microphone APO and run the 8-second Auto-Tune to calibrate your mic.",
                    )
                    .font(typography::BODY)
                    .color(t.text_dim),
                )
                .child(
                    row()
                        .key("onboarding/actions")
                        .gap(spacing::SM)
                        .child(pill_button(
                            "onboarding/install",
                            "button",
                            "Install APO & Continue",
                            true,
                            t,
                            |s| {
                                s.ui.onboarding_dismissed = true;
                                s.emit(Intent::InstallApo);
                            },
                        ))
                        .child(pill_button(
                            "onboarding/dismiss",
                            "button",
                            "Skip for Now",
                            false,
                            t,
                            |s| {
                                s.ui.onboarding_dismissed = true;
                            },
                        )),
                ),
        )
}
