//! Top deck console bar. Top row: brand lockup (`Tine DECK 2.3`), mic selector,
//! archetype segmented control and the one primary action (`Auto-Tune (8s)`). Bottom row:
//! virtual-mic status, denoiser selector, 5s Mic Check, Light/Dark theme switch,
//! "Hear Myself" monitor, Mute pill, and Decky-Deck `[⏻ POWER]` processing toggle.

use rune::{El, PopupMenu, el, popup_menu_supported, row, text};
use tine_protocol::params::DenoiseEngine;
use tine_protocol::presets::Archetype;

use crate::intent::Intent;
use crate::model::{ApoBadge, MicCheckStep};
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};
use crate::views::kit::{self, Tone, glyph};

const ARCHETYPES: [(Archetype, &str); 4] = [
    (Archetype::Dynamic, "Dynamic"),
    (Archetype::Condenser, "Condenser"),
    (Archetype::Headset, "Headset"),
    (Archetype::Laptop, "Laptop"),
];

/// Renders the top header bar.
///
/// Keyboard focus order (per `docs/project/05-ui-ux.md`):
/// mic selector → archetype pills → Auto-Tune → Hear Myself → A/B. Auxiliary pills
/// (`denoise-engine`, `mic-check`, `theme-toggle`, `monitor-device`, `mute`) stay
/// pointer-accessible without disturbing the documented Tab order.
#[must_use]
pub fn view(s: &AppState, t: TineTheme) -> El {
    kit::card("header", t)
        .px(spacing::LG)
        .py(spacing::SM + 2.0)
        .gap(spacing::SM + 2.0)
        .child(
            row()
                .key("header/top-row")
                .w_full()
                .gap(spacing::MD)
                .center()
                .child(kit::brand(t))
                .child(divider("header/divider-1", t))
                .child(mic_selector(s, t))
                .child(el().key("header/spacer-1").grow(1.0))
                .child(archetype_pills(s, t))
                .child(kit::button(
                    "header/autotune",
                    "button",
                    "Auto-Tune (8s)",
                    Tone::Primary,
                    Some(&glyph::SPARKLE),
                    t,
                    |s| {
                        s.emit(Intent::StartAutoTune);
                    },
                )),
        )
        .child(el().key("header/rule").w_full().h(1.0).fill(t.border))
        .child(
            row()
                .key("header/bottom-row")
                .w_full()
                .center()
                .gap(spacing::XS + 2.0)
                .child(apo_status(s, t))
                .child(denoise_selector(s, t))
                .child(mic_check_pill(t))
                .child(theme_toggle_pill(s, t))
                .child(el().key("header/spacer-2").grow(1.0))
                .child(monitor_group(s, t))
                .child(divider("header/divider-2", t))
                .child(mute_pill(s, t))
                .child(bypass_switch(s, t)),
        )
}

/// A short vertical hairline between control groups.
fn divider(key: &'static str, t: TineTheme) -> El {
    el().key(key).w(1.0).h(20.0).fill(t.border_strong)
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let mut chars = name.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Mic selector: shows the selected microphone and opens a native dropdown menu.
fn mic_selector(s: &AppState, t: TineTheme) -> El {
    let mic_name = s
        .model
        .selected
        .and_then(|idx| s.model.endpoints.get(idx))
        .map_or("No Microphone", |e| e.name.as_str());
    let short = truncate_name(mic_name, 22);
    let label = format!("Mic: {short} ▾");
    let count = s.model.endpoints.len();

    row()
        .key("header/mic-selector")
        .role("combobox")
        .label(label)
        .focusable(true)
        .h(kit::CONTROL_H)
        .pl(spacing::MD)
        .pr(spacing::SM)
        .gap(spacing::SM)
        .center()
        .radius(radii::PILL)
        .fill(t.surface_raised)
        .border(1.0, t.border)
        .hover(t.surface_hover)
        .press_scale(0.98)
        .cursor(rune::Cursor::Pointer)
        .on_press_tx(move |s: &mut AppState, tx: &mut rune::Tx| {
            // Without native menu support (non-Windows/macOS hosts) fall back to
            // cycling through the endpoints so the control still does something.
            if !popup_menu_supported() {
                if count > 0 {
                    let next = s.model.selected.map_or(0, |i| (i + 1) % count);
                    if let Some(ep) = s.model.endpoints.get(next) {
                        let id = ep.id.clone();
                        s.model.selected = Some(next);
                        s.emit(Intent::SelectEndpoint(id));
                    }
                }
                return;
            }
            let selected_id = s
                .model
                .selected
                .and_then(|idx| s.model.endpoints.get(idx))
                .map(|e| e.id.clone());
            let menu = PopupMenu::picker(
                s.model.endpoints.iter().map(|e| (e.id.clone(), e.name.clone())),
                selected_id.as_deref().unwrap_or(""),
            );
            tx.popup_menu(
                menu,
                "header/mic-selector",
                |s: &mut AppState, picked: Option<&str>, _tx| {
                    if let Some(id) = picked
                        && let Some((idx, ep)) = s.model.endpoints.iter().enumerate().find(|(_, e)| e.id == id)
                    {
                        let ep_id = ep.id.clone();
                        s.model.selected = Some(idx);
                        s.emit(Intent::SelectEndpoint(ep_id));
                    }
                },
            );
        })
        .child(kit::glyph_el(&glyph::MIC, 15.0, t.deck_blue_text))
        .child(text(short).font(typography::LABEL).color(t.text))
        .child(kit::glyph_el(&glyph::CHEVRON_DOWN, 14.0, t.text_dim))
}

/// Virtual Cable routing status chip with a state-dependent action button.
fn apo_status(s: &AppState, t: TineTheme) -> El {
    let dot_color = match s.model.apo {
        ApoBadge::Active => t.success,
        ApoBadge::Inactive => t.meter_amber,
        ApoBadge::NotInstalled | ApoBadge::Error(_) => t.danger,
    };
    let status_text = match &s.model.apo {
        ApoBadge::Active => String::from("Cable: Ready"),
        ApoBadge::NotInstalled => String::from("Cable: Missing"),
        ApoBadge::Inactive => String::from("Cable: Inactive"),
        ApoBadge::Error(msg) => format!("Cable: {}", truncate_name(msg, 20)),
    };

    let mut group = row()
        .key("header/apo-group")
        .gap(spacing::XS + 2.0)
        .center()
        .child(kit::status_chip("header/apo-status", status_text, dot_color, t));

    match s.model.apo {
        ApoBadge::NotInstalled => {
            group = group.child(kit::button(
                "header/apo-install",
                "button",
                "Check",
                Tone::Selected,
                None,
                t,
                |s| {
                    s.emit(Intent::InstallApo);
                },
            ));
        }
        ApoBadge::Inactive | ApoBadge::Error(_) => {
            group = group.child(kit::button(
                "header/apo-repair",
                "button",
                "Recheck",
                Tone::Selected,
                None,
                t,
                |s| {
                    s.emit(Intent::RepairApo);
                },
            ));
        }
        ApoBadge::Active => {}
    }

    group
}

/// Compact Stage 2 denoiser backend picker (`RNNoise` / `Studio Clean` / `Low-Lat`).
/// Not a Tab stop so the documented header focus order stays unchanged.
fn denoise_selector(s: &AppState, t: TineTheme) -> El {
    let current = s.model.denoise_engine;
    let short = match current {
        DenoiseEngine::Rnnoise => "RNNoise",
        DenoiseEngine::DeepFilterNet3 => "Studio",
        DenoiseEngine::Off => "Low-Lat",
    };
    let fg = if current == DenoiseEngine::Rnnoise {
        t.text_dim
    } else {
        t.accent_text
    };
    let bg = if current == DenoiseEngine::Rnnoise {
        t.well
    } else {
        t.accent_soft
    };
    let border = if current == DenoiseEngine::Rnnoise {
        t.border
    } else {
        t.accent_glow
    };

    row()
        .key("header/denoise-engine")
        .role("combobox")
        .label(format!("Denoiser: {}", current.display_name()))
        .h(kit::CONTROL_H - 4.0)
        .pl(spacing::SM)
        .pr(spacing::XS + 2.0)
        .gap(4.0)
        .center()
        .radius(radii::PILL)
        .fill(bg)
        .border(1.0, border)
        .hover(t.surface_raised)
        .focusable(false)
        .cursor(rune::Cursor::Pointer)
        .on_press_tx(move |s: &mut AppState, tx: &mut rune::Tx| {
            if !popup_menu_supported() {
                let next = match s.model.denoise_engine {
                    DenoiseEngine::Rnnoise => DenoiseEngine::DeepFilterNet3,
                    DenoiseEngine::DeepFilterNet3 => DenoiseEngine::Off,
                    DenoiseEngine::Off => DenoiseEngine::Rnnoise,
                };
                s.model.denoise_engine = next;
                s.emit(Intent::SetDenoiseEngine(next));
                return;
            }
            let choices = DenoiseEngine::ALL
                .iter()
                .map(|e| (e.as_str().to_owned(), e.display_name().to_owned()));
            let active = s.model.denoise_engine.as_str().to_owned();
            let menu = PopupMenu::picker(choices, &active);
            tx.popup_menu(
                menu,
                "header/denoise-engine",
                |s: &mut AppState, picked: Option<&str>, _tx| {
                    if let Some(id) = picked
                        && let Some(engine) = DenoiseEngine::from_name(id)
                    {
                        s.model.denoise_engine = engine;
                        s.emit(Intent::SetDenoiseEngine(engine));
                    }
                },
            );
        })
        .child(text(short).font(typography::CAPTION).color(fg))
        .child(kit::glyph_el(&glyph::CHEVRON_DOWN, 12.0, fg))
}

/// Tactile Light / Dark theme mode toggle pill (not a Tab stop).
fn theme_toggle_pill(s: &AppState, t: TineTheme) -> El {
    let is_light = s.ui.theme_mode.is_light();
    let (caption, lead, a11y_label) = if is_light {
        ("Light", &glyph::SUN, "Theme: Light mode (switch to Dark)")
    } else {
        ("Dark", &glyph::MOON, "Theme: Dark mode (switch to Light)")
    };
    row()
        .key("header/theme-toggle")
        .role("switch")
        .checked(is_light)
        .label(a11y_label)
        .h(kit::CONTROL_H - 4.0)
        .px(spacing::SM)
        .gap(5.0)
        .center()
        .radius(radii::PILL)
        .fill(if is_light { t.deck_blue_soft } else { t.well })
        .border(1.0, if is_light { t.deck_blue } else { t.border })
        .hover(t.surface_raised)
        .press_scale(0.96)
        .focusable(false)
        .cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut AppState| {
            s.toggle_theme();
        })
        .child(kit::glyph_el(
            lead,
            13.0,
            if is_light { t.deck_blue_text } else { t.text_dim },
        ))
        .child(
            text(caption)
                .font(typography::CAPTION)
                .color(if is_light { t.deck_blue_text } else { t.text }),
        )
}

/// Compact 5-second Mic Check A/B audition pill (not a Tab stop).
fn mic_check_pill(t: TineTheme) -> El {
    row()
        .key("header/mic-check")
        .role("button")
        .label("Mic Check (5s A/B)")
        .h(kit::CONTROL_H - 4.0)
        .px(spacing::SM)
        .center()
        .radius(radii::PILL)
        .fill(t.well)
        .border(1.0, t.border)
        .hover(t.surface_raised)
        .press_scale(0.96)
        .focusable(false)
        .cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut AppState| {
            s.model.mic_check = MicCheckStep::Recording {
                progress: 0.0,
                level_db: -60.0,
            };
            s.emit(Intent::StartMicCheck);
        })
        .child(text("Mic Check").font(typography::CAPTION).color(t.text_dim))
}

/// Compact RT output mute toggle pill (not a Tab stop; matches `Ctrl+Shift+M`).
fn mute_pill(s: &AppState, t: TineTheme) -> El {
    let muted = s.model.muted;
    let label = if muted {
        "Muted (Ctrl+Shift+M)"
    } else {
        "Mute (Ctrl+Shift+M)"
    };
    let caption = if muted { "Muted" } else { "Mute" };
    let (bg, border, fg) = if muted {
        (rune::Color { a: 0.18, ..t.danger }, t.danger, t.text)
    } else {
        (t.well, t.border, t.text_dim)
    };
    row()
        .key("header/mute")
        .role("switch")
        .checked(muted)
        .label(label)
        .h(kit::CONTROL_H - 4.0)
        .px(spacing::SM)
        .center()
        .radius(radii::PILL)
        .fill(bg)
        .border(1.0, border)
        .hover(t.surface_raised)
        .press_scale(0.96)
        .focusable(false)
        .cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut AppState| {
            let next = !s.model.muted;
            s.model.muted = next;
            s.emit(Intent::SetMuted(next));
        })
        .child(text(caption).font(typography::CAPTION).color(fg))
}

/// Segmented archetype control (one track, the selected mic type raised).
fn archetype_pills(s: &AppState, t: TineTheme) -> El {
    let mut group = kit::segmented("header/archetypes", "radiogroup", "Microphone archetype", t);
    for (arch, name) in ARCHETYPES {
        let key = match arch {
            Archetype::Dynamic => "header/arch-dynamic",
            Archetype::Condenser => "header/arch-condenser",
            Archetype::Headset => "header/arch-headset",
            Archetype::Laptop => "header/arch-laptop",
        };
        let selected = s.model.archetype == arch;
        group = group.child(kit::segment(key, "radio", name, selected, t, move |s| {
            s.set_archetype_optimistic(arch);
        }));
    }
    group
}

/// Decky-Deck `[⏻ POWER]` processing / A/B bypass toggle (`key = "header/bypass"`, Tab stop).
fn bypass_switch(s: &AppState, t: TineTheme) -> El {
    let on = !s.model.bypass;
    let (label, caption) = if on {
        ("Sound: Tine (On)", "TINE ON")
    } else {
        ("Sound: Raw (Off)", "RAW MIC")
    };
    let bg = if on { t.surface_raised } else { t.well };
    let border = if on { t.deck_blue } else { t.border };
    let icon_color = if on { t.deck_blue_text } else { t.text_dim };

    row()
        .key("header/bypass")
        .role("switch")
        .checked(on)
        .label(label)
        .focusable(true)
        .h(kit::CONTROL_H)
        .px(spacing::MD)
        .gap(6.0)
        .center()
        .radius(radii::PILL)
        .fill(bg)
        .border(1.0, border)
        .hover(t.surface_hover)
        .press_scale(0.96)
        .cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut AppState| {
            s.toggle_bypass_optimistic();
        })
        .child(kit::glyph_el(&glyph::POWER, 14.0, icon_color))
        .child(
            text(caption)
                .font(typography::CAPTION)
                .color(if on { t.text } else { t.text_dim }),
        )
}

/// "Hear Myself" switch with latency note and playback-device picker.
fn monitor_group(s: &AppState, t: TineTheme) -> El {
    let monitor = &s.model.monitor;

    let mut group = row()
        .key("header/monitor-group")
        .role("group")
        .label("Hear Myself")
        .gap(spacing::XS)
        .center();

    group = group.child(kit::switch(
        "header/monitor",
        "Hear Myself",
        "Hear Myself",
        monitor.enabled,
        t,
        |s| {
            s.model.monitor.enabled = !s.model.monitor.enabled;
            s.emit(Intent::ToggleMonitor);
        },
    ));

    if monitor.enabled {
        // Nominal round-trip delay is ~42 ms (21.6 ms DSP + WASAPI); only warn red if > 60 ms.
        let latency = monitor.estimated_latency_ms;
        let (color, note) = if latency > 60.0 {
            (t.danger, format!("⚠ ~{latency:.0}ms"))
        } else {
            (t.text_dim, format!("~{latency:.0}ms"))
        };
        group = group.child(
            text(note)
                .key("header/monitor-latency")
                .font(typography::CAPTION)
                .color(color),
        );
    }

    let device_label = monitor
        .device_name
        .clone()
        .map_or_else(|| String::from("Choose…"), |d| d.clone());

    // Playback-device popover. Not a Tab stop (keep the documented focus order);
    // still pressable via pointer and assistive technology.
    group.child(
        row()
            .key("header/monitor-device")
            .role("button")
            .label(format!("Playback: {device_label}"))
            .h(kit::CONTROL_H - 4.0)
            .pl(spacing::SM)
            .pr(spacing::XS + 2.0)
            .gap(5.0)
            .center()
            .radius(radii::PILL)
            .fill(t.well)
            .border(1.0, t.border)
            .hover(t.surface_raised)
            .press_scale(0.97)
            .focusable(false)
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(move |s: &mut AppState, tx: &mut rune::Tx| {
                if !popup_menu_supported() {
                    s.emit(Intent::OpenSettings);
                    return;
                }
                // Clone the device list; core audio controls can mutate on other threads.
                let devices: Vec<(String, String)> = s
                    .model
                    .monitor
                    .available_devices
                    .iter()
                    .map(|d| (d.clone(), d.clone()))
                    .collect();
                let current = s.model.monitor.device_name.clone().unwrap_or_default();
                let menu = PopupMenu::picker(devices, &current);
                tx.popup_menu(
                    menu,
                    "header/monitor-device",
                    |s: &mut AppState, picked: Option<&str>, _tx| {
                        if let Some(name) = picked {
                            s.model.monitor.device_name = Some(name.to_owned());
                            s.emit(Intent::PickMonitorDevice(name.to_owned()));
                        }
                    },
                );
            })
            .child(kit::glyph_el(&glyph::HEADPHONES, 14.0, t.text_dim))
            .child(
                text(truncate_name(&device_label, 12))
                    .font(typography::CAPTION)
                    .color(t.text_dim),
            )
            .child(kit::glyph_el(&glyph::CHEVRON_DOWN, 13.0, t.text_dim)),
    )
}
