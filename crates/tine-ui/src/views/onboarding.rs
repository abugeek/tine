//! First-run onboarding overlay: a four-step modal that takes a new user from
//! "just installed" to a good sound in under two minutes —
//!
//! 1. **Pick the mic** — endpoint list, with the suggested archetype pre-selected
//!    (the engine picks it from the device's form factor; the UI shows whatever
//!    `UiModel::archetype` carries).
//! 2. **Install the APO** — what the one UAC prompt does, with progress, success,
//!    or a typed error with remedies.
//! 3. **Auto-Tune** (skippable) — hands over to the 8-second wizard.
//! 4. **Finish** — "Open Discord/OBS and pick your normal mic, it's processed
//!    now", with a "Hear Myself" test button.
//!
//! Steps are UI-local (`UiLocal::onboarding_step`); the engine-facing side effects
//! reuse existing intents (`SelectEndpoint`, `SetArchetype`, `InstallApo`,
//! `StartAutoTune`, `ToggleMonitor`).

use rune::{El, col, el, row, text};
use tine_protocol::presets::Archetype;

use crate::intent::Intent;
use crate::model::ApoBadge;
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

/// Total number of onboarding steps.
pub const STEP_COUNT: u8 = 4;

/// Renders the onboarding modal (mount only when
/// `model.first_run && !ui.onboarding_dismissed`).
#[must_use]
pub fn view(s: &AppState, t: TineTheme) -> El {
    let mut card = col()
        .key("onboarding-card")
        .w(600.0)
        .px(spacing::XL)
        .py(spacing::LG)
        .gap(spacing::MD)
        .radius(radii::LG)
        .fill(t.surface_raised)
        .border(1.0, t.accent)
        .transition(
            rune::Transition::scale(0.96)
                .combined(rune::Transition::opacity())
                .springs(rune::SpringPair::new(rune::Spring::SNAPPY, rune::Spring::SMOOTH)),
        )
        // Esc dismisses: skip the rest of onboarding; it never blocks the app.
        .focus_return("header/autotune")
        .modal_tx(true, |s: &mut AppState, tx: &mut rune::Tx| {
            s.ui.onboarding_dismissed = true;
            tx.instant();
        })
        .child(dots(s.ui.onboarding_step, t));

    match s.ui.onboarding_step {
        1 => card = card.child(step1_mic(s, t)),
        2 => card = card.child(step2_apo(s, t)),
        3 => card = card.child(step3_autotune(s, t)),
        _ => card = card.child(step4_finish(s, t)),
    }

    el().key("onboarding-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .fill(rune::color::oklcha(0.0, 0.0, 0.0, 0.65))
        .child(card)
}

/// Step progress dots (`1..=4`).
#[must_use]
pub fn dots(step: u8, t: TineTheme) -> El {
    let mut strip = row().key("onboarding/dots").w_full().gap(spacing::XS);
    for i in 1..=STEP_COUNT {
        strip = strip.child(
            el().key(format!("onboarding/dot-{i}"))
                .role("label")
                .label(format!("Onboarding step {i} of {STEP_COUNT}"))
                .w(if i == step { 22.0 } else { 8.0 })
                .h(8.0)
                .radius(radii::PILL)
                .fill(if i <= step { t.accent } else { t.border }),
        );
    }
    strip
}

// --- Step 1: pick the mic ----------------------------------------------------

fn step1_mic(s: &AppState, t: TineTheme) -> El {
    let mut body = col()
        .key("onboarding/step-1")
        .w_full()
        .gap(spacing::MD)
        .child(title("Welcome to Tine", t.text))
        .child(
            text(
                "Pick the microphone you talk into. Tine's archetype suggestion comes from your device's form factor.",
            )
            .font(typography::BODY)
            .color(t.text_dim),
        );

    // Endpoint list: radio pills; model.selected marks the current one.
    let mut list = col()
        .key("onboarding/mic-list")
        .role("radiogroup")
        .label("Pick your microphone")
        .w_full()
        .gap(spacing::XS);

    for (idx, endpoint) in s.model.endpoints.iter().enumerate() {
        let selected = s.model.selected == Some(idx);
        let id = endpoint.id.clone();
        let name = endpoint.name.clone();
        list = list.child(
            row()
                .key(format!("onboarding/mic-{idx}"))
                .role("radio")
                .checked(selected)
                .label(format!("Microphone {idx}: {name}"))
                .focusable(true)
                .h(30.0)
                .px(spacing::MD)
                .gap(spacing::SM)
                .center()
                .radius(radii::MD)
                .fill(if selected { t.accent_glow } else { t.surface })
                .border(1.0, if selected { t.accent } else { t.border })
                .on_press(move |s: &mut AppState| {
                    s.model.selected = Some(idx);
                    s.emit(Intent::SelectEndpoint(id.clone()));
                })
                .child(
                    text(if selected { "(•)" } else { "( )" })
                        .font(typography::CAPTION)
                        .color(t.accent),
                )
                .child(text(name).font(typography::BODY).color(t.text)),
        );
    }
    body = body.child(list);

    // Suggested archetype: pre-selected from the form factor by the engine.
    let mut archs = row()
        .key("onboarding/archetypes")
        .role("radiogroup")
        .label("Suggested archetype")
        .w_full()
        .gap(spacing::XS);
    for (arch, name) in ARCHETYPES {
        let selected = s.model.archetype == arch;
        let mut pill = row()
            .key(format!("onboarding/arch-{}", archetype_id(arch)))
            .role("radio")
            .checked(selected)
            .label(name)
            .focusable(true)
            .h(26.0)
            .px(spacing::MD)
            .center()
            .radius(radii::PILL)
            .fill(if selected { t.accent } else { t.surface })
            .border(1.0, t.border)
            .on_press(move |s: &mut AppState| {
                s.model.archetype = arch;
                s.emit(Intent::SetArchetype(arch));
            })
            .child(
                text(name)
                    .font(typography::LABEL)
                    .color(if selected { t.text_on_accent } else { t.text }),
            );
        if selected {
            pill = pill.child(
                text("suggested")
                    .key(format!("onboarding/arch-suggested-{}", archetype_id(arch)))
                    .role("label")
                    .label("Suggested archetype")
                    .font(typography::CAPTION)
                    .color(t.text_on_accent),
            );
        }
        archs = archs.child(pill);
    }

    body = body
        .child(archs)
        .child(row().key("onboarding/step-1-actions").gap(spacing::SM).child(button(
            "onboarding/next-1",
            "Continue",
            true,
            t,
            |s| {
                s.ui.onboarding_step = 2;
            },
        )));
    body
}

const ARCHETYPES: [(Archetype, &str); 4] = [
    (Archetype::Dynamic, "Dynamic"),
    (Archetype::Condenser, "Condenser"),
    (Archetype::Headset, "Headset"),
    (Archetype::Laptop, "Laptop"),
];

/// Stable lowercase id for archetype element keys.
fn archetype_id(a: Archetype) -> &'static str {
    match a {
        Archetype::Dynamic => "dynamic",
        Archetype::Condenser => "condenser",
        Archetype::Headset => "headset",
        Archetype::Laptop => "laptop",
    }
}

// --- Step 2: set up Virtual Cable routing -----------------------------------

/// What step 2 shows, derived from the model's Virtual Cable badge + local check progress.
fn step2_apo(s: &AppState, t: TineTheme) -> El {
    let mut body = col()
        .key("onboarding/step-2")
        .w_full()
        .gap(spacing::MD)
        .child(title("Set up Virtual Cable routing", t.text))
        .child(
            text(
                "Tine processes your microphone in real time and outputs to VB-Audio Virtual Cable. \
                 Install VB-Cable if it isn't present on your system, then click Check Setup below \
                 so Discord, OBS, and games can receive your processed voice.",
            )
            .font(typography::BODY)
            .color(t.text_dim),
        );

    match (&s.model.apo, s.ui.apo_installing) {
        (ApoBadge::Active, _) => {
            body = body.child(status_line(
                "onboarding/apo-success",
                "Virtual Cable detected — your microphone is ready to route",
                t.meter_green,
                t,
            ));
        }
        (ApoBadge::Error(msg), _) => {
            body = body
                .child(status_line(
                    "onboarding/apo-error",
                    format!("Virtual Cable check failed: {msg}"),
                    t.danger,
                    t,
                ))
                .child(remedies(t))
                .child(button("onboarding/recheck", "Recheck", true, t, |s| {
                    s.ui.apo_installing = true;
                    s.emit(Intent::InstallApo);
                }));
        }
        (_, true) => {
            body = body
                .child(status_line(
                    "onboarding/apo-progress",
                    "Checking Virtual Cable setup…",
                    t.meter_amber,
                    t,
                ))
                .child(button("onboarding/recheck", "Recheck", false, t, |s| {
                    s.ui.apo_installing = true;
                    s.emit(Intent::InstallApo);
                }));
        }
        (_, false) => {
            body = body.child(button("onboarding/install", "Check Setup", true, t, |s| {
                s.ui.apo_installing = true;
                s.emit(Intent::InstallApo);
            }));
        }
    }

    body = body.child(
        row()
            .key("onboarding/step-2-actions")
            .gap(spacing::SM)
            .child(button("onboarding/back-2", "Back", false, t, |s| {
                s.ui.onboarding_step = 1;
            }))
            .child(button("onboarding/next-2", "Skip this for now", false, t, |s| {
                s.ui.onboarding_step = 3;
            })),
    );
    body
}

/// Concrete fixes for the Virtual Cable setup check error.
fn remedies(t: TineTheme) -> El {
    let mut r = col().key("onboarding/apo-remedies").w_full().gap(spacing::XS);
    for (i, tip_text) in [
        "Download and install VB-Audio Virtual Cable (vb-audio.com/Cable), then click Recheck.",
        "In Windows Sound Settings, ensure \"CABLE Input (VB-Audio Virtual Cable)\" is enabled.",
        "Reboot once if VB-Cable was just installed so Windows registers the virtual device.",
    ]
    .iter()
    .enumerate()
    {
        r = r.child(
            text(tip_text.to_owned())
                .key(format!("onboarding/apo-remedy-{i}"))
                .role("label")
                .label(format!("Remedy {i}"))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );
    }
    r
}

// --- Step 3: Auto-Tune (skippable) --------------------------------------------

fn step3_autotune(_s: &AppState, t: TineTheme) -> El {
    col()
        .key("onboarding/step-3")
        .w_full()
        .gap(spacing::MD)
        .child(title("Auto-Tune your mic (8 seconds)", t.text))
        .child(
            text(
                "Measure your room and voice once, and every Tine control starts from a true \
                 baseline. You need 8 quiet seconds — 3 silent, 5 speaking normally.",
            )
            .font(typography::BODY)
            .color(t.text_dim),
        )
        .child(
            row()
                .key("onboarding/step-3-actions")
                .gap(spacing::SM)
                .child(button("onboarding/back-3", "Back", false, t, |s| {
                    s.ui.onboarding_step = 2;
                }))
                .child(button("onboarding/skip-3", "Skip Auto-Tune", false, t, |s| {
                    s.ui.onboarding_step = 4;
                }))
                // Run: closes onboarding at step 4 and opens the wizard modal.
                .child(button("onboarding/run-autotune", "Run Auto-Tune", true, t, |s| {
                    s.ui.onboarding_step = 4;
                    s.emit(Intent::StartAutoTune);
                })),
        )
}

// --- Step 4: finish ------------------------------------------------------------

fn step4_finish(s: &AppState, t: TineTheme) -> El {
    col()
        .key("onboarding/step-4")
        .w_full()
        .gap(spacing::MD)
        .child(title("You're set — go be heard", t.text))
        .child(
            text(
                "Open Discord or OBS and pick \"CABLE Output (VB-Audio Virtual Cable)\" as your \
                 microphone. Try it with Hear Myself, and mind the ~30 ms monitoring delay.",
            )
            .font(typography::BODY)
            .color(t.text_dim),
        )
        .child(
            row()
                .key("onboarding/step-4-actions")
                .gap(spacing::SM)
                .child(button("onboarding/back-4", "Back", false, t, |s| {
                    s.ui.onboarding_step = 3;
                }))
                .child(monitor_test_button(s, t))
                .child(button("onboarding/finish", "Finish", true, t, |s| {
                    s.ui.onboarding_dismissed = true;
                })),
        )
}

/// The "Hear Myself" test button (mirrors the header switch's optimistic toggle).
fn monitor_test_button(s: &AppState, t: TineTheme) -> El {
    let label = if s.model.monitor.enabled {
        "Hear Myself: stop"
    } else {
        "Hear Myself: test"
    };
    button("onboarding/monitor-test", label, s.model.monitor.enabled, t, |s| {
        s.model.monitor.enabled = !s.model.monitor.enabled;
        s.emit(Intent::ToggleMonitor);
    })
}

// --- shared bits ---------------------------------------------------------------

fn title(label: &str, color: rune::Color) -> El {
    text(label.to_owned())
        .key(format!("onboarding/title-{label}"))
        .role("heading")
        .level(1)
        .font(typography::TITLE)
        .color(color)
}

fn status_line(key: &'static str, body: impl Into<String>, color: rune::Color, t: TineTheme) -> El {
    let body: String = body.into();
    row()
        .key(key)
        .role("status")
        .label(body.clone())
        .w_full()
        .gap(spacing::SM)
        .center()
        .child(
            el().key(format!("{key}/dot"))
                .size(10.0, 10.0)
                .radius(radii::PILL)
                .fill(color),
        )
        .child(text(body).font(typography::BODY).color(t.text))
}

fn button(
    key: &'static str,
    label: &str,
    primary: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let bg = if primary { t.accent } else { t.surface };
    let fg = if primary { t.text_on_accent } else { t.text };
    row()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(28.0)
        .px(spacing::LG)
        .center()
        .radius(radii::PILL)
        .fill(bg)
        .border(1.0, t.border)
        .on_press(on_press)
        .child(text(label).font(typography::LABEL).color(fg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archetype_ids_are_stable() {
        assert_eq!(ARCHETYPES.len(), 4);
        assert_eq!(archetype_id(Archetype::Dynamic), "dynamic");
        assert_eq!(archetype_id(Archetype::Condenser), "condenser");
        assert_eq!(archetype_id(Archetype::Headset), "headset");
        assert_eq!(archetype_id(Archetype::Laptop), "laptop");
        assert_eq!(STEP_COUNT, 4);
    }
}
