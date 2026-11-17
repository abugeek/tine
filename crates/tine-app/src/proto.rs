//! JSON pipe protocol shared by the CLI, the pipe server, and the UI child.
//!
//! Every request is one newline-delimited JSON object; every response is one
//! newline-delimited JSON object. `PipeRequest` mirrors the engine
//! [`tine_engine::controller::Command`] plus the shell commands (`open`,
//! `status`, `meters`, `calibrate`, `quit`, `soak`).

use serde::{Deserialize, Serialize};

/// A command sent over `\\.\pipe\tine-<user-sid>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum PipeRequest {
    /// Show the UI window (spawn the `--ui` child if needed).
    Open,
    /// Full runtime snapshot for display.
    Status,
    /// Live telemetry + spectrum.
    Meters,
    /// Runs the 8-second Auto-Tune calibration against the live audio path
    /// (D-018 lease shim when no APO mapping exists).
    Calibrate,
    /// Flush persistence and shut the app down.
    Quit,
    /// Set one Pro-mode parameter by SPECS key (pins it, D-007).
    Set {
        /// Stable machine key from `tine_protocol::params::SPECS`.
        key: String,
        /// Raw value; clamped by the engine.
        value: f32,
    },
    /// Move a Golden Line macro (`purity`/`body`/`presence`/`leveler`).
    Macro {
        /// Macro machine name.
        name: String,
        /// Position `0.0..=1.0`.
        value: f32,
    },
    /// Switch the hardware archetype.
    Archetype {
        /// Archetype id (`dynamic`/`condenser`/`headset`/`laptop`).
        name: String,
    },
    /// Global A/B bypass.
    Bypass {
        /// `true` = raw passthrough.
        on: bool,
    },
    /// "Hear Myself" monitor toggle.
    Monitor {
        /// Enabled state.
        on: bool,
        /// Optional playback device id to switch to.
        device: Option<String>,
    },
    /// Soak the running instance and write a report.
    Soak {
        /// Duration in minutes.
        minutes: u64,
        /// Report file path (written by the app).
        report: String,
    },
    /// Switch the capture endpoint the engine processes (the D-018
    /// routing loop follows the controller's model automatically).
    Endpoint {
        /// WASAPI endpoint id of the capture device.
        id: String,
    },
    /// Unpin one Pro-mode parameter (back to the archetype line, D-007).
    Unpin {
        /// Stable machine key from `tine_protocol::params::SPECS`.
        key: String,
    },
    /// Unpin every parameter (the whole rack back to the archetype line).
    UnpinAll,
    /// Auto-Tune wizard progress query (the UI child's ~10 Hz poll).
    CalibrateQuery,
    /// Cancel the Auto-Tune run in flight.
    CalibrateCancel,
    /// Apply the pending Auto-Tune results.
    CalibrateApply {
        /// Keep the user's pinned parameters (`true`, D-007).
        keep_pins: bool,
    },
    /// Discard the pending Auto-Tune results.
    CalibrateDiscard,
    /// Persist whether the Surgical Pro Rack drawer is expanded.
    SetProDrawerOpen {
        /// `true` when the Pro drawer is open.
        open: bool,
    },
    /// Persist that first-run onboarding was completed or dismissed.
    OnboardingDone,
    /// Re-run the D-018 virtual-cable routing loop against the active endpoint.
    RouterFollow,
}

/// Server reply. `code` doubles as the CLI exit code: 0 ok, 1 failure,
/// 2 usage, 3 stale/unknown request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipeResponse {
    /// Whether the command succeeded.
    pub ok: bool,
    /// `0` ok · `1` engine error · `2` usage · `3` not applicable.
    pub code: i32,
    /// Payload (command-specific JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Human-readable error text when `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PipeResponse {
    /// Successful response with payload.
    #[must_use]
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            code: 0,
            data: Some(data),
            error: None,
        }
    }

    /// Successful response without payload.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            ok: true,
            code: 0,
            data: None,
            error: None,
        }
    }

    /// Failure response with an exit code and message.
    #[must_use]
    pub fn err(code: i32, error: impl Into<String>) -> Self {
        Self {
            ok: false,
            code,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// Exit codes shared by the CLI and the pipe protocol.
pub mod exit_code {
    /// Success.
    pub const OK: i32 = 0;
    /// Engine/runtime failure.
    pub const FAILURE: i32 = 1;
    /// Usage error (bad arguments).
    pub const USAGE: i32 = 2;
    /// Request against a state that does not exist (e.g. no instance).
    pub const NOT_RUNNING: i32 = 3;
}

/// `CreateNamedPipeW` "unlimited instances" sentinel.
pub const PIPE_UNLIMITED_INSTANCES: u32 = 255;

/// Serializes one request into a newline-terminated JSON line.
#[must_use]
pub fn encode_line(req: &PipeRequest) -> Vec<u8> {
    let mut json = serde_json::to_vec(req).unwrap_or_else(|_| b"{\"cmd\":\"quit\"}".to_vec());
    json.push(b'\n');
    json
}

/// Splits a receive buffer into complete newline-terminated lines.
#[must_use]
pub fn take_line(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline = buf.iter().position(|&b| b == b'\n')?;
    let line: Vec<u8> = buf.drain(..=newline).collect();
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::{PipeRequest, PipeResponse, exit_code};

    #[test]
    fn requests_round_trip_through_json() {
        let cases = vec![
            PipeRequest::Open,
            PipeRequest::Status,
            PipeRequest::Meters,
            PipeRequest::Calibrate,
            PipeRequest::Quit,
            PipeRequest::Set {
                key: "input_gain_db".to_owned(),
                value: 3.5,
            },
            PipeRequest::Macro {
                name: "body".to_owned(),
                value: 0.7,
            },
            PipeRequest::Archetype {
                name: "headset".to_owned(),
            },
            PipeRequest::Bypass { on: true },
            PipeRequest::Monitor {
                on: true,
                device: Some("out-1".to_owned()),
            },
            PipeRequest::Soak {
                minutes: 1,
                report: "target/soak.json".to_owned(),
            },
            PipeRequest::Endpoint {
                id: "{mic-1}".to_owned(),
            },
            PipeRequest::Unpin {
                key: "input_gain_db".to_owned(),
            },
            PipeRequest::UnpinAll,
            PipeRequest::CalibrateQuery,
            PipeRequest::CalibrateCancel,
            PipeRequest::CalibrateApply { keep_pins: true },
            PipeRequest::CalibrateDiscard,
            PipeRequest::SetProDrawerOpen { open: true },
            PipeRequest::OnboardingDone,
            PipeRequest::RouterFollow,
        ];
        for req in &cases {
            let json = serde_json::to_string(req).unwrap_or_default();
            assert!(json.contains("\"cmd\":"), "{json}");
            let back: PipeRequest = serde_json::from_str(&json).unwrap_or(PipeRequest::Quit);
            assert_eq!(&back, req, "round trip {json}");
        }
    }

    #[test]
    fn snake_case_command_names() {
        let req = PipeRequest::Bypass { on: false };
        let json = serde_json::to_string(&req).unwrap_or_default();
        assert!(json.contains("\"cmd\":\"bypass\""), "{json}");
        assert!(json.contains("\"on\":false"), "{json}");
    }

    #[test]
    fn responses_round_trip() {
        let resp = PipeResponse::err(exit_code::USAGE, "bad key");
        let json = serde_json::to_string(&resp).unwrap_or_default();
        let back: PipeResponse = serde_json::from_str(&json).unwrap_or(PipeResponse::empty());
        assert_eq!(back, resp);
        assert!(
            serde_json::to_string(&PipeResponse::empty())
                .unwrap_or_default()
                .contains("\"code\":0")
        );
    }

    #[test]
    fn take_line_splits_waiting_data() {
        let mut buf = b"{\"cmd\":\"open\"}\n{\"cmd\":\"sta".to_vec();
        let line = super::take_line(&mut buf).unwrap_or_default();
        assert_eq!(line, b"{\"cmd\":\"open\"}\n");
        assert!(super::take_line(&mut buf).is_none());
        assert_eq!(buf, b"{\"cmd\":\"sta");
    }

    #[test]
    fn encode_terminates_with_newline() {
        let bytes = super::encode_line(&PipeRequest::Open);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let parsed: PipeRequest = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap_or(PipeRequest::Quit);
        assert_eq!(parsed, PipeRequest::Open);
    }
}
