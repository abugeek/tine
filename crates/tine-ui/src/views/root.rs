//! Root Rune view assembling header, Golden Line canvas, Pro drawer, and modal overlays.

use rune::{Cx, El, col, el, row, text};
use tine_protocol::presets::Archetype;

use crate::intent::Intent;
use crate::model::{ApoBadge, AutoTuneStep, Macro};
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

const ARCHETYPES: [(Archetype, &str); 4] = [
    (Archetype::Dynamic, "Dynamic"),
    (Archetype::Condenser, "Condenser"),
    (Archetype::Headset, "Headset"),
    (Archetype::Laptop, "Laptop"),
];

/// Renders the complete Tine UI tree.
#[must_use]
pub fn view(s: &AppState, _cx: &mut Cx<AppState>) -> El {
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
        .child(header_placeholder(s, t))
        .child(canvas_placeholder(s, t))
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

fn header_placeholder(s: &AppState, t: TineTheme) -> El {
    let mic_name = s
        .model
        .selected
        .and_then(|idx| s.model.endpoints.get(idx))
        .map_or("No Microphone", |e| e.name.as_str());

    let mic_btn = pill_button(
        "header/mic-selector",
        "combobox",
        format!("Mic: {mic_name}"),
        false,
        t,
        |s| {
            if !s.model.endpoints.is_empty() {
                let next = s.model.selected.map_or(0, |i| (i + 1) % s.model.endpoints.len());
                s.model.selected = Some(next);
                s.emit(Intent::SelectEndpoint(next));
            }
        },
    );

    let apo_color = match s.model.apo {
        ApoBadge::Active => t.meter_green,
        ApoBadge::Inactive => t.meter_amber,
        ApoBadge::NotInstalled | ApoBadge::Error(_) => t.danger,
    };

    let mut apo_row = row()
        .key("header/apo-status")
        .gap(spacing::XS)
        .center()
        .px(spacing::SM)
        .h(30.0)
        .radius(radii::MD)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(el().key("header/apo-dot").size(8.0, 8.0).radius(4.0).fill(apo_color))
        .child(
            text(format!("APO: {}", s.model.apo.label()))
                .font(typography::LABEL)
                .color(t.text),
        );

    match s.model.apo {
        ApoBadge::NotInstalled => {
            apo_row = apo_row.child(pill_button(
                "header/apo-install",
                "button",
                "Install APO",
                true,
                t,
                |s| s.emit(Intent::InstallApo),
            ));
        }
        ApoBadge::Inactive | ApoBadge::Error(_) => {
            apo_row = apo_row.child(pill_button("header/apo-repair", "button", "Repair APO", true, t, |s| {
                s.emit(Intent::RepairApo);
            }));
        }
        ApoBadge::Active => {}
    }

    let mut arch_group = row()
        .key("header/archetypes")
        .role("radiogroup")
        .label("Microphone archetype")
        .gap(spacing::XS);

    for (arch, name) in ARCHETYPES {
        let key = match arch {
            Archetype::Dynamic => "header/arch-dynamic",
            Archetype::Condenser => "header/arch-condenser",
            Archetype::Headset => "header/arch-headset",
            Archetype::Laptop => "header/arch-laptop",
        };
        let selected = s.model.archetype == arch;
        arch_group = arch_group.child(pill_button(key, "radio", name, selected, t, move |s| {
            s.model.archetype = arch;
            s.emit(Intent::SetArchetype(arch));
        }));
    }

    let autotune_btn = pill_button("header/autotune", "button", "Auto-Tune (8s)", false, t, |s| {
        s.emit(Intent::StartAutoTune);
    });

    let monitor_label = if s.model.monitor.enabled {
        format!("Hear Myself ({:.0} ms)", s.model.monitor.estimated_latency_ms)
    } else {
        String::from("Hear Myself")
    };
    let monitor_btn = pill_button(
        "header/monitor",
        "switch",
        monitor_label,
        s.model.monitor.enabled,
        t,
        |s| {
            s.model.monitor.enabled = !s.model.monitor.enabled;
            s.emit(Intent::ToggleMonitor);
        },
    );

    let bypass_label = if s.model.bypass { "A/B: Raw" } else { "A/B: Tine" };
    let bypass_btn = pill_button("header/bypass", "switch", bypass_label, !s.model.bypass, t, |s| {
        s.toggle_bypass_optimistic();
    });

    col()
        .key("header")
        .w_full()
        .px(spacing::MD)
        .py(spacing::MD)
        .gap(spacing::SM)
        .radius(radii::LG)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(
            row()
                .key("header/top-row")
                .w_full()
                .gap(spacing::SM)
                .child(mic_btn)
                .child(apo_row)
                .child(el().key("header/spacer-1").grow(1.0))
                .child(arch_group)
                .child(autotune_btn),
        )
        .child(
            row()
                .key("header/bottom-row")
                .w_full()
                .gap(spacing::SM)
                .child(el().key("header/spacer-2").grow(1.0))
                .child(monitor_btn)
                .child(bypass_btn),
        )
}

fn canvas_placeholder(s: &AppState, t: TineTheme) -> El {
    let mut spectrum_row = row().key("canvas/spectrum").w_full().h(72.0).gap(3.0).center();

    for (idx, &mag) in s.live.spectrum.iter().enumerate() {
        let bar_h = (mag.clamp(0.0, 1.0) * 64.0).max(4.0);
        spectrum_row = spectrum_row.child(
            el().key(format!("canvas/bar-{idx}"))
                .grow(1.0)
                .h(bar_h)
                .radius(2.0)
                .fill(t.accent_glow),
        );
    }

    let mut macro_row = row().key("canvas/macros").w_full().gap(spacing::LG).center();

    for m in Macro::ALL {
        let val = m.get(s.model.macros);
        let pct = (val * 100.0).round() as u32;
        macro_row = macro_row.child(
            col()
                .key(m.key())
                .role("slider")
                .label(format!("{} {pct}%", m.label()))
                .focusable(true)
                .w(140.0)
                .px(spacing::MD)
                .py(spacing::MD)
                .gap(spacing::XS)
                .center()
                .radius(radii::MD)
                .fill(t.surface_raised)
                .border(1.0, t.accent)
                .on_press(move |s: &mut AppState| {
                    let next = (m.get(s.model.macros) + 0.05) % 1.05;
                    s.set_macro_optimistic(m, next.min(1.0));
                })
                .child(text(m.label()).font(typography::LABEL).color(t.text))
                .child(text(format!("{pct}%")).font(typography::CAPTION).color(t.text_dim)),
        );
    }

    let in_db = s.live.telemetry.input_rms_db;
    let out_db = s.live.telemetry.output_rms_db;

    col()
        .key("canvas")
        .w_full()
        .grow(1.0)
        .min_h(160.0)
        .px(spacing::LG)
        .py(spacing::LG)
        .gap(spacing::MD)
        .radius(radii::LG)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(
            row()
                .key("canvas/title-row")
                .w_full()
                .child(
                    text("Golden Line — Voice Conditioner")
                        .font(typography::BODY)
                        .color(t.text),
                )
                .child(el().key("canvas/title-spacer").grow(1.0))
                .child(
                    text(format!("In {in_db:.1} dBFS  |  Out {out_db:.1} dBFS"))
                        .font(typography::CAPTION)
                        .color(t.text_dim),
                ),
        )
        .child(spectrum_row)
        .child(macro_row)
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
