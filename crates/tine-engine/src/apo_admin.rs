//! Virtual cable discovery, endpoint enumeration, health checks, and hot-plug watching.
//!
//! Per D-018, v0.1 routes processed microphone audio from `tine-engine` to a
//! virtual audio cable (`CABLE Input (VB-Audio Virtual Cable)` or equivalent)
//! instead of installing an in-process Windows APO, so all operations here run
//! non-elevated with zero UAC prompts and zero HKLM registry writes.

use std::sync::mpsc::{self, Receiver};

use thiserror::Error;
use tine_protocol::presets::Archetype;

use crate::win::{ComDeviceWatcher, DataFlow, EndpointRecord, WinError, enumerate_endpoints, suggest_archetype};
pub use crate::win::{DeviceEvent, FormFactor};

/// Errors returned by endpoint and virtual-cable administration helpers.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ApoError {
    /// Underlying Win32/COM error.
    #[error(transparent)]
    Win(#[from] WinError),
}

/// Availability and routing state of the virtual audio cable / endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApoState {
    /// Virtual cable sink is available and ready for routing.
    Active {
        /// Age of the most recent heartbeat in milliseconds (0 for in-process routing).
        heartbeat_age_ms: u32,
    },
    /// Virtual cable is present in Windows but currently disabled or unavailable.
    InstalledNotLoaded,
    /// The target capture endpoint is no longer present in Windows.
    RemovedBySystem,
    /// No supported virtual audio cable render sink is installed.
    NotInstalled,
    /// Audio enhancements / routing are disabled on the endpoint.
    EnhancementsDisabled,
    /// An error occurred while querying endpoint or virtual-cable state.
    Error(ApoError),
}

/// A live Windows audio capture (microphone) endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureEndpoint {
    /// Windows `MMDevice` endpoint ID.
    pub id: String,
    /// Human-readable friendly name.
    pub name: String,
    /// Physical form factor reported by Windows.
    pub form_factor: FormFactor,
    /// Suggested Tine microphone [`Archetype`] based on form factor and name.
    pub suggested: Archetype,
    /// Current virtual-cable routing state.
    pub state: ApoState,
}

/// Information about a discovered virtual audio cable render sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualSinkInfo {
    /// Windows `MMDevice` endpoint ID of the virtual cable render sink.
    pub id: String,
    /// Human-readable friendly name (e.g. `CABLE Input (VB-Audio Virtual Cable)`).
    pub name: String,
    /// `true` when the endpoint is in `DEVICE_STATE_ACTIVE` and ready for audio.
    pub is_available: bool,
}

/// Enumerates all active Windows capture endpoints with form-factor [`Archetype`] suggestions.
pub fn list_capture_endpoints() -> Result<Vec<CaptureEndpoint>, ApoError> {
    let sink_state = match find_virtual_cable_sink() {
        Ok(Some(sink)) if sink.is_available => ApoState::Active { heartbeat_age_ms: 0 },
        Ok(Some(_)) => ApoState::InstalledNotLoaded,
        Ok(None) => ApoState::NotInstalled,
        Err(e) => ApoState::Error(e),
    };

    let records = enumerate_endpoints(DataFlow::Capture, true)?;
    Ok(records
        .into_iter()
        .map(|rec| CaptureEndpoint {
            suggested: suggest_archetype(rec.form_factor, &rec.name),
            id: rec.id,
            name: rec.name,
            form_factor: rec.form_factor,
            state: sink_state.clone(),
        })
        .collect())
}

/// Scans Windows render endpoints for a true VB-Audio Virtual Cable sink
/// (`CABLE Input` or `VB-Audio`), never matching `SteelSeries Sonar` or
/// generic `Virtual Audio` playback devices.
///
/// Prefers active (`is_available == true`) endpoints and `CABLE Input` matches.
pub fn find_virtual_cable_sink() -> Result<Option<VirtualSinkInfo>, ApoError> {
    let records = enumerate_endpoints(DataFlow::Render, false)?;
    Ok(select_virtual_cable_sink(&records))
}

/// Returns `true` if `name` identifies a virtual audio cable or virtual mixer
/// capture endpoint (such as `CABLE Output`, `VB-Audio`, or `Sonar - Microphone`)
/// that must not be chosen as Tine's default physical microphone input.
#[must_use]
pub fn is_virtual_cable_capture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable output") || lower.contains("vb-audio") || lower.contains("sonar - microphone")
}

/// Returns the preferred physical capture endpoint ID, preferring the Windows
/// default input device if it is not a virtual cable, otherwise the first
/// physical microphone found.
#[must_use]
pub fn default_physical_capture_endpoint() -> Option<String> {
    let captures = list_capture_endpoints().ok()?;
    let default_input = crate::audio_io::list_inputs()
        .ok()
        .and_then(|inputs| inputs.into_iter().find(|d| d.is_default));

    if let Some(def) = default_input
        && !is_virtual_cable_capture(&def.name)
    {
        if let Some(ep) = captures
            .iter()
            .find(|ep| ep.id == def.id || def.id.ends_with(&ep.id) || ep.name == def.name)
        {
            return Some(ep.id.clone());
        }
        return Some(def.id);
    }

    captures
        .into_iter()
        .find(|ep| !is_virtual_cable_capture(&ep.name))
        .map(|ep| ep.id)
}

fn select_virtual_cable_sink(records: &[EndpointRecord]) -> Option<VirtualSinkInfo> {
    let mut best: Option<(u8, VirtualSinkInfo)> = None;

    for rec in records {
        let Some(priority) = virtual_cable_priority(&rec.name, rec.is_active) else {
            continue;
        };
        let candidate = VirtualSinkInfo {
            id: rec.id.clone(),
            name: rec.name.clone(),
            is_available: rec.is_active,
        };
        if best.as_ref().is_none_or(|(best_prio, _)| priority < *best_prio) {
            best = Some((priority, candidate));
        }
    }

    best.map(|(_, info)| info)
}

/// Lower score = higher preference.
fn virtual_cable_priority(name: &str, is_active: bool) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("steelseries") || lower.contains("sonar") {
        return None;
    }
    let base = if lower.contains("cable input") {
        0
    } else if lower.contains("vb-audio") {
        2
    } else {
        return None;
    };
    Some(if is_active { base } else { base + 10 })
}

/// Reports the current virtual-cable routing health for the given capture `endpoint` ID.
///
/// If `endpoint` is empty, only virtual-cable sink availability is checked.
#[must_use]
pub fn health(endpoint: &str) -> ApoState {
    let sink = match find_virtual_cable_sink() {
        Ok(Some(s)) => s,
        Ok(None) => return ApoState::NotInstalled,
        Err(e) => return ApoState::Error(e),
    };

    if !sink.is_available {
        return ApoState::InstalledNotLoaded;
    }

    if !endpoint.is_empty() {
        match enumerate_endpoints(DataFlow::Capture, true) {
            Ok(captures) => {
                if !captures.iter().any(|c| c.id == endpoint) {
                    return ApoState::RemovedBySystem;
                }
            }
            Err(e) => return ApoState::Error(ApoError::Win(e)),
        }
    }

    ApoState::Active { heartbeat_age_ms: 0 }
}

/// Watches Windows audio endpoints for hot-plug, removal, and default-device changes.
///
/// Wraps `IMMNotificationClient` and delivers [`DeviceEvent`]s over an `mpsc` channel.
pub struct DeviceWatcher {
    _inner: ComDeviceWatcher,
}

impl DeviceWatcher {
    /// Registers an `IMMNotificationClient` and returns the watcher handle plus the
    /// receiving end of the [`DeviceEvent`] channel.
    pub fn new() -> Result<(Self, Receiver<DeviceEvent>), ApoError> {
        let (tx, rx) = mpsc::channel();
        let inner = ComDeviceWatcher::start(tx)?;
        Ok((Self { _inner: inner }, rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, name: &str, is_active: bool) -> EndpointRecord {
        EndpointRecord {
            id: id.to_owned(),
            name: name.to_owned(),
            form_factor: FormFactor::Speakers,
            is_active,
        }
    }

    #[test]
    fn form_factor_suggests_expected_archetypes() {
        assert_eq!(
            suggest_archetype(FormFactor::Headset, "HyperX Cloud II"),
            Archetype::Headset
        );
        assert_eq!(
            suggest_archetype(FormFactor::Headphones, "Generic Headphones"),
            Archetype::Headset
        );
        assert_eq!(
            suggest_archetype(FormFactor::Microphone, "Microphone Array (Realtek)"),
            Archetype::Laptop
        );
        assert_eq!(
            suggest_archetype(FormFactor::Microphone, "Blue Yeti Stereo Microphone"),
            Archetype::Condenser
        );
        assert_eq!(
            suggest_archetype(FormFactor::Microphone, "fifine Microphone"),
            Archetype::Dynamic
        );
        assert_eq!(
            suggest_archetype(FormFactor::LineLevel, "Focusrite Scarlett 2i2"),
            Archetype::Dynamic
        );
    }

    #[test]
    fn select_virtual_cable_prefers_active_cable_input() {
        let records = vec![
            rec("1", "Speakers (Realtek High Definition Audio)", true),
            rec("2", "Virtual Audio Driver (Disabled)", false),
            rec("3", "CABLE Input (VB-Audio Virtual Cable)", true),
            rec("4", "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)", true),
        ];

        let found = select_virtual_cable_sink(&records);
        assert_eq!(
            found,
            Some(VirtualSinkInfo {
                id: "3".to_owned(),
                name: "CABLE Input (VB-Audio Virtual Cable)".to_owned(),
                is_available: true,
            })
        );
    }

    #[test]
    fn select_virtual_cable_returns_none_when_absent() {
        let records = vec![
            rec("1", "Speakers (Realtek High Definition Audio)", true),
            rec("2", "NVIDIA High Definition Audio", true),
        ];
        assert_eq!(select_virtual_cable_sink(&records), None);
    }

    #[test]
    fn select_virtual_cable_ignores_sonar_and_generic_virtual_audio() {
        let records = vec![
            rec(
                "1",
                "SteelSeries Sonar - Gaming (SteelSeries Sonar Virtual Audio Device)",
                true,
            ),
            rec("2", "Generic Virtual Audio Device", true),
            rec("3", "Speakers (Realtek High Definition Audio)", true),
        ];
        assert_eq!(select_virtual_cable_sink(&records), None);
    }

    #[test]
    fn is_virtual_cable_capture_detects_virtual_and_allows_physical() {
        assert!(is_virtual_cable_capture("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(is_virtual_cable_capture("Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)"));
        assert!(is_virtual_cable_capture(
            "SteelSeries Sonar - Microphone (SteelSeries Sonar Virtual Audio Device)"
        ));
        assert!(!is_virtual_cable_capture("Microphone (FIFINE USB Microphone)"));
        assert!(!is_virtual_cable_capture("Headset Microphone (HyperX Cloud II)"));
    }

    #[test]
    fn select_virtual_cable_reports_inactive_when_disabled() {
        let records = vec![rec("2", "CABLE Input (VB-Audio Virtual Cable)", false)];
        let found = select_virtual_cable_sink(&records);
        assert_eq!(
            found,
            Some(VirtualSinkInfo {
                id: "2".to_owned(),
                name: "CABLE Input (VB-Audio Virtual Cable)".to_owned(),
                is_available: false,
            })
        );
    }

    #[test]
    fn live_windows_enumeration_and_health_do_not_error() {
        let captures = list_capture_endpoints().expect("list_capture_endpoints should succeed");
        for ep in &captures {
            assert_ne!(ep.id, "");
            assert_ne!(ep.name, "");
        }

        let sink = find_virtual_cable_sink().expect("find_virtual_cable_sink should succeed");
        let state = health("");
        match sink {
            Some(s) if s.is_available => {
                assert_eq!(state, ApoState::Active { heartbeat_age_ms: 0 });
            }
            Some(_) => assert_eq!(state, ApoState::InstalledNotLoaded),
            None => assert_eq!(state, ApoState::NotInstalled),
        }

        // Missing endpoint ID should report RemovedBySystem when sink is active,
        // or NotInstalled / InstalledNotLoaded when sink is absent/disabled.
        let missing_state = health("{0.0.1.00000000}.{nonexistent-tine-endpoint}");
        if let Some(s) = find_virtual_cable_sink().expect("sink check")
            && s.is_available
        {
            assert_eq!(missing_state, ApoState::RemovedBySystem);
        }
    }

    #[test]
    fn device_watcher_starts_and_drops_cleanly() {
        let (watcher, _rx) = DeviceWatcher::new().expect("DeviceWatcher::new should succeed");
        drop(watcher);
    }
}
