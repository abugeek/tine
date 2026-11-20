//! 5-second Mic Check modal overlay (sample-aligned Raw vs Tine A/B audition).

use rune::{El, col, el, row, text};

use crate::intent::Intent;
use crate::model::MicCheckStep;
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};
use crate::views::kit::{self, Tone};

/// Renders the focus-trapping 5-second Mic Check modal card.
#[must_use]
pub fn view(s: &AppState, t: TineTheme) -> El {
    let body = match &s.model.mic_check {
        MicCheckStep::Idle => el().key("mic-check/idle"),
        MicCheckStep::Recording { progress, level_db } => recording_body(*progress, *level_db, t),
        MicCheckStep::Playing {
            raw,
            progress,
            duration_s,
        } => playing_body(*raw, *progress, *duration_s, t),
        MicCheckStep::Failed(msg) => failed_body(msg, t),
    };

    el().key("mic-check-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(t.scrim)
        .center()
        .child(
            kit::card("mic-check-card", t)
                .w(480.0)
                .px(spacing::XL)
                .py(spacing::XL)
                .gap(spacing::MD)
                .border(1.0, t.border_strong)
                .child(
                    row()
                        .key("mic-check/header")
                        .w_full()
                        .center()
                        .child(text("Mic Check · 5s A/B").font(typography::TITLE).color(t.text))
                        .child(el().key("mic-check/spacer").grow(1.0))
                        .child(kit::button(
                            "mic-check/close",
                            "button",
                            "Close",
                            Tone::Secondary,
                            None,
                            t,
                            |s| {
                                s.model.mic_check = MicCheckStep::Idle;
                                s.emit(Intent::StopMicCheck);
                            },
                        )),
                )
                .child(body),
        )
}

fn recording_body(progress: f32, level_db: f32, t: TineTheme) -> El {
    let remaining = ((1.0 - progress.clamp(0.0, 1.0)) * 5.0).max(0.0);
    let level_norm = ((level_db + 60.0) / 60.0).clamp(0.0, 1.0);
    col()
        .key("mic-check/recording")
        .w_full()
        .gap(spacing::MD)
        .child(
            text(format!("Speak normally for 5 seconds ({remaining:.1} s left)…"))
                .font(typography::BODY)
                .color(t.text_dim)
                .wrap(),
        )
        .child(meter_bar("mic-check/progress-bar", "Progress", progress, t.accent, t))
        .child(meter_bar(
            "mic-check/level-bar",
            &format!("Input {level_db:.1} dBFS"),
            level_norm,
            t.meter_green,
            t,
        ))
        .child(
            row()
                .key("mic-check/rec-actions")
                .w_full()
                .gap(spacing::SM)
                .child(kit::button(
                    "mic-check/cancel",
                    "button",
                    "Cancel",
                    Tone::Secondary,
                    None,
                    t,
                    |s| {
                        s.model.mic_check = MicCheckStep::Idle;
                        s.emit(Intent::StopMicCheck);
                    },
                )),
        )
}

fn playing_body(raw: bool, progress: f32, duration_s: f32, t: TineTheme) -> El {
    let time_pos = (progress.clamp(0.0, 1.0) * duration_s).clamp(0.0, duration_s);
    col()
        .key("mic-check/playing")
        .w_full()
        .gap(spacing::MD)
        .child(
            text("Switch live between your unprocessed mic and Tine (sample-aligned, zero pause).")
                .font(typography::BODY)
                .color(t.text_dim)
                .wrap(),
        )
        .child(
            row()
                .key("mic-check/ab-switch")
                .w_full()
                .gap(spacing::SM)
                .center()
                .child(kit::button(
                    "mic-check/mode-tine",
                    "button",
                    "Tine (Processed)",
                    if raw { Tone::Secondary } else { Tone::Primary },
                    None,
                    t,
                    |s| {
                        if let MicCheckStep::Playing {
                            progress, duration_s, ..
                        } = s.model.mic_check
                        {
                            s.model.mic_check = MicCheckStep::Playing {
                                raw: false,
                                progress,
                                duration_s,
                            };
                        }
                        s.emit(Intent::SetMicCheckMode(false));
                    },
                ))
                .child(kit::button(
                    "mic-check/mode-raw",
                    "button",
                    "Raw Mic",
                    if raw { Tone::Primary } else { Tone::Secondary },
                    None,
                    t,
                    |s| {
                        if let MicCheckStep::Playing {
                            progress, duration_s, ..
                        } = s.model.mic_check
                        {
                            s.model.mic_check = MicCheckStep::Playing {
                                raw: true,
                                progress,
                                duration_s,
                            };
                        }
                        s.emit(Intent::SetMicCheckMode(true));
                    },
                )),
        )
        .child(meter_bar(
            "mic-check/playback-bar",
            &format!("Loop {time_pos:.1} s / {duration_s:.1} s"),
            progress,
            if raw { t.meter_amber } else { t.accent },
            t,
        ))
        .child(
            row()
                .key("mic-check/play-actions")
                .w_full()
                .gap(spacing::SM)
                .child(kit::button(
                    "mic-check/rerecord",
                    "button",
                    "Re-record (5s)",
                    Tone::Secondary,
                    None,
                    t,
                    |s| {
                        s.model.mic_check = MicCheckStep::Recording {
                            progress: 0.0,
                            level_db: -60.0,
                        };
                        s.emit(Intent::StartMicCheck);
                    },
                ))
                .child(el().key("mic-check/play-spacer").grow(1.0))
                .child(kit::button(
                    "mic-check/done",
                    "button",
                    "Done",
                    Tone::Primary,
                    None,
                    t,
                    |s| {
                        s.model.mic_check = MicCheckStep::Idle;
                        s.emit(Intent::StopMicCheck);
                    },
                )),
        )
}

fn failed_body(msg: &str, t: TineTheme) -> El {
    col()
        .key("mic-check/failed")
        .w_full()
        .gap(spacing::MD)
        .child(
            text(msg.to_owned())
                .key("mic-check/failure-message")
                .font(typography::BODY)
                .color(t.danger)
                .wrap(),
        )
        .child(
            row()
                .key("mic-check/failed-actions")
                .w_full()
                .gap(spacing::SM)
                .child(kit::button(
                    "mic-check/retry",
                    "button",
                    "Try Again",
                    Tone::Primary,
                    None,
                    t,
                    |s| {
                        s.model.mic_check = MicCheckStep::Recording {
                            progress: 0.0,
                            level_db: -60.0,
                        };
                        s.emit(Intent::StartMicCheck);
                    },
                )),
        )
}

fn meter_bar(key: &'static str, label: &str, frac: f32, fill: rune::Color, t: TineTheme) -> El {
    let norm = frac.clamp(0.0, 1.0);
    col()
        .key(key)
        .w_full()
        .gap(spacing::XS)
        .child(text(label.to_owned()).font(typography::CAPTION).color(t.text_dim))
        .child(
            row()
                .key(format!("{key}/track"))
                .w_full()
                .h(8.0)
                .radius(radii::SM)
                .fill(t.well)
                .border(1.0, t.border)
                .child(
                    el().key(format!("{key}/fill"))
                        .grow(norm.max(0.001))
                        .h_full()
                        .radius(radii::SM)
                        .fill(if norm > 0.001 { fill } else { t.well }),
                )
                .child(el().key(format!("{key}/rest")).grow((1.0 - norm).max(0.001))),
        )
}
