//! T21 pipe protocol end-to-end: the newly unparked control intents,
//! driven through the real named-pipe server into a real controller.
//!
//! The host here mirrors the shell's `execute_pipe` control arm exactly:
//! a `single_instance` pipe server thread, `shell::pipe_control_command`
//! as the wire-to-engine seam, and a controller spawned against an
//! in-process [`ParamBlock`] target (no audio devices involved). The
//! JSON travels over `\\.\pipe\` like it does between the `--ui` child
//! and the tray parent (D-017).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use tine_app::proto::{PipeRequest, PipeResponse, exit_code};
use tine_app::services;
use tine_app::shell::pipe_control_command;
use tine_app::single_instance;
use tine_engine::controller::{Controller, ControllerHandle, EngineConfig, Event};
use tine_engine::model::EndpointId;
use tine_engine::paths::EnginePaths;
use tine_protocol::params::ParamId;
use tine_protocol::sync::ParamBlock;

const MIC_A: &str = "{mic-a}";
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

/// Temporary persistence roots for one test (leaked deliberately: the
/// controller's trailing flush must stay able to write until the test
/// process exits; nextest isolates each test process).
fn temp_paths() -> EnginePaths {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = EnginePaths::from_roots(
        dir.path().join("app"),
        dir.path().join("prog"),
        dir.path().join("local"),
    );
    std::mem::forget(dir);
    paths
}

/// A unique test pipe name (never collides with a real `tine.exe`).
fn pipe_name(tag: &str) -> String {
    format!("\\\\.\\pipe\\tine-test-pipe-{tag}-{}", std::process::id())
}

/// Spawns the pipe host: the real single-instance pipe server plus the
/// handler thread that mirrors `Shell::execute_pipe`'s pure-control arm
/// (`pipe_control_command` → `ControllerHandle::send`) and answers
/// `calibrate_query` from the services flow state.
fn spawn_pipe_host(pipe: &str, handle: ControllerHandle) {
    let (tx_jobs, rx_jobs) = channel::<tine_app::single_instance::PipeJob>();
    single_instance::spawn_server(pipe.to_owned(), tx_jobs);
    let _ = std::thread::Builder::new()
        .name("test-pipe-handler".to_owned())
        .spawn(move || {
            while let Ok(job) = rx_jobs.recv() {
                let resp = match &job.request {
                    PipeRequest::CalibrateQuery => PipeResponse::ok(services::autotune_state()),
                    PipeRequest::CalibratePreview {
                        style,
                        keep_pins,
                        audition_raw,
                    } => {
                        let parsed = tine_dsp::calibrator::VoiceStyle::from_name(style)
                            .unwrap_or(tine_dsp::calibrator::VoiceStyle::AutoBalanced);
                        PipeResponse::ok(services::autotune_preview(parsed, *keep_pins, *audition_raw, None))
                    }
                    req => match pipe_control_command(req) {
                        Ok(cmd) => {
                            let _ = handle.send(cmd);
                            PipeResponse::empty()
                        }
                        Err(msg) => PipeResponse::err(exit_code::USAGE, msg),
                    },
                };
                let _ = job.reply.send(resp);
            }
        });
}

/// Waits for a controller event matching `pred` (bounded, no silent hang).
fn wait_event(events: &Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remain) {
            Ok(ev) if pred(&ev) => return ev,
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("timed out waiting for the event"),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("controller exited"),
        }
    }
}

/// A `set` (the intent that pins, D-007) followed by the newly unparked
/// `unpin` — both over the real pipe — land in the engine event stream:
/// the pin disappears and `input_gain_db` falls back to the archetype
/// line.
#[test]
fn unpin_flows_end_to_end_through_the_real_pipe() {
    let paths = temp_paths();
    let block = Arc::new(ParamBlock::new());
    let (handle, events) = Controller::spawn(EngineConfig::block(EndpointId::new(MIC_A.to_owned()), block, paths));
    let pipe = pipe_name("unpin");
    spawn_pipe_host(&pipe, handle.clone());

    // 1. `set` pins the parameter (the pre-req for an unpin).
    let resp = single_instance::send_request(
        &pipe,
        &PipeRequest::Set {
            key: "input_gain_db".to_owned(),
            value: 6.0,
        },
    )
    .expect("pipe round trip");
    assert!(resp.ok, "set failed: {:?}", resp.error);
    let pinned = wait_event(
        &events,
        |ev| matches!(ev, Event::ModelChanged(m) if m.profile.pins.is_pinned(ParamId::InputGainDb)),
    );
    if let Event::ModelChanged(m) = &pinned {
        assert!(m.profile.pins.is_pinned(ParamId::InputGainDb));
    }

    // 2. `unpin` — a newly unparked intent — releases the pin through
    //    the same pipe.
    let resp = single_instance::send_request(
        &pipe,
        &PipeRequest::Unpin {
            key: "input_gain_db".to_owned(),
        },
    )
    .expect("pipe round trip");
    assert!(resp.ok, "unpin failed: {:?}", resp.error);
    let unpinned = wait_event(
        &events,
        |ev| matches!(ev, Event::ModelChanged(m) if !m.profile.pins.is_pinned(ParamId::InputGainDb)),
    );
    if let Event::ModelChanged(m) = &unpinned {
        assert!(!m.profile.pins.is_pinned(ParamId::InputGainDb));
    }

    // 3. `unpin_all` — the rack-wide twin — is also routable.
    let resp = single_instance::send_request(&pipe, &PipeRequest::UnpinAll).expect("pipe round trip");
    assert!(resp.ok, "unpin_all failed: {:?}", resp.error);
}

/// An unknown SPECS key is a usage error over the wire (never a silent
/// drop): `ok == false` with the CLI's usage exit code.
#[test]
fn unknown_unpin_key_is_a_usage_error_over_the_pipe() {
    let paths = temp_paths();
    let block = Arc::new(ParamBlock::new());
    let (handle, _events) = Controller::spawn(EngineConfig::block(EndpointId::new(MIC_A.to_owned()), block, paths));
    let pipe = pipe_name("usage");
    spawn_pipe_host(&pipe, handle);
    let resp = single_instance::send_request(
        &pipe,
        &PipeRequest::Unpin {
            key: "not_a_real_key".to_owned(),
        },
    )
    .expect("pipe round trip");
    assert!(!resp.ok);
    assert_eq!(resp.code, exit_code::USAGE);
    assert!(resp.error.unwrap_or_default().contains("unknown parameter key"));
}

/// `calibrate_query` over the pipe answers the services flow state (the
/// wizard poll's contract): `idle` with no run in flight.
#[test]
fn calibrate_query_answers_the_flow_state_over_the_pipe() {
    let paths = temp_paths();
    services::shutdown();
    let block = Arc::new(ParamBlock::new());
    let (handle, _events) = Controller::spawn(EngineConfig::block(EndpointId::new(MIC_A.to_owned()), block, paths));
    let pipe = pipe_name("query");
    spawn_pipe_host(&pipe, handle);
    let resp = single_instance::send_request(&pipe, &PipeRequest::CalibrateQuery).expect("pipe round trip");
    assert!(resp.ok, "{:?}", resp.error);
    let data = resp.data.expect("query payload");
    assert_eq!(data.get("flow").and_then(|f| f.as_str()), Some("idle"));

    let preview_resp = single_instance::send_request(
        &pipe,
        &PipeRequest::CalibratePreview {
            style: "broadcast".to_owned(),
            keep_pins: true,
            audition_raw: None,
        },
    )
    .expect("preview pipe round trip");
    assert!(preview_resp.ok, "{:?}", preview_resp.error);
    let preview_data = preview_resp.data.expect("preview payload");
    assert_eq!(preview_data.get("flow").and_then(|f| f.as_str()), Some("idle"));
}
