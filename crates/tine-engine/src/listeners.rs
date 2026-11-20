//! Listener gate policy (T24): is any app recording Tine's virtual
//! microphone, and when may the routing loop park?
//!
//! Tine's router runs mic → DSP → `CABLE Input` continuously. When no app
//! has `CABLE Output` open (and nothing inside Tine needs the processed
//! signal), that work is wasted: ~1 % of a core in `tine.exe` plus ~2 % in
//! `audiodg.exe` for the cable streams (T24 benchmark). The app's gate
//! thread polls the cable's capture sessions through
//! [`crate::win::CaptureSessionWatcher`] and applies the pure policy below.
//! Uncertainty always resolves to "keep running": audio must never be
//! missing when an app listens.

use std::time::{Duration, Instant};

use crate::win::CaptureSession;

/// Continuous idle time (no listener, no internal consumer) before parking.
pub const PARK_AFTER: Duration = Duration::from_secs(10);

/// Poll period while parked: bounds how long an app that opens the virtual
/// mic waits for audio (plus the ~40 ms router start).
pub const PARKED_POLL: Duration = Duration::from_millis(200);

/// Poll period while routing: only detects that the last listener left.
pub const RUNNING_POLL: Duration = Duration::from_secs(1);

/// The capture endpoint name paired with a virtual-cable sink: VB-Audio
/// and `VoiceMeeter` name their pairs `… Input (…)` (render) and `… Output (…)`
/// (capture), while Virtual Audio Cable (`Line 1 (Virtual Audio Cable)`) uses
/// the same friendly name for both endpoints.
#[must_use]
pub fn capture_name_for_sink(sink_name: &str) -> String {
    if sink_name.contains("Input") {
        return sink_name.replacen("Input", "Output", 1);
    }
    let lower = sink_name.to_ascii_lowercase();
    if let Some(idx) = lower.find("input")
        && let (Some(prefix), Some(suffix)) = (sink_name.get(..idx), sink_name.get(idx + "input".len()..))
    {
        return format!("{prefix}Output{suffix}");
    }
    sink_name.to_owned()
}

/// Whether capture endpoint `name` is the listening side of `sink_name`.
#[must_use]
pub fn is_sink_capture_side(name: &str, sink_name: &str) -> bool {
    name.eq_ignore_ascii_case(&capture_name_for_sink(sink_name))
}

/// `true` when another process has a running capture session on the cable.
#[must_use]
pub fn foreign_listener(sessions: &[CaptureSession], own_pid: u32) -> bool {
    sessions.iter().any(|s| s.active && s.pid != own_pid)
}

/// Resolves deduplicated display names of other active applications recording
/// directly from the physical microphone (bypassing Tine's virtual cable).
#[must_use]
pub fn raw_mic_app_names(
    sessions: &[CaptureSession],
    own_pid: u32,
    resolve_name: impl Fn(u32) -> Option<String>,
) -> Vec<String> {
    let mut names = Vec::new();
    for s in sessions {
        if !s.active || s.pid == 0 || s.pid == own_pid {
            continue;
        }
        let Some(name) = resolve_name(s.pid) else {
            continue;
        };
        if name.is_empty() || names.iter().any(|n: &String| n == &name) {
            continue;
        }
        names.push(name);
        if names.len() >= 4 {
            break;
        }
    }
    names
}

/// What the gate should do after one poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAction {
    /// Leave the router as it is.
    Stay,
    /// Stop the routing loop (nobody needed it for [`PARK_AFTER`]).
    Park,
    /// Restart the routing loop (something needs it again).
    Resume,
}

/// Hysteresis for parking: park only after continuous idleness, resume at
/// the first sign of need.
#[derive(Debug, Default)]
pub struct ParkPolicy {
    idle_since: Option<Instant>,
}

impl ParkPolicy {
    /// One decision. `needed` = a listener exists, the probe failed, or an
    /// internal consumer (monitor, meters, Auto-Tune) is active.
    pub fn decide(&mut self, now: Instant, parked: bool, needed: bool) -> GateAction {
        if needed {
            self.idle_since = None;
            return if parked { GateAction::Resume } else { GateAction::Stay };
        }
        if parked {
            return GateAction::Stay;
        }
        let since = *self.idle_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= PARK_AFTER {
            self.idle_since = None;
            GateAction::Park
        } else {
            GateAction::Stay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vb_cable_pair_names_map() {
        let sink = "CABLE Input (VB-Audio Virtual Cable)";
        assert_eq!(capture_name_for_sink(sink), "CABLE Output (VB-Audio Virtual Cable)");
        assert!(is_sink_capture_side("cable output (vb-audio virtual cable)", sink));
        assert!(!is_sink_capture_side("Microphone (fifine Microphone)", sink));
        assert!(!is_sink_capture_side("CABLE-A Output (VB-Audio Cable A)", sink));

        let hifi = "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)";
        assert_eq!(capture_name_for_sink(hifi), "Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)");
        assert!(is_sink_capture_side("Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)", hifi));

        let vm = "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)";
        assert_eq!(
            capture_name_for_sink(vm),
            "VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)"
        );
        assert!(is_sink_capture_side(
            "VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)",
            vm
        ));

        let vac = "Line 1 (Virtual Audio Cable)";
        assert_eq!(capture_name_for_sink(vac), "Line 1 (Virtual Audio Cable)");
        assert!(is_sink_capture_side("Line 1 (Virtual Audio Cable)", vac));
    }

    #[test]
    fn only_running_foreign_sessions_count() {
        let own = 42;
        let idle_system = CaptureSession { pid: 0, active: false };
        let ours = CaptureSession { pid: own, active: true };
        let discord_idle = CaptureSession { pid: 7, active: false };
        let discord = CaptureSession { pid: 7, active: true };
        assert!(!foreign_listener(&[idle_system, ours, discord_idle], own));
        assert!(foreign_listener(&[idle_system, discord], own));
    }

    #[test]
    fn parks_only_after_continuous_idle_and_resumes_immediately() {
        let t0 = Instant::now();
        let mut p = ParkPolicy::default();
        assert_eq!(p.decide(t0, false, false), GateAction::Stay);
        assert_eq!(p.decide(t0 + Duration::from_secs(9), false, false), GateAction::Stay);
        // A blip of need resets the idle clock.
        assert_eq!(p.decide(t0 + Duration::from_secs(9), false, true), GateAction::Stay);
        assert_eq!(p.decide(t0 + Duration::from_secs(15), false, false), GateAction::Stay);
        assert_eq!(p.decide(t0 + Duration::from_secs(25), false, false), GateAction::Park);
        // Parked: stays parked while idle, resumes on the first need.
        assert_eq!(p.decide(t0 + Duration::from_secs(60), true, false), GateAction::Stay);
        assert_eq!(p.decide(t0 + Duration::from_secs(61), true, true), GateAction::Resume);
    }

    #[test]
    fn raw_mic_app_names_filters_self_system_inactive_and_dedups() {
        let own = 42;
        let sessions = [
            CaptureSession { pid: 0, active: true },
            CaptureSession { pid: own, active: true },
            CaptureSession { pid: 10, active: false },
            CaptureSession { pid: 11, active: true },
            CaptureSession { pid: 12, active: true },
            CaptureSession { pid: 13, active: true },
        ];
        let names = raw_mic_app_names(&sessions, own, |pid| match pid {
            11 | 12 => Some("Discord".to_owned()),
            13 => Some("OBS Studio".to_owned()),
            _ => None,
        });
        assert_eq!(names, vec!["Discord".to_owned(), "OBS Studio".to_owned()]);
    }
}
