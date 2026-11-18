//! Auto-Tune (8 s) wizard overlay: a focus-trapping modal over the Golden Line
//! that walks the real calibration phase machine —
//! `tine_dsp::calibrator::CalPhase`: Countdown 1 s (Prepare: tips + countdown
//! ring) → Noise 3 s (Silence: ring + live level) → Voice 5 s (Speak: suggested
//! sentence + level + voiced-so-far) → Analyzing → Results (noise floor, speech
//! level, SNR, verified output peak, before → after diffs, "Keep my Pro pins",
//! Apply/Discard/Retry) or a Failed card with a concrete remedy for each
//! `CalFailure` variant.
//!
//! ## Narrow T11 adapter (view-model plumbing only)
//!
//! The bridge between the engine-side state machine
//! (`tine_dsp::calibrator`: `CalProgress`, `CalFailure`, `CalibrationReport`)
//! and this wizard's view model lives in **this module, in the functions
//! below** — nothing else in `tine-ui` knows the DSP types:
//!
//! * [`autotune_from_progress`] — `CalProgress` → `AutoTuneUi` (phase,
//!   per-phase countdown derived from `phase_progress`, live level,
//!   `voiced_ms`);
//! * [`CalFailureUi::from`] — `CalFailure` → the failure card's payload;
//! * [`calibration_from_report`] — `CalibrationReport` → the results card.
//!
//! TODO(T11): the only remaining gap is engine-side: `tine-app`'s Auto-Tune
//! runner (T16/T21, which owns the capture thread that `push`es raw mic
//! blocks) calls these three functions to fill `UiModel::autotune` /
//! `calibration`, and applies `Intent::ApplyCalibration { keep_pins }` by
//! replaying the report's changes (pinned parameters keep their value per
//! D-007). `tine-ui` itself depends only on `tine-protocol` + `tine-dsp`,
//! never `tine-engine`.

use rune::{El, Shape, col, el, row, text};
use tine_dsp::calibrator::{COUNTDOWN_S, CalFailure, CalPhase, CalProgress, CalibrationReport, NOISE_S, VOICE_S};
use tine_protocol::params::ParamId;

use crate::intent::Intent;
use crate::model::{AutoTuneStep, AutoTuneUi, CalFailureUi, CalibrationParamDiff, CalibrationUi, UiModel};
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

/// Suggested sentence read during the Speak phase.
pub const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";

/// Seconds in the measured silence phase (Noise — the real `tine-dsp` constant).
pub const SILENCE_SECS: f32 = NOISE_S;
/// Seconds in the measured speech phase (Voice — the real `tine-dsp` constant).
pub const SPEAK_SECS: f32 = VOICE_S;

/// Narrow adapter: one `CalProgress` push → the wizard's whole view model.
///
/// Per-phase countdowns are derived from the real `phase_progress` (the
/// fraction of the current phase already captured), so the ring always matches
/// what the calibrator actually consumed. The overall `progress` covers the
/// 8 measured seconds (Noise + Voice; the Countdown second is discarded by
/// design).
///
/// `failure` must be `Some` exactly when `phase` is [`CalPhase::Failed`]:
/// `finish()`'s `Err(CalFailure)` payload — `CalProgress` alone doesn't carry
/// it. The runner (T16/T21) keeps it from the `finish` call that ended the
/// capture and passes both here.
#[must_use]
pub fn autotune_from_progress(p: &CalProgress, failure: Option<CalFailure>) -> AutoTuneUi {
    let remaining = |total: f32| total * (1.0 - p.phase_progress.clamp(0.0, 1.0));
    let step = match p.phase {
        CalPhase::Countdown => AutoTuneStep::Prepare {
            remaining_secs: remaining(COUNTDOWN_S),
        },
        CalPhase::Noise => AutoTuneStep::Silence {
            remaining_secs: remaining(NOISE_S),
        },
        CalPhase::Voice => AutoTuneStep::Speak {
            remaining_secs: remaining(VOICE_S),
        },
        CalPhase::Analyzing => AutoTuneStep::Analyzing,
        CalPhase::Done => AutoTuneStep::Results,
        CalPhase::Failed => AutoTuneStep::Failed(CalFailureUi::from(failure.unwrap_or(CalFailure::TooQuiet))),
    };
    let measured = match p.phase {
        CalPhase::Countdown => 0.0,
        CalPhase::Noise => p.phase_progress * NOISE_S,
        CalPhase::Voice => NOISE_S + p.phase_progress * VOICE_S,
        CalPhase::Analyzing | CalPhase::Done | CalPhase::Failed => NOISE_S + VOICE_S,
    };
    AutoTuneUi {
        step,
        progress: (measured / (NOISE_S + VOICE_S)).clamp(0.0, 1.0),
        level_db: p.live_level_db,
        prompt_text: String::new(),
        voiced_ms: p.voiced_ms,
    }
}

/// Narrow adapter: one `CalibrationReport` → the results card's view model.
#[must_use]
pub fn calibration_from_report(report: &CalibrationReport) -> CalibrationUi {
    let cal = &report.calibration;
    CalibrationUi {
        noise_rms_db: cal.noise_rms_db,
        speech_rms_db: cal.speech_rms_db,
        simulated_peak_db: report.simulated_peak_db,
        simulated_rms_db: report.simulated_rms_db,
        changes: report
            .changes
            .iter()
            .map(|&(id, before, after)| CalibrationParamDiff {
                param: id,
                label: id.spec().label,
                before,
                after,
                unit: unit_suffix(id),
            })
            .collect(),
    }
}

/// Viewer-facing unit suffix for a diff row.
fn unit_suffix(id: ParamId) -> &'static str {
    use tine_protocol::params::Unit;
    match id.spec().unit {
        Unit::Db => "dB",
        Unit::Dbfs => "dBFS",
        Unit::Hz => "Hz",
        Unit::Ms => "ms",
        Unit::Ratio | Unit::None => "",
    }
}

impl From<CalFailure> for CalFailureUi {
    fn from(f: CalFailure) -> Self {
        match f {
            CalFailure::TooQuiet => Self::TooQuiet,
            CalFailure::TooNoisy { snr_db } => Self::TooNoisy { snr_db },
            CalFailure::Clipping { pct } => Self::Clipping { pct },
            CalFailure::NotEnoughSpeech { voiced_ms } => Self::NotEnoughSpeech { voiced_ms },
            CalFailure::GainOutOfRange { needed_db } => Self::GainOutOfRange { needed_db },
        }
    }
}

/// Speech-to-noise ratio in dB from the calibration's measured levels.
#[must_use]
pub fn snr_db(c: &CalibrationUi) -> f32 {
    c.speech_rms_db - c.noise_rms_db
}

/// A concrete fix for every `CalFailure` variant (T11 contract).
#[must_use]
pub fn failure_remedy(f: CalFailureUi) -> &'static str {
    match f {
        CalFailureUi::TooQuiet => "Move the mic closer (5–15 cm) and turn up its hardware gain, then retry.",
        CalFailureUi::TooNoisy { .. } => {
            "Close the window, silence fans/AC, or move the mic closer so your voice dominates the room."
        }
        CalFailureUi::Clipping { .. } => {
            "Your mic is clipping — lower its hardware gain or move back a little, then retry."
        }
        CalFailureUi::NotEnoughSpeech { .. } => {
            "Speak continuously at your normal volume for the full 5 seconds — reading the sentence out loud works well."
        }
        CalFailureUi::GainOutOfRange { .. } => {
            "Enable Mic Boost or a hardware preamp: your mic needs more gain than Tine can add in software."
        }
    }
}

/// Renders the Auto-Tune wizard modal (mount only when
/// `!matches!(step, AutoTuneStep::Idle)`).
#[must_use]
pub fn view(s: &AppState, t: TineTheme) -> El {
    let au = &s.model.autotune;

    let mut card = col()
        .key("autotune-card")
        .w(560.0)
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
        // Esc requests dismissal; the wizard cancels and focus returns to the
        // header's Auto-Tune button.
        .focus_return("header/autotune")
        .modal_tx(false, |s: &mut AppState, tx: &mut rune::Tx| {
            s.emit(Intent::CancelAutoTune);
            tx.instant();
        })
        .child(step_indicator(au, t));

    match &au.step {
        AutoTuneStep::Idle => {}
        AutoTuneStep::Prepare { remaining_secs } => {
            card = card
                .child(title("Get ready", t.text))
                .child(
                    text("Tine measures your room first, then your voice: 8 seconds total.")
                        .font(typography::BODY)
                        .color(t.text_dim),
                )
                .child(tip("1", "Stay silent for the first 3 seconds.", t))
                .child(tip("2", "Then speak normally, at your usual distance.", t))
                .child(tip(
                    "3",
                    format!("Read this when prompted: \u{201c}{SENTENCE}\u{201d}"),
                    t,
                ))
                .child(countdown_ring(*remaining_secs, COUNTDOWN_S, t))
                .child(level_bar(au.level_db, t))
                .child(cancel_row(t));
        }
        AutoTuneStep::Silence { remaining_secs } => {
            card = card
                .child(title("Step 1 of 2 — Silence", t.text))
                .child(
                    text("Stay quiet — measuring your room noise floor.")
                        .font(typography::BODY)
                        .color(t.text_dim),
                )
                .child(countdown_ring(*remaining_secs, SILENCE_SECS, t))
                .child(level_bar(au.level_db, t))
                .child(cancel_row(t));
        }
        AutoTuneStep::Speak { remaining_secs } => {
            card = card
                .child(title("Step 2 of 2 — Speak normally", t.text))
                .child(
                    text(format!("\u{201c}{}\u{201d}", speak_prompt(au)))
                        .font(typography::BODY)
                        .color(t.text),
                )
                .child(
                    text(format!(
                        "Speak at your usual volume and distance. {:.1} s of speech captured so far.",
                        au.voiced_ms as f32 / 1000.0
                    ))
                    .key("autotune/voiced-so-far")
                    .role("label")
                    .label(format!("{} ms of speech captured so far", au.voiced_ms))
                    .font(typography::CAPTION)
                    .color(t.text_dim),
                )
                .child(countdown_ring(*remaining_secs, SPEAK_SECS, t))
                .child(level_bar(au.level_db, t))
                .child(cancel_row(t));
        }
        AutoTuneStep::Analyzing => {
            card = card
                .child(title("Analyzing…", t.text))
                .child(
                    text("Simulating the full pipeline over your captured voice.")
                        .font(typography::BODY)
                        .color(t.text_dim),
                )
                .child(progress_bar(au.progress, t))
                .child(cancel_row(t));
        }
        AutoTuneStep::Results => {
            card = card
                .child(title("Auto-Tune Results", t.text))
                .child(results_card(&s.model, t))
                .child(keep_pins_checkbox(s.ui.autotune_keep_pins, t))
                .child(
                    // TODO(T11): the engine applies `ApplyCalibration { keep_pins }` by
                    // replaying the calibration report's changes (respecting pins);
                    // the UI only reports the intent (narrow adapter, see module docs).
                    button_row(
                        &[
                            ("autotune/apply", "Apply", true),
                            ("autotune/discard", "Discard", false),
                            ("autotune/retry", "Retry", false),
                        ],
                        t,
                    ),
                );
        }
        AutoTuneStep::Failed(reason) => {
            card = card
                .child(title("Auto-Tune failed", t.danger))
                .child(
                    text(reason.summary())
                        .key("autotune/failure-summary")
                        .font(typography::BODY)
                        .color(t.text),
                )
                .child(
                    text(failure_remedy(*reason))
                        .key("autotune/failure-remedy")
                        .font(typography::BODY)
                        .color(t.text_dim),
                )
                .child(button_row(
                    &[("autotune/close", "Close", false), ("autotune/retry", "Retry", true)],
                    t,
                ));
        }
    }

    scrim().child(card)
}

/// The scrim behind the modal.
fn scrim() -> El {
    el().key("autotune-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .fill(rune::color::oklcha(0.0, 0.0, 0.0, 0.65))
}

/// "Prepare / Silence / Speak / Analyze / Results" strip; the failed wizard dims it.
fn step_indicator(au: &AutoTuneUi, t: TineTheme) -> El {
    const STEPS: [&str; 5] = ["Prepare", "Silence 3 s", "Speak 5 s", "Analyze", "Results"];
    let idx = match &au.step {
        AutoTuneStep::Idle | AutoTuneStep::Prepare { .. } => 0,
        AutoTuneStep::Silence { .. } => 1,
        AutoTuneStep::Speak { .. } => 2,
        AutoTuneStep::Analyzing => 3,
        AutoTuneStep::Results | AutoTuneStep::Failed(_) => 4,
    };

    let mut strip = row().key("autotune/steps").w_full().gap(spacing::XS).center();
    for (i, name) in STEPS.iter().enumerate() {
        let reached = i <= idx;
        strip = strip.child(
            row()
                .key(format!("autotune/step-{i}"))
                .role("label")
                .label(format!(
                    "Step {}: {name} ({})",
                    i + 1,
                    if reached { "reached" } else { "upcoming" }
                ))
                .px(spacing::SM)
                .h(22.0)
                .center()
                .radius(radii::PILL)
                .fill(if reached { t.accent_glow } else { t.surface })
                .border(1.0, if i == idx { t.accent } else { t.border })
                .child(text(name.to_owned()).font(typography::CAPTION).color(if reached {
                    t.text
                } else {
                    t.text_dim
                })),
        );
    }
    strip
}

/// Card title.
fn title(label: &str, color: rune::Color) -> El {
    text(label.to_owned())
        .key(format!("autotune/title-{label}"))
        .role("heading")
        .level(1)
        .font(typography::TITLE)
        .color(color)
}

/// One numbered preparation tip.
fn tip(n: &str, body: impl Into<String>, t: TineTheme) -> El {
    row()
        .key(format!("autotune/tip-{n}"))
        .role("label")
        .label(format!("Tip {n}"))
        .gap(spacing::SM)
        .child(text(n).font(typography::LABEL).color(t.accent))
        .child(text(body).font(typography::BODY).color(t.text_dim))
}

/// The prompt sentence for the Speak phase (model override or the default).
fn speak_prompt(au: &AutoTuneUi) -> &str {
    if au.prompt_text.trim().is_empty() {
        SENTENCE
    } else {
        au.prompt_text.as_str()
    }
}

/// Countdown ring: shows the phase's remaining time as an unwinding arc.
fn countdown_ring(remaining_secs: f32, total_secs: f32, t: TineTheme) -> El {
    let d = 84.0;
    let r = 34.0;
    let remaining = (remaining_secs / total_secs).clamp(0.0, 1.0);

    let track = Shape::new().stroke(ring_points(r, 0.0, 1.0), 5.0, t.border);
    let arc = Shape::new().stroke(ring_points(r, 1.0 - remaining, 1.0), 5.0, t.accent);

    el().key("autotune/countdown")
        .role("timer")
        .label(format!("{remaining_secs:.1} s remaining"))
        .w(d)
        .h(d)
        .center()
        .child(rune::canvas(track).key("autotune/countdown-track"))
        .child(rune::canvas(arc).key("autotune/countdown-arc").absolute(0.0, 0.0))
        .child(
            text(format!("{remaining_secs:.1}s"))
                .key("autotune/countdown-readout")
                .role("label")
                .label(format!("{remaining_secs:.1} seconds remaining"))
                .absolute(28.0, 34.0)
                .font(typography::CAPTION)
                .color(t.text),
        )
}

/// Parametric circle polygon over `start..=end` (fractions of the circle,
/// starting at 12 o'clock).
fn ring_points(radius: f32, start: f32, end: f32) -> Vec<[f32; 2]> {
    let n = 48;
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let f = start + (end - start) * (i as f32 / n as f32);
        let angle = f * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        pts.push([radius + 8.0 + radius * angle.cos(), radius + 8.0 + radius * angle.sin()]);
    }
    pts
}

/// Horizontal live-level meter (-60..0 dBFS).
fn level_bar(level_db: f32, t: TineTheme) -> El {
    let norm = ((level_db + 60.0) / 60.0).clamp(0.0, 1.0);
    let color = if level_db > -3.0 {
        t.meter_red
    } else if level_db > -12.0 {
        t.meter_amber
    } else {
        t.meter_green
    };
    let bar = el()
        .key("autotune/level-track")
        .grow(1.0)
        .h(8.0)
        .radius(radii::SM)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(el().key("autotune/level-fill").grow(norm.max(0.001)).fill(color))
        .child(el().key("autotune/level-rest").grow((1.0 - norm).max(0.001)));

    row()
        .key("autotune/level")
        .role("meter")
        .label(format!("Live input {level_db:.1} dBFS"))
        .w_full()
        .gap(spacing::SM)
        .center()
        .child(text("Live").font(typography::CAPTION).color(t.text_dim))
        .child(bar)
        .child(
            text(format!("{level_db:.1} dBFS"))
                .font(typography::CAPTION)
                .color(t.text_dim),
        )
}

/// Determinate progress bar for the Analyzing phase.
fn progress_bar(progress: f32, t: TineTheme) -> El {
    let norm = progress.clamp(0.0, 1.0);
    el().key("autotune/progress")
        .role("progressbar")
        .label(format!("{:.0} % analyzed", norm * 100.0))
        .w_full()
        .h(8.0)
        .radius(radii::PILL)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(el().key("autotune/progress-fill").grow(norm.max(0.001)).fill(t.accent))
        .child(el().key("autotune/progress-rest").grow((1.0 - norm).max(0.001)))
}

/// Measured levels + the before → after parameter table.
fn results_card(model: &UiModel, t: TineTheme) -> El {
    let Some(cal) = &model.calibration else {
        return col().key("autotune/results").child(
            text("No calibration results available.")
                .font(typography::BODY)
                .color(t.danger),
        );
    };

    let mut card = col().key("autotune/results").w_full().gap(spacing::XS).child(
        row()
            .key("autotune/measurements")
            .role("group")
            .label("Measured noise floor, speech level and SNR")
            .w_full()
            .gap(spacing::LG)
            .child(measurement("Noise floor", format!("{:.1} dBFS", cal.noise_rms_db), t))
            .child(measurement("Speech level", format!("{:.1} dBFS", cal.speech_rms_db), t))
            .child(measurement("SNR", format!("{:.1} dB", snr_db(cal)), t))
            .child(measurement(
                "Verified peak",
                format!("{:.1} dBFS", cal.simulated_peak_db),
                t,
            )),
    );

    for diff in &cal.changes {
        card = card.child(diff_row(diff.param, diff.label, diff.before, diff.after, diff.unit, t));
    }

    card.child(
        text(format!(
            "Checked by simulating the full pipeline over your captured voice: output peak {:.1} dBFS, RMS {:.1} dBFS.",
            cal.simulated_peak_db, cal.simulated_rms_db
        ))
        .key("autotune/verified-note")
        .role("label")
        .label(format!(
            "Verified simulated output: peak {:.1} dBFS, RMS {:.1} dBFS",
            cal.simulated_peak_db, cal.simulated_rms_db
        ))
        .font(typography::CAPTION)
        .color(t.text_dim),
    )
}

fn measurement(label: &str, value: String, t: TineTheme) -> El {
    col()
        .key(format!("autotune/measure-{}", label.replace(' ', "-")))
        .role("label")
        .label(format!("{label}: {value}"))
        .gap(2.0)
        .child(text(label).font(typography::CAPTION).color(t.text_dim))
        .child(text(value).font(typography::LABEL).color(t.text))
}

fn diff_row(param: ParamId, label: &str, before: f32, after: f32, unit: &str, t: TineTheme) -> El {
    let value = |v: f32| {
        if unit.is_empty() {
            format!("{v:.2}")
        } else {
            format!("{v:.1} {unit}")
        }
    };
    let text_row = format!("{}: {} \u{2192} {}", label, value(before), value(after));
    row()
        .key(format!("autotune/diff-{}", param.spec().key))
        .role("label")
        .label(text_row.clone())
        .w_full()
        .gap(spacing::MD)
        .child(text(label.to_owned()).font(typography::CAPTION).color(t.text_dim))
        .child(el().key("autotune/diff-spacer").grow(1.0))
        .child(text(text_row).font(typography::CAPTION).color(t.text))
}

/// The "Keep my Pro pins" checkbox (D-007).
#[must_use]
pub fn keep_pins_checkbox(keep_pins: bool, t: TineTheme) -> El {
    row()
        .key("autotune/keep-pins")
        .role("checkbox")
        .checked(keep_pins)
        .label(format!(
            "Keep my Pro pins — {}",
            if keep_pins {
                "pinned parameters keep their Pro value"
            } else {
                "Auto-Tune retunes pinned parameters too"
            }
        ))
        .focusable(true)
        .on_press(|s: &mut AppState| {
            s.ui.autotune_keep_pins = !s.ui.autotune_keep_pins;
        })
        .child(
            text(if keep_pins { "[x]" } else { "[ ]" })
                .font(typography::CAPTION)
                .color(t.accent),
        )
        .child(text("Keep my Pro pins").font(typography::CAPTION).color(t.text_dim))
}

/// A row of wizard action buttons.
fn button_row(acts: &[(&'static str, &'static str, bool)], t: TineTheme) -> El {
    let mut r = row().key("autotune/actions").w_full().gap(spacing::SM);
    for (key, label, primary) in acts {
        r = r.child(button(key, label, *primary, t, action_for(key)));
    }
    r
}

/// Cancel-only row for the in-progress phases.
fn cancel_row(t: TineTheme) -> El {
    button_row(&[("autotune/cancel", "Cancel", false)], t)
}

/// The button tagged `autotune/apply` carries the live keep-pins state.
fn action_for(key: &str) -> impl Fn(&mut AppState) + 'static {
    match key {
        "autotune/cancel" | "autotune/close" => |s: &mut AppState| s.emit(Intent::CancelAutoTune),
        "autotune/discard" => |s: &mut AppState| s.emit(Intent::DiscardCalibration),
        // Retry restarts the wizard (the runner re-enters the Countdown phase).
        "autotune/retry" => |s: &mut AppState| s.emit(Intent::StartAutoTune),
        // TODO(T11): engine replays the CalibrationReport changes, honoring pins.
        "autotune/apply" => |s: &mut AppState| {
            s.emit(Intent::ApplyCalibration {
                keep_pins: s.ui.autotune_keep_pins,
            });
        },
        _ => |_s: &mut AppState| {},
    }
}

/// One primary/secondary pill button.
fn button(
    key: &'static str,
    label: &'static str,
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
    use tine_dsp::calibrator::{AutoCalibrator, COUNTDOWN_S};
    use tine_dsp::presets::builtin;
    use tine_protocol::params::DspParams;
    use tine_protocol::presets::{Archetype, PinSet};

    #[test]
    fn every_failure_has_a_concrete_remedy() {
        for failure in [
            CalFailureUi::TooQuiet,
            CalFailureUi::TooNoisy { snr_db: 7.2 },
            CalFailureUi::Clipping { pct: 0.021 },
            CalFailureUi::NotEnoughSpeech { voiced_ms: 800 },
            CalFailureUi::GainOutOfRange { needed_db: 41.0 },
        ] {
            let summary = failure.summary();
            let remedy = failure_remedy(failure);
            assert_ne!(summary, "");
            assert!(remedy.len() > 20, "remedy must be concrete: {remedy}");
        }
        // The T20 spec's example fix sentence for clipping.
        let clip = failure_remedy(CalFailureUi::Clipping { pct: 0.01 });
        assert!(clip.contains("clipping"), "{clip}");
    }

    #[test]
    fn snr_is_speech_minus_noise() {
        let c = CalibrationUi {
            noise_rms_db: -51.4,
            speech_rms_db: -21.5,
            simulated_peak_db: -4.2,
            simulated_rms_db: -16.8,
            changes: Vec::new(),
        };
        assert!((snr_db(&c) - 29.9).abs() < 1e-4);
    }

    /// Deterministic uniform noise in `[-1, 1]` (tiny LCG, no `rand` dep).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as i32) as f32 / (i32::MAX as f32)
        }
    }

    /// Drives the wizard adapter against the **real** `tine_dsp` calibrator
    /// (T11): a synthetic but spec-clean 48 kHz capture — 1 s countdown
    /// (discarded), 3 s room noise near -51 dBFS RMS, 5 s continuous 150 Hz
    /// sine at -23 dBFS RMS (fully voiced, ~26 dB SNR) — pushed in host-sized
    /// 480-sample blocks, exactly the way the capture thread (T16/T21) will.
    #[test]
    fn adapter_tracks_the_real_calibrator_state_machine() {
        const FS: f32 = 48_000.0;
        const BLOCK: usize = 480;
        let mut cal = AutoCalibrator::new(FS);

        // Countdown (first second, discarded): Prepare with a ~1 s ring.
        let mut p = cal.push(&[0.0; BLOCK]);
        assert_eq!(p.phase, CalPhase::Countdown);
        let ui = autotune_from_progress(&p, None);
        match ui.step {
            AutoTuneStep::Prepare { remaining_secs } => {
                assert!(
                    (0.98..=COUNTDOWN_S).contains(&remaining_secs),
                    "remaining {remaining_secs}"
                );
            }
            other => panic!("expected Prepare, got {other:?}"),
        }

        // Finish the countdown second: the next view is Silence (Noise phase)
        // with the full NOISE_S ring.
        for _ in 0..(FS as usize / BLOCK - 1) {
            p = cal.push(&[0.0; BLOCK]);
        }
        assert_eq!(p.phase, CalPhase::Noise);
        let ui = autotune_from_progress(&p, None);
        match ui.step {
            AutoTuneStep::Silence { remaining_secs } => {
                assert!(
                    (2.98..=SILENCE_SECS).contains(&remaining_secs),
                    "remaining {remaining_secs}"
                );
            }
            other => panic!("expected Silence, got {other:?}"),
        }
        assert_eq!(ui.prompt_text, String::new(), "runner fills the prompt");

        // Half the noise segment captured: the ring is at ~1.5 s and the
        // overall progress is measured / (NOISE_S + VOICE_S).
        let mut rng = Lcg(0x2545_F491_4F6C_DD1D);
        let mut noise_block = [0.0f32; BLOCK];
        for _ in 0..((SILENCE_SECS * 0.5 * FS / BLOCK as f32) as usize) {
            for s in &mut noise_block {
                *s = rng.next() * 0.005;
            }
            p = cal.push(&noise_block);
        }
        let ui = autotune_from_progress(&p, None);
        match ui.step {
            AutoTuneStep::Silence { remaining_secs } => {
                assert!((remaining_secs - 1.5).abs() < 0.06, "remaining {remaining_secs}");
            }
            other => panic!("expected Silence, got {other:?}"),
        }
        assert!((ui.progress - 1.5 / (SILENCE_SECS + SPEAK_SECS)).abs() < 0.02);

        // Second half of the noise segment: the phase flips to Voice next.
        for _ in 0..((SILENCE_SECS * 0.5 * FS / BLOCK as f32) as usize) {
            for s in &mut noise_block {
                *s = rng.next() * 0.005;
            }
            p = cal.push(&noise_block);
        }
        assert_eq!(p.phase, CalPhase::Voice);
        let mut speech_block = [0.0f32; BLOCK];
        let mut voiced = 0u32;
        for i in 0..((SPEAK_SECS * FS / BLOCK as f32) as usize) {
            for (j, s) in speech_block.iter_mut().enumerate() {
                let t = ((i * BLOCK + j) as u32 as f32) / FS;
                *s = 0.1 * (2.0 * core::f32::consts::PI * 150.0 * t).sin();
            }
            p = cal.push(&speech_block);
            voiced = voiced.max(p.voiced_ms);
            if i == (FS as usize / BLOCK) {
                // 1 s in: the Speak view mirrors the engine's `voiced_ms`.
                let ui = autotune_from_progress(&p, None);
                assert_eq!(ui.voiced_ms, p.voiced_ms);
                assert!(matches!(ui.step, AutoTuneStep::Speak { .. }));
                assert!(ui.voiced_ms > 0, "voiced {}", ui.voiced_ms);
            }
        }
        assert!(voiced >= 1_500, "voiced {voiced}");

        // 9 s captured: Analyzing latches and ignores further audio.
        assert_eq!(p.phase, CalPhase::Analyzing);
        p = cal.push(&speech_block);
        assert_eq!(p.phase, CalPhase::Analyzing);
        let ui = autotune_from_progress(&p, None);
        assert_eq!(ui.step, AutoTuneStep::Analyzing);
        assert!((ui.progress - 1.0).abs() < 1e-6, "progress {}", ui.progress);

        // finish() succeeds -> Done -> Results, and the report maps into the
        // results view model with exact measurements and 5 labelled diffs.
        let preset = builtin(Archetype::Dynamic);
        let report = cal
            .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
            .expect("synthetic capture must calibrate");
        p = cal.push(&[]);
        assert_eq!(p.phase, CalPhase::Done);
        let ui = autotune_from_progress(&p, None);
        assert_eq!(ui.step, AutoTuneStep::Results);

        let results = calibration_from_report(&report);
        assert_eq!(results.changes.len(), report.changes.len());
        assert_eq!(results.noise_rms_db, report.calibration.noise_rms_db);
        assert_eq!(results.speech_rms_db, report.calibration.speech_rms_db);
        assert_eq!(results.simulated_peak_db, report.simulated_peak_db);
        assert_eq!(results.simulated_rms_db, report.simulated_rms_db);
        for (diff, &(id, before, after)) in results.changes.iter().zip(report.changes.iter()) {
            assert_eq!(diff.param, id);
            assert!((diff.before - before).abs() < 1e-6);
            assert!((diff.after - after).abs() < 1e-6);
            assert!(!diff.label.is_empty(), "label for {id:?}");
        }
        // Uniform noise at +-0.005 has ~-50.8 dBFS RMS: a quiet room.
        assert!(results.noise_rms_db < -48.0);
        // The report's offline verification proves the -9..-6 dBFS window.
        assert!(
            (-9.0..=-6.0).contains(&results.simulated_peak_db),
            "simulated peak {}",
            results.simulated_peak_db
        );

        // A dead-mic capture (digital silence) fails exactly like the engine:
        // TooQuiet -> the Failed card, whose remedy names the mic.
        let mut dead = AutoCalibrator::new(FS);
        for _ in 0..9 * (FS as usize / BLOCK) {
            dead.push(&[0.0; BLOCK]);
        }
        let failure = dead
            .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
            .expect_err("silence must fail");
        p = dead.push(&[]);
        assert_eq!(p.phase, CalPhase::Failed);
        let ui = autotune_from_progress(&p, Some(failure));
        assert_eq!(ui.step, AutoTuneStep::Failed(CalFailureUi::TooQuiet));
        let remedy = failure_remedy(CalFailureUi::TooQuiet);
        assert!(remedy.contains("mic"), "remedy {remedy}");
    }
}
