//! `rune::Tray` wrapper: brand-mark icons per state, flat menu with checked-state-in-label
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
    /// DSP processing (ember capsule).
    Active,
    /// Microphone muted (grey capsule + crimson slash).
    Muted,
    /// Bypassed (grey capsule).
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
    /// Click-free RT mic mute enabled.
    pub muted: bool,
    /// Bypass A/B state.
    pub bypass: bool,
    /// "Hear Myself" monitor enabled.
    pub monitor: bool,
    /// Keep-Warm (PTT) mode: prevents idle parking when no app captures.
    pub keep_warm: bool,
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
            muted: false,
            bypass: false,
            monitor: false,
            keep_warm: false,
            archetype: Archetype::default(),
            autostart: false,
        }
    }

    /// The icon variant for the current state.
    #[must_use]
    pub fn variant(&self) -> TrayVariant {
        match (self.linked, self.muted, self.bypass) {
            (false, _, _) => TrayVariant::Error,
            (true, true, _) => TrayVariant::Muted,
            (true, false, true) => TrayVariant::Bypassed,
            (true, false, false) => TrayVariant::Active,
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
        } else if self.muted {
            "muted"
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
    /// Toggle RT output mute (`Ctrl+Shift+M`).
    pub const MUTE: &str = "mute";
    /// Toggle A/B bypass (`Ctrl+Shift+B`).
    pub const BYPASS: &str = "bypass";
    /// Toggle "Hear Myself".
    pub const MONITOR: &str = "monitor";
    /// Toggle Keep Warm (PTT) idle parking override.
    pub const KEEP_WARM: &str = "keep_warm";
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
    let mute_label = if s.muted {
        "✓ Muted (Ctrl+Shift+M)"
    } else {
        "Mute Mic (Ctrl+Shift+M)"
    };
    let bypass_label = if s.bypass { "Bypassed" } else { "✓ Processing" };
    let monitor_label = if s.monitor { "✓ Hear Myself" } else { "Hear Myself" };
    let keep_warm_label = if s.keep_warm {
        "✓ Keep Warm (PTT)"
    } else {
        "Keep Warm (PTT)"
    };
    let autostart_label = if s.autostart {
        "✓ Start with Windows"
    } else {
        "Start with Windows"
    };
    let mut items = vec![
        TrayItem::action(menu_id::OPEN, "Open Tine"),
        TrayItem::action(menu_id::MUTE, mute_label),
        TrayItem::action(menu_id::BYPASS, bypass_label),
        TrayItem::action(menu_id::MONITOR, monitor_label),
        TrayItem::action(menu_id::KEEP_WARM, keep_warm_label),
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

/// Tray glyph edge length in pixels (pre-rendered by `scripts/brand/render_icons.py`).
const GLYPH_PX: u32 = 32;

/// The brand mark per tray state, as raw RGBA (no decoder needed in the parent).
static GLYPH_ACTIVE: &[u8] = include_bytes!("../../../assets/brand/tray-active-32.rgba");
static GLYPH_BYPASSED: &[u8] = include_bytes!("../../../assets/brand/tray-bypassed-32.rgba");
static GLYPH_ERROR: &[u8] = include_bytes!("../../../assets/brand/tray-error-32.rgba");

/// The tray glyph for `variant`: the Tine mark (fork + mic capsule) whose capsule
/// shows the state: ember while processing, grey when bypassed, red (dimmed fork)
/// when the link is lost.
#[must_use]
pub fn fork_pixels(variant: TrayVariant) -> (u32, u32, Vec<u8>) {
    if variant == TrayVariant::Muted {
        let mut rgba = GLYPH_BYPASSED.to_vec();
        for y in 6..26_usize {
            for x in 6..26_usize {
                if x.abs_diff(y) <= 1 {
                    let idx = (y * GLYPH_PX as usize + x) * 4;
                    if let Some(px) = rgba.get_mut(idx..idx + 4) {
                        px.copy_from_slice(&[235, 72, 72, 255]);
                    }
                }
            }
        }
        return (GLYPH_PX, GLYPH_PX, rgba);
    }
    let rgba = match variant {
        TrayVariant::Active => GLYPH_ACTIVE,
        TrayVariant::Bypassed => GLYPH_BYPASSED,
        TrayVariant::Error | TrayVariant::Muted => GLYPH_ERROR,
    };
    (GLYPH_PX, GLYPH_PX, rgba.to_vec())
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
            muted: false,
            bypass: false,
            monitor: true,
            keep_warm: true,
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
        assert!(labels.contains(&"✓ Keep Warm (PTT)"));
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
        s.muted = true;
        assert_eq!(s.variant(), TrayVariant::Muted);
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
        s.muted = true;
        assert_eq!(s.tooltip(), "Tine — Shure SM7B (muted)");
    }

    #[test]
    fn icons_are_distinct_per_variant() {
        let mut seen = Vec::new();
        for variant in [
            TrayVariant::Active,
            TrayVariant::Muted,
            TrayVariant::Bypassed,
            TrayVariant::Error,
        ] {
            let (w, h, rgba) = super::fork_pixels(variant);
            assert_eq!((w, h), (32, 32));
            assert_eq!(rgba.len(), 32 * 32 * 4);
            // Rounded plate: transparent corners, opaque centre.
            assert_eq!(rgba[3], 0, "corner must be transparent");
            let centre = ((16 * 32 + 16) * 4 + 3) as usize;
            assert_eq!(rgba[centre], 0xFF, "plate must be opaque");
            assert!(!seen.contains(&rgba), "{variant:?} glyph duplicates another state");
            seen.push(rgba);
            let _ = super::make_icon(variant);
        }
    }
}
