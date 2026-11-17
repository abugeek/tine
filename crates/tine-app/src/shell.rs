//! The app's parent process (D-017): tray + control plane + pipe server +
//! `--ui` child spawner — and *no* Rune `App`/`wgpu`/`winit` ever
//! initialized here (the window lives in the short-lived child).
//!
//! Shutdown per spec: Quit flushes persistence (dropping every
//! `ControllerHandle` stops the controller thread, which flushes on
//! channel close), stops services, removes the tray, kills the UI child,
//! and exits.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use rune::{Tray, TrayEvent};
use serde_json::{Value, json};
use tine_engine::controller::{Command, Controller, EngineConfig, Event};
use tine_engine::model::{EndpointId, KeepPins, Macro, MonitorSettings};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;
use tine_engine::shm_client::LinkState;

use crate::proto::{PipeRequest, PipeResponse, exit_code};
use crate::single_instance::PipeJob;
use crate::tray::{TraySnapshot, menu_id};
use crate::{autostart, services, tray};

/// Message consumed by the app loop (everything is serialized through it).
enum Msg {
    /// Tray activate/menu pick (forwarded from the tray thread).
    Tray(TrayEvent),
    /// A pipe request with its reply channel.
    Pipe(PipeJob),
    /// Engine event (forwarded from a forwarder thread).
    Engine(Event),
}

/// Everything the app loop needs; lives on the loop thread.
struct Shell {
    handle: tine_engine::controller::ControllerHandle,
    model: Option<Arc<tine_engine::model::AppModel>>,
    link: LinkState,
    endpoint_names: Option<Vec<(String, String)>>,
    autostart: bool,
    tray: Option<Arc<Tray>>,
    tray_variant: Option<tray::TrayVariant>,
    ui_child: Option<std::process::Child>,
    soaks: Vec<std::thread::JoinHandle<()>>,
    entered: Instant,
}

impl Shell {
    fn snapshot(&self) -> TraySnapshot {
        let mut s = TraySnapshot::initial();
        s.linked = self.link.is_linked();
        s.autostart = self.autostart;
        if let Some(model) = &self.model {
            s.bypass = model.bypass;
            s.monitor = model.monitor.enabled;
            s.archetype = model.profile.archetype;
        }
        if let Some(model) = &self.model
            && let Some(endpoint) = &model.endpoint
        {
            let raw = endpoint.as_str();
            s.endpoint_id = Some(raw.to_owned());
            s.mic_name = self.mic_name(raw);
        }
        s
    }

    /// Friendly endpoint name from the cached enumeration (id as fallback).
    fn mic_name(&self, endpoint_id: &str) -> String {
        self.endpoint_names
            .as_ref()
            .and_then(|list| {
                list.iter()
                    .find(|(id, _)| id == endpoint_id)
                    .map(|(_, name)| name.clone())
            })
            .unwrap_or_else(|| endpoint_id.to_owned())
    }

    /// Lazy endpoint enumeration: not needed for the tray's cold start, only
    /// for the friendly mic name (and `status` output).
    fn ensure_endpoint_names(&mut self) {
        if self.endpoint_names.is_none() {
            self.endpoint_names = Some(
                tine_engine::apo_admin::list_capture_endpoints()
                    .map(|list| list.into_iter().map(|ep| (ep.id, ep.name)).collect::<Vec<_>>())
                    .unwrap_or_default(),
            );
        }
    }

    /// Creates or recreates the tray for the snapshot's icon variant.
    fn sync_tray(&mut self, tx: &Sender<Msg>) {
        let snap = self.snapshot();
        let want = Some(snap.variant());
        if self.tray.is_some() && self.tray_variant == want {
            if let Some(t) = &self.tray {
                t.set_menu(tray::build_menu(&snap));
                t.set_tooltip(snap.tooltip());
            }
            return;
        }
        // `rune::Tray` has no `set_icon`: recreate when the variant changed
        // (create the new one first, then drop the old icon).
        let tx = tx.clone();
        match tray::create(&snap, move |ev| {
            let _ = tx.send(Msg::Tray(ev));
        }) {
            Ok(new_tray) => {
                self.tray = Some(Arc::new(new_tray));
                self.tray_variant = want;
                tracing::info!(target: "tine::tray", variant = ?want, "tray (re)created");
            }
            Err(e) => {
                tracing::warn!(target: "tine::tray", "Tray::new failed ({e}); continuing without tray");
            }
        }
    }
}

/// Runs the app. Returns the process exit code.
pub fn run_app(background: bool, metrics: bool) -> i32 {
    let entered = Instant::now();

    // 1. Single instance: forward `open` (or exit quietly for --background).
    let (guard, primary) = if background {
        crate::single_instance::acquire_quiet()
    } else {
        crate::single_instance::acquire_or_forward_open()
    };
    if !primary {
        return exit_code::OK;
    }
    let Some(guard) = guard else {
        return exit_code::OK;
    };

    // 2. Logging (lazy: nothing heavier before this).
    let paths = match EnginePaths::system() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tine: cannot resolve storage paths: {e}");
            return exit_code::FAILURE;
        }
    };
    if let Err(e) = crate::logging::init(&paths.logs_dir()) {
        eprintln!("tine: file logging disabled ({e})");
    }
    tracing::info!(target: "tine::shell", version = crate::version::tine_version(), "starting");

    // 3. Controller (thread; blocks on its own channel, no polling while
    //    linked). Endpoint: persisted selection, else the default capture
    //    device, else a placeholder id — no COM on the cold-start path.
    //    D-018: the target is the in-process parameter block the
    //    virtual-cable routing loop reads (the router is started below).
    let settings = persist::load_settings(&paths).into_value();
    let endpoint = settings
        .selected_endpoint_id
        .clone()
        .map(EndpointId::new)
        .or_else(default_capture_endpoint)
        .unwrap_or_else(|| EndpointId::new("default"));
    tracing::info!(target: "tine::shell", endpoint = endpoint.as_str(), "controller starting");
    let (handle, rx_events) = Controller::spawn(EngineConfig::block(
        endpoint.clone(),
        services::param_block(),
        paths.clone(),
    ));

    // 4. App loop plumbing.
    let (tx, rx) = channel::<Msg>();
    let tx_events = tx.clone();
    let forwarder = std::thread::Builder::new()
        .name("tine-event-forward".to_owned())
        .spawn(move || {
            while let Ok(event) = rx_events.recv() {
                if tx_events.send(Msg::Engine(event)).is_err() {
                    break;
                }
            }
        });

    let mut shell = Shell {
        handle: handle.clone(),
        model: None,
        link: LinkState::Unlinked,
        endpoint_names: None,
        autostart: autostart::is_enabled().unwrap_or(false),
        tray: None,
        tray_variant: None,
        ui_child: None,
        soaks: Vec::new(),
        entered,
    };

    // 5. Tray (cold-start finish line).
    shell.sync_tray(&tx);
    let cold_ms = shell.entered.elapsed().as_secs_f64() * 1000.0;
    tracing::info!(target: "tine::shell", cold_start_tray_ms = cold_ms, "cold start to tray complete");
    if metrics {
        eprintln!("metric:cold_start_tray_ms={cold_ms:.2}");
    }

    // 6. Pipe server (own job channel, forwarded into the app loop).
    let (tx_jobs, rx_jobs) = channel::<PipeJob>();
    {
        let tx_forward = tx.clone();
        std::thread::Builder::new()
            .name("tine-pipe-forward".to_owned())
            .spawn(move || {
                while let Ok(job) = rx_jobs.recv() {
                    if tx_forward.send(Msg::Pipe(job)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }
    crate::single_instance::spawn_server(guard.pipe_name.clone(), tx_jobs);

    // 6b. D-018 routing: the virtual-cable loop runs in this process
    //     (audio keeps flowing while the window is closed); the router
    //     follows the controller's endpoint on every `ModelChanged`.
    //     The loop's health is this topology's "linked" notion — with
    //     the block target the engine never emits `ApoLink`.
    let routed = services::router_follow(endpoint.as_str());
    shell.link = link_from_router(routed);
    shell.sync_tray(&tx);

    // 7. Event loop.
    let mut code = exit_code::OK;
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Engine(Event::ModelChanged(model)) => {
                shell.ensure_endpoint_names();
                shell.model = Some(model);
                if let Some(endpoint) = shell.model.as_ref().and_then(|m| m.endpoint.as_ref()) {
                    let routed = services::router_follow(endpoint.as_str());
                    shell.link = link_from_router(routed);
                }
                shell.sync_tray(&tx);
            }
            Msg::Engine(Event::ApoLink(link)) => {
                tracing::info!(target: "tine::shell", link = ?link, "link state changed");
                shell.link = link;
                shell.sync_tray(&tx);
            }
            Msg::Engine(Event::Error(e)) => {
                tracing::error!(target: "tine::shell", error = ?e, "engine error");
                // Persist failures (e.g. a full disk rejects the settings
                // save) must reach the user, not only the log (T22 QA).
                services::surface_message(format!("Tine couldn't apply or save a change: {e}"));
            }
            Msg::Tray(TrayEvent::Activate { .. }) => shell.open_ui(),
            Msg::Tray(TrayEvent::Menu(id)) if id == menu_id::OPEN => shell.open_ui(),
            Msg::Tray(TrayEvent::Menu(id)) if id == menu_id::QUIT => {
                tracing::info!(target: "tine::shell", "quit requested from tray");
                code = exit_code::OK;
                break;
            }
            Msg::Tray(TrayEvent::Menu(id)) => shell.on_tray_menu(&id),
            Msg::Pipe(job) => {
                if matches!(job.request, PipeRequest::Quit) {
                    tracing::info!(target: "tine::shell", "quit requested via pipe");
                    let _ = job.reply.send(PipeResponse::empty());
                    code = exit_code::OK;
                    break;
                }
                shell.on_pipe(job);
            }
            // `ParamsResolved` and unknown future tray events need no work:
            // resolved params already reached the shared-memory target.
            Msg::Engine(Event::ParamsResolved(_)) | Msg::Tray(_) => {}
        }
    }

    // 8. Clean shutdown: stop services (monitor, Auto-Tune shim, routing
    //    loop — releasing the mic and the cable sink), flush persistence
    //    (dropping every `ControllerHandle` ends the controller thread,
    //    which flushes on channel close), remove the tray, kill the UI
    //    child, exit.
    tracing::info!(target: "tine::shell", "shutting down");
    services::shutdown();
    // Interrupt any running soak so its thread joins within a sample period.
    crate::console::INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
    for join in shell.soaks.drain(..) {
        let _ = join.join();
    }
    if let Some(mut child) = shell.ui_child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(shell); // drops its controller handle clone
    drop(handle); // drops the last handle -> controller thread flushes and exits
    drop(forwarder);
    drop(guard); // releases `Local\Tine.SingleInstance` and closes the pipe name
    code
}

/// Best-effort default capture endpoint (first active mic). COM; used only
/// when nothing is persisted yet.
fn default_capture_endpoint() -> Option<EndpointId> {
    tine_engine::apo_admin::list_capture_endpoints()
        .ok()
        .and_then(|list| list.first().map(|ep| EndpointId::new(ep.id.clone())))
}

impl Shell {
    /// Opens the UI: spawn the `--ui` child if none is alive.
    fn open_ui(&mut self) {
        let alive = self
            .ui_child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none());
        if alive {
            tracing::debug!(target: "tine::shell", "UI child already running");
            return;
        }
        match std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--ui")
                // The window child is a GUI app: it must not inherit the
                // parent's stdio. The parent may carry exotic pipe handles
                // (service-like launch contexts), which make `spawn` fail
                // with `ERROR_NOT_SUPPORTED`; the child logs to the shared
                // file sink, so null stdio is both safe and more robust.
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())
        }) {
            Ok(child) => {
                self.ui_child = Some(child);
                tracing::info!(target: "tine::shell", "UI child spawned");
            }
            Err(e) => tracing::error!(target: "tine::shell", "spawning UI child failed: {e}"),
        }
    }

    fn on_tray_menu(&mut self, id: &str) {
        if let Some(arch) = id.strip_prefix(menu_id::ARCH_PREFIX) {
            if let Some(arch) = crate::cli::archetype_from_name(arch) {
                let _ = self.handle.send(Command::SetArchetype(arch));
            }
            return;
        }
        match id {
            menu_id::BYPASS => {
                let next = self.model.as_ref().is_none_or(|m| !m.bypass);
                let _ = self.handle.send(Command::SetBypass(next));
            }
            menu_id::MONITOR => {
                let current = self.model.as_ref().is_some_and(|m| m.monitor.enabled);
                let next = !current;
                self.apply_monitor(next, None);
            }
            menu_id::AUTOSTART => {
                let next = !self.autostart;
                match autostart::set_enabled(next) {
                    Ok(()) => {
                        self.autostart = next;
                        tracing::info!(target: "tine::shell", autostart = next, "autostart toggled");
                    }
                    Err(e) => tracing::error!(target: "tine::shell", "autostart toggle failed: {e}"),
                }
            }
            _ => {}
        }
    }

    fn apply_monitor(&mut self, enabled: bool, device: Option<String>) {
        let mut settings = MonitorSettings {
            enabled,
            output_device: device,
        };
        if settings.output_device.is_none() {
            settings.output_device = self.model.as_ref().and_then(|m| m.monitor.output_device.clone());
        }
        services::monitor_apply(settings.enabled, settings.output_device.as_deref());
        let _ = self.handle.send(Command::SetMonitor(settings));
    }

    fn on_pipe(&mut self, job: PipeJob) {
        let response = self.execute_pipe(job.request);
        let _ = job.reply.send(response);
    }

    fn execute_pipe(&mut self, req: PipeRequest) -> PipeResponse {
        match req {
            PipeRequest::Open => {
                self.open_ui();
                PipeResponse::empty()
            }
            PipeRequest::Status => {
                self.ensure_endpoint_names();
                PipeResponse::ok(self.status_value())
            }
            PipeRequest::Meters => {
                // D-018: with the router running, the pull order matters
                // (single `UiFrame` reader): spectrum first caches the
                // frame; the telemetry read then comes from that cache.
                let spectrum = services::spectrum();
                let telemetry = services::telemetry_live().unwrap_or_else(|| self.handle.telemetry());
                PipeResponse::ok(json!({
                    "telemetry": {
                        "sequence": telemetry.sequence,
                        "input_rms_db": telemetry.input_rms_db,
                        "input_peak_db": telemetry.input_peak_db,
                        "output_rms_db": telemetry.output_rms_db,
                        "output_peak_db": telemetry.output_peak_db,
                        "gate_attenuation_db": telemetry.gate_attenuation_db,
                        "comp_gain_reduction_db": telemetry.comp_gain_reduction_db,
                        "deesser_reduction_db": telemetry.deesser_reduction_db,
                    },
                    "spectrum": spectrum,
                }))
            }
            PipeRequest::Calibrate => {
                services::autotune_start();
                // The reply carries the flow state: `running` right away,
                // or the surfaced reason when the run could not start.
                let state = services::autotune_state();
                if state.get("flow").and_then(Value::as_str) != Some("running") {
                    for note in services::take_messages() {
                        tracing::warn!(target: "tine::shell", "{note}");
                    }
                }
                PipeResponse::ok(json!({ "calibrate": state }))
            }
            PipeRequest::Quit => PipeResponse::empty(), // handled by the loop
            PipeRequest::Set { .. }
            | PipeRequest::Macro { .. }
            | PipeRequest::Archetype { .. }
            | PipeRequest::Bypass { .. }
            | PipeRequest::Endpoint { .. }
            | PipeRequest::Unpin { .. }
            | PipeRequest::UnpinAll => match pipe_control_command(&req) {
                Ok(cmd) => {
                    let _ = self.handle.send(cmd);
                    let ack = match &req {
                        PipeRequest::Set { key, value } => json!({ "set": key, "value": value }),
                        PipeRequest::Macro { name, value } => json!({ "macro": name, "value": value }),
                        PipeRequest::Archetype { name } => match crate::cli::archetype_from_name(name) {
                            Some(a) => json!({ "archetype": a.id() }),
                            None => json!({ "archetype": name }),
                        },
                        PipeRequest::Bypass { on } => json!({ "bypass": on }),
                        PipeRequest::Endpoint { id } => json!({ "endpoint": id }),
                        PipeRequest::Unpin { key } => json!({ "unpin": key }),
                        PipeRequest::UnpinAll => json!({ "unpin": "all" }),
                        _ => Value::Null,
                    };
                    PipeResponse::ok(ack)
                }
                Err(msg) => PipeResponse::err(exit_code::USAGE, msg),
            },
            PipeRequest::CalibrateQuery => PipeResponse::ok(services::autotune_state()),
            PipeRequest::CalibrateCancel => {
                services::autotune_cancel();
                PipeResponse::ok(json!({ "calibrate": "cancelling" }))
            }
            PipeRequest::CalibrateApply { keep_pins } => match services::autotune_take_pending() {
                Some(calibration) => {
                    let keep = if keep_pins { KeepPins::Keep } else { KeepPins::Clear };
                    if let Err(e) = self.handle.send(Command::ApplyCalibration(calibration, keep)) {
                        tracing::error!(target: "tine::shell", error = %e, "calibration apply failed");
                        return PipeResponse::err(exit_code::FAILURE, format!("Tine engine unreachable: {e}"));
                    }
                    tracing::info!(target: "tine::shell", keep_pins, "calibration applied");
                    PipeResponse::ok(json!({ "calibrate": "applied" }))
                }
                None => PipeResponse::err(
                    exit_code::NOT_RUNNING,
                    "no Auto-Tune results to apply (run a calibration first)",
                ),
            },
            PipeRequest::CalibrateDiscard => {
                services::autotune_discard();
                PipeResponse::ok(json!({ "calibrate": "discarded" }))
            }
            PipeRequest::Monitor { on, device } => {
                self.apply_monitor(on, device);
                PipeResponse::ok(json!({ "monitor": on }))
            }
            PipeRequest::Soak { minutes, report } => {
                let path_report = report.clone();
                tracing::info!(target: "tine::shell", minutes = minutes, report = %path_report, "soak starting");
                let handle = self.handle.clone();
                let join = std::thread::Builder::new()
                    .name("tine-soak".to_owned())
                    .spawn(move || {
                        let seq = move || handle.telemetry().sequence;
                        let notes = json!({
                            "where": "running instance",
                            "requested": "tine soak",
                        });
                        let report_doc =
                            crate::soak::run_blocking(minutes, std::path::Path::new(&path_report), &notes, &seq);
                        tracing::info!(
                            target: "tine::shell",
                            summary = ?report_doc.get("summary"),
                            "soak finished"
                        );
                    })
                    .ok();
                self.soaks.extend(join);
                PipeResponse::ok(json!({ "soak": "started", "minutes": minutes, "report": report }))
            }
        }
    }

    fn status_value(&self) -> Value {
        let model = self.model.as_ref();
        let snapshot = self.snapshot();
        let telemetry = self.handle.telemetry();
        json!({
            "version": crate::version::tine_version(),
            "uptime_s": self.entered.elapsed().as_secs_f64(),
            "endpoint_id": snapshot.endpoint_id.clone().unwrap_or_default(),
            "mic_name": snapshot.mic_name,
            "archetype": model.map_or("unknown", |m| m.profile.archetype.id()),
            "macros": model.map_or(Value::Null, |m| {
                json!({
                    "purity": Macro::Purity.get(&m.profile.macros),
                    "body": Macro::Body.get(&m.profile.macros),
                    "presence": Macro::Presence.get(&m.profile.macros),
                    "leveler": Macro::Leveler.get(&m.profile.macros),
                })
            }),
            "bypass": model.is_some_and(|m| m.bypass),
            "state": if self.link.is_linked() {
                if model.is_some_and(|m| m.bypass) { "bypassed" } else { "processing" }
            } else {
                "inactive"
            },
            "link": link_string(&self.link),
            "monitor": model.is_some_and(|m| m.monitor.enabled),
            "monitor_device": model.and_then(|m| m.monitor.output_device.clone()),
            "router": services::router_status(),
            "autotune": services::autotune_state(),
            "notes": services::take_messages(),
            "autostart": self.autostart,
            "autostart_command": if self.autostart {
                crate::autostart::command_line_for_current_exe()
            } else {
                String::new()
            },
            "telemetry": {
                "sequence": telemetry.sequence,
                "input_peak_db": telemetry.input_peak_db,
                "output_peak_db": telemetry.output_peak_db,
            },
            "windows": crate::version::windows_edition(),
        })
    }
}

/// Wire-to-engine mapping of every *pure control* pipe request: no shell
/// state is consulted, so this is the single place the pipe protocol's
/// control surface resolves to [`Command`]s — and the seam the pipe
/// integration tests drive against a real controller.
///
/// `Err` carries the human-facing usage message (unknown key/macro/…).
/// Requests that need shell or services state (`open`, `status`,
/// `meters`, `calibrate*`, `monitor`, `soak`, `quit`) are not control
/// commands; they are mapped in [`Shell::execute_pipe`] and land here
/// only as an `Err` the shell never produces.
pub fn pipe_control_command(req: &PipeRequest) -> Result<Command, String> {
    match req {
        PipeRequest::Set { key, value } => crate::cli::param_id_by_key(key)
            .map(|id| Command::SetParam(id, *value))
            .ok_or_else(|| format!("unknown parameter key `{key}` (see docs/project/03-dsp-pipeline.md)")),
        PipeRequest::Macro { name, value } => Macro::from_name(name)
            .map(|m| Command::SetMacro(m, *value))
            .ok_or_else(|| format!("unknown macro `{name}` (purity|body|presence|leveler)")),
        PipeRequest::Archetype { name } => crate::cli::archetype_from_name(name)
            .map(Command::SetArchetype)
            .ok_or_else(|| format!("unknown archetype `{name}` (dynamic|condenser|headset|laptop)")),
        PipeRequest::Bypass { on } => Ok(Command::SetBypass(*on)),
        PipeRequest::Endpoint { id } => Ok(Command::SelectEndpoint(EndpointId::new(id.clone()))),
        PipeRequest::Unpin { key } => crate::cli::param_id_by_key(key)
            .map(Command::Unpin)
            .ok_or_else(|| format!("unknown parameter key `{key}` (see docs/project/03-dsp-pipeline.md)")),
        PipeRequest::UnpinAll => Ok(Command::UnpinAll),
        other => Err(format!("{} is not a control request", json_cmd(other))),
    }
}

/// The request's tag (`cmd`) for messages (control arms never land here).
fn json_cmd(req: &PipeRequest) -> String {
    serde_json::to_value(req)
        .ok()
        .and_then(|v| v.get("cmd").cloned())
        .map_or_else(|| "unknown".to_owned(), |c| c.to_string())
}

/// The D-018 "linked" notion: the routing loop's health (the block
/// target never emits an `ApoLink` event).
fn link_from_router(routed: bool) -> LinkState {
    if routed {
        LinkState::Linked { heartbeat_age_ms: 0 }
    } else {
        LinkState::Unlinked
    }
}

fn link_string(link: &LinkState) -> &'static str {
    match link {
        LinkState::Unlinked => "unlinked",
        LinkState::VersionMismatch { .. } => "version-mismatch",
        LinkState::Stale { .. } => "stale",
        LinkState::Linked { .. } => "linked",
    }
}

#[cfg(test)]
mod tests {
    use super::link_string;
    use tine_engine::shm_client::LinkState;

    #[test]
    fn link_strings_are_stable() {
        assert_eq!(link_string(&LinkState::Unlinked), "unlinked");
        assert_eq!(
            link_string(&LinkState::VersionMismatch { actual: 2, expected: 1 }),
            "version-mismatch"
        );
        assert_eq!(link_string(&LinkState::Stale { heartbeat_age_ms: 5 }), "stale");
        assert_eq!(link_string(&LinkState::Linked { heartbeat_age_ms: 0 }), "linked");
    }
}
