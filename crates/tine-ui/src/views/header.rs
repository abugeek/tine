//! Top header bar: mic selector dropdown, APO status badge with Install/Repair actions,
//! archetype segmented pills, Auto-Tune, "Hear Myself" monitor with latency badge and
//! device popover, and the A/B bypass switch.

use rune::{El, PopupMenu, col, el, popup_menu_supported, row, text};
use tine_protocol::presets::Archetype;

use crate::intent::Intent;
use crate::model::ApoBadge;
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

const ARCHETYPES: [(Archetype, &str); 4] = [
    (Archetype::Dynamic, "Dynamic"),
    (Archetype::Condenser, "Condenser"),
    (Archetype::Headset, "Headset"),
    (Archetype::Laptop, "Laptop"),
];

/// Renders the top header bar.
///
/// Keyboard focus order (per `docs/project/05-ui-ux.md`):
/// mic selector → archetype pills → Auto-Tune → Hear Myself → A/B. The device picker
/// pill is intentionally not a Tab stop; it stays reachable with the pointer and via
/// assistive technology without disturbing the documented order.
#[must_use]
pub fn view(s: &AppState, t: TineTheme) -> El {
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
                .child(mic_selector(s, t))
                .child(apo_status(s, t))
                .child(el().key("header/spacer-1").grow(1.0))
                .child(archetype_pills(s, t))
                .child(pill_button(
                    "header/autotune",
                    "button",
                    "Auto-Tune (8s)",
                    false,
                    t,
                    |s| {
                        s.emit(Intent::StartAutoTune);
                    },
                )),
        )
        .child(
            row()
                .key("header/bottom-row")
                .w_full()
                .gap(spacing::SM)
                .child(el().key("header/spacer-2").grow(1.0))
                .child(monitor_group(s, t))
                .child(bypass_switch(s, t)),
        )
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

/// Mic selector: shows the selected microphone and opens a native dropdown menu.
fn mic_selector(s: &AppState, t: TineTheme) -> El {
    let mic_name = s
        .model
        .selected
        .and_then(|idx| s.model.endpoints.get(idx))
        .map_or("No Microphone", |e| e.name.as_str());
    let label = format!("Mic: {mic_name} ▾");
    let count = s.model.endpoints.len();

    row()
        .key("header/mic-selector")
        .role("combobox")
        .label(label.clone())
        .focusable(true)
        .h(30.0)
        .px(spacing::MD)
        .center()
        .radius(radii::PILL)
        .fill(t.surface_raised)
        .border(1.0, t.border)
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
        .child(text(label).font(typography::LABEL).color(t.text))
}

/// Virtual Cable routing status badge with state-dependent action button.
fn apo_status(s: &AppState, t: TineTheme) -> El {
    let dot_color = match s.model.apo {
        ApoBadge::Active => t.meter_green,
        ApoBadge::Inactive => t.meter_amber,
        ApoBadge::NotInstalled | ApoBadge::Error(_) => t.danger,
    };
    let status_text = match &s.model.apo {
        ApoBadge::Active => String::from("Cable: Active"),
        ApoBadge::NotInstalled => String::from("Cable: Missing"),
        ApoBadge::Inactive => String::from("Cable: Inactive"),
        ApoBadge::Error(msg) => format!("Cable: Error — {msg}"),
    };

    let mut apo_row = row()
        .key("header/apo-status")
        .role("status")
        .label(status_text.clone())
        .gap(spacing::XS)
        .center()
        .px(spacing::SM)
        .h(30.0)
        .radius(radii::MD)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(el().key("header/apo-dot").size(8.0, 8.0).radius(4.0).fill(dot_color))
        .child(text(status_text).font(typography::LABEL).color(t.text));

    match s.model.apo {
        ApoBadge::NotInstalled => {
            apo_row = apo_row.child(pill_button(
                "header/apo-install",
                "button",
                "Check Setup",
                true,
                t,
                |s| {
                    s.emit(Intent::InstallApo);
                },
            ));
        }
        ApoBadge::Inactive | ApoBadge::Error(_) => {
            apo_row = apo_row.child(pill_button("header/apo-repair", "button", "Recheck", true, t, |s| {
                s.emit(Intent::RepairApo);
            }));
        }
        ApoBadge::Active => {}
    }

    apo_row
}

/// Segmented archetype pill group.
fn archetype_pills(s: &AppState, t: TineTheme) -> El {
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

    arch_group
}

fn bypass_switch(s: &AppState, t: TineTheme) -> El {
    let label = if s.model.bypass { "A/B: Raw" } else { "A/B: Tine" };
    pill_button("header/bypass", "switch", label, !s.model.bypass, t, |s| {
        s.toggle_bypass_optimistic();
    })
}

/// "Hear Myself" switch with latency badge and playback-device popover.
fn monitor_group(s: &AppState, t: TineTheme) -> El {
    let monitor = &s.model.monitor;

    let mut group = row()
        .key("header/monitor-group")
        .role("group")
        .label("Hear Myself")
        .gap(spacing::XS)
        .center();

    group = group.child(pill_button(
        "header/monitor",
        "switch",
        "Hear Myself",
        monitor.enabled,
        t,
        |s| {
            s.model.monitor.enabled = !s.model.monitor.enabled;
            s.emit(Intent::ToggleMonitor);
        },
    ));

    if monitor.enabled {
        // Latency warning badge while monitoring is live.
        let latency = monitor.estimated_latency_ms;
        let over_budget = latency > 23.0;
        let (color, note) = if over_budget {
            (t.danger, format!("⚠ ~{latency:.0} ms"))
        } else {
            (t.meter_amber, format!("~{latency:.0} ms"))
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
        .map_or_else(|| String::from("Choose device…"), |d| d.clone());

    // Playback-device popover. Not a Tab stop (keep the documented focus order);
    // still pressable via pointer and assistive technology.
    group = group.child(
        row()
            .key("header/monitor-device")
            .role("button")
            .label(format!("Playback: {device_label}"))
            .h(30.0)
            .px(spacing::SM)
            .center()
            .radius(radii::MD)
            .fill(t.surface_raised)
            .border(1.0, t.border)
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
            .child(
                text(format!("{device_label} ▾"))
                    .font(typography::CAPTION)
                    .color(t.text_dim),
            ),
    );

    group
}
