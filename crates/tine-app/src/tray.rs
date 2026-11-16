//! `rune::Tray` wrapper: icon generation, flat menu with checked-state-in-label
//! (rune trays are flat: `Action` and `Separator` only), and tooltip.
//!
//! `rune::Tray` exposes no `set_icon`, so the icon variant is selected at
//! creation time; the shell recreates the tray when the variant changes
//! (create-new-then-drop-old keeps an icon on screen the whole time).

use rune::{Tray, TrayEvent, TrayIcon, TrayItem, TraySpec};

use tine_protocol::presets::Archetype;

/// Which icon variant the tray shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayVariant {
    /// DSP processing (accent).
    Active,
    /// Bypassed (amber).
    Bypassed,
    /// Lost link / error (red).
    Error,
}

/// Everything the tray menu and tooltip display.
#[derive(Clone, Debug, PartialEq)]
pub struct TraySnapshot {
    /// Friendly mic (endpoint) name.
    pub mic_name: String,
    /// Endpoint id (fallback display).
    pub endpoint_id: Option<String>,
    /// Any processing state linked?
    pub linked: bool,
    /// Bypass A/B state.
    pub bypass: bool,
    /// "Hear Myself" monitor enabled.
    pub monitor: bool,
    /// Current archetype.
    pub archetype: Archetype,
    /// HKCU Run autostart enabled.
    pub autostart: bool,
}

impl TraySnapshot {
    /// Fallback snapshot before the first controller event arrives.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            mic_name: "No microphone selected".to_owned(),
            endpoint_id: None,
            linked: false,
            bypass: false,
            monitor: false,
            archetype: Archetype::default(),
            autostart: false,
        }
    }

    /// The icon variant for the current state.
    #[must_use]
    pub fn variant(&self) -> TrayVariant {
        match (self.linked, self.bypass) {
            (false, _) => TrayVariant::Error,
            (true, true) => TrayVariant::Bypassed,
            (true, false) => TrayVariant::Active,
        }
    }

    /// Tooltip: mic name + state (spec).
    #[must_use]
    pub fn tooltip(&self) -> String {
        let mic = if self.mic_name.is_empty() {
            self.endpoint_id.clone().unwrap_or_else(|| "no mic".to_owned())
        } else {
            self.mic_name.clone()
        };
        let state = if !self.linked {
            "inactive"
        } else if self.bypass {
            "bypassed"
        } else {
            "processing"
        };
        format!("Tine — {mic} ({state})")
    }
}

/// Menu item ids (rune sends these back on `TrayEvent::Menu`).
pub mod menu_id {
    /// Open the UI window.
    pub const OPEN: &str = "open";
    /// Toggle A/B bypass.
    pub const BYPASS: &str = "bypass";
    /// Toggle "Hear Myself".
    pub const MONITOR: &str = "monitor";
    /// Toggle HKCU Run autostart.
    pub const AUTOSTART: &str = "autostart";
    /// Archetype items: id prefix, e.g. `arch:dynamic`.
    pub const ARCH_PREFIX: &str = "arch:";
    /// Quit.
    pub const QUIT: &str = "quit";
}

/// Builds the flat tray menu (checked state in the label text).
#[must_use]
pub fn build_menu(s: &TraySnapshot) -> Vec<TrayItem> {
    let bypass_label = if s.bypass { "Bypassed" } else { "✓ Processing" };
    let monitor_label = if s.monitor { "✓ Hear Myself" } else { "Hear Myself" };
    let autostart_label = if s.autostart {
        "✓ Start with Windows"
    } else {
        "Start with Windows"
    };
    let mut items = vec![
        TrayItem::action(menu_id::OPEN, "Open Tine"),
        TrayItem::action(menu_id::BYPASS, bypass_label),
        TrayItem::action(menu_id::MONITOR, monitor_label),
        TrayItem::Separator,
    ];
    for a in [
        Archetype::Dynamic,
        Archetype::Condenser,
        Archetype::Headset,
        Archetype::Laptop,
    ] {
        let label = if s.archetype == a {
            format!("✓ {}", a.display_name())
        } else {
            a.display_name().to_owned()
        };
        items.push(TrayItem::action(format!("{}{}", menu_id::ARCH_PREFIX, a.id()), label));
    }
    items.push(TrayItem::Separator);
    items.push(TrayItem::action(menu_id::AUTOSTART, autostart_label));
    items.push(TrayItem::Separator);
    items.push(TrayItem::action(menu_id::QUIT, "Quit"));
    items
}

/// Tuning-fork silhouette test (24×24 pixel space): two prongs, a bridge,
/// a stem. Plain range checks so the test stays `const`.
const fn fork_glyph(x: u32, y: u32) -> bool {
    let in_prong = x >= 6 && x <= 8;
    let in_prong_r = x >= 15 && x <= 17;
    let in_bridge_span = x >= 6 && x <= 17;
    let in_bridge_h = y >= 13 && y <= 15;
    let in_stem_h = x >= 11 && x <= 12;
    let in_body = y >= 3 && y <= 13;
    let in_stem = y >= 15 && y <= 21;
    let prong_l = in_prong && in_body;
    let prong_r = in_prong_r && in_body;
    let bridge = in_bridge_span && in_bridge_h;
    let stem = in_stem_h && in_stem;
    prong_l || prong_r || bridge || stem
}

/// Generates the tuning-fork glyph pixels in the variant's palette
/// (runtime-generated RGBA, 24×24 — no external asset needed).
#[must_use]
pub fn fork_pixels(variant: TrayVariant) -> (u32, u32, Vec<u8>) {
    const W: u32 = 24;
    const H: u32 = 24;
    let rgb: [u8; 3] = match variant {
        TrayVariant::Active => [0x38, 0xBD, 0xF8],   // accent cyan
        TrayVariant::Bypassed => [0xFB, 0xBF, 0x24], // amber
        TrayVariant::Error => [0xF8, 0x71, 0x71],    // red
    };
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for i in 0..W * H {
        if fork_glyph(i % W, i / W) {
            let base = (i * 4) as usize;
            rgba[base] = rgb[0];
            rgba[base + 1] = rgb[1];
            rgba[base + 2] = rgb[2];
            rgba[base + 3] = 0xFF;
        }
    }
    (W, H, rgba)
}

/// Wraps [`fork_pixels`] into a `rune::TrayIcon`.
#[must_use]
pub fn make_icon(variant: TrayVariant) -> TrayIcon {
    let (w, h, rgba) = fork_pixels(variant);
    TrayIcon::rgba(w, h, rgba)
}

/// Creates the tray for the given snapshot.
///
/// `on_event` runs on the tray's own thread: forward to the app loop only.
///
/// # Errors
/// `Tray::new` fails without an interactive taskbar (headless sessions);
/// the shell logs and continues without a tray.
pub fn create(s: &TraySnapshot, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Tray, String> {
    let variant = s.variant();
    Tray::new(
        TraySpec::new("dev.tine.Tine", "Tine", make_icon(variant))
            .tooltip(s.tooltip())
            .menu(build_menu(s)),
        on_event,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{TraySnapshot, TrayVariant, build_menu, menu_id};
    use tine_protocol::presets::Archetype;

    #[test]
    fn menu_marks_the_current_state() {
        let s = TraySnapshot {
            mic_name: "Fifine AM8".to_owned(),
            endpoint_id: Some("ep-1".to_owned()),
            linked: true,
            bypass: false,
            monitor: true,
            archetype: Archetype::Headset,
            autostart: true,
        };
        let menu = build_menu(&s);
        let labels: Vec<&str> = menu
            .iter()
            .filter_map(|item| match item {
                rune::TrayItem::Action { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"✓ Processing"));
        assert!(labels.contains(&"✓ Hear Myself"));
        assert!(labels.contains(&"✓ Gaming Headset / Boom"));
        assert!(labels.contains(&"✓ Start with Windows"));
        assert!(labels.contains(&"Open Tine"));
        assert!(labels.contains(&"Quit"));
        assert!(!labels.contains(&"Bypassed"));
    }

    #[test]
    fn menu_marks_bypassed_when_off_processing() {
        let s = TraySnapshot {
            bypass: true,
            ..TraySnapshot::initial()
        };
        let labels: Vec<String> = build_menu(&s)
            .into_iter()
            .filter_map(|item| match item {
                rune::TrayItem::Action { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Bypassed".to_owned()));
    }

    #[test]
    fn archetype_items_carry_ids() {
        let menu = build_menu(&TraySnapshot::initial());
        let ids: Vec<String> = menu
            .into_iter()
            .filter_map(|item| match item {
                rune::TrayItem::Action { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        for id in [
            format!("{}dynamic", menu_id::ARCH_PREFIX),
            format!("{}condenser", menu_id::ARCH_PREFIX),
            format!("{}headset", menu_id::ARCH_PREFIX),
            format!("{}laptop", menu_id::ARCH_PREFIX),
        ] {
            assert!(ids.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn variant_follows_state() {
        let mut s = TraySnapshot::initial();
        assert_eq!(s.variant(), TrayVariant::Error);
        s.linked = true;
        assert_eq!(s.variant(), TrayVariant::Active);
        s.bypass = true;
        assert_eq!(s.variant(), TrayVariant::Bypassed);
    }

    #[test]
    fn tooltip_shows_mic_and_state() {
        let mut s = TraySnapshot::initial();
        s.linked = true;
        s.bypass = false;
        s.mic_name = "Shure SM7B".to_owned();
        assert_eq!(s.tooltip(), "Tine — Shure SM7B (processing)");
        s.bypass = true;
        assert_eq!(s.tooltip(), "Tine — Shure SM7B (bypassed)");
    }

    #[test]
    fn icons_match_the_variant_palette() {
        for (variant, leading) in [
            (TrayVariant::Active, 0x38u8),
            (TrayVariant::Bypassed, 0xFB),
            (TrayVariant::Error, 0xF8),
        ] {
            let (w, h, rgba) = super::fork_pixels(variant);
            assert_eq!((w, h), (24, 24));
            assert_eq!(rgba.len(), 24 * 24 * 4);
            // Alpha is every 4th byte (offset 3); red is every 4th from 0.
            assert!(rgba.iter().skip(3).step_by(4).all(|&a| a == 0 || a == 0xFF));
            assert!(
                rgba.iter().step_by(4).any(|&r| r == leading),
                "no {leading:#x} pixels for {variant:?}"
            );
            let _ = super::make_icon(variant);
        }
    }
}
