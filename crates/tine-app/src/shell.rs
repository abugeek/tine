//! The app's parent process (D-017): tray + control plane + pipe server +
//! `--ui` child spawner — and *no* Rune `App`/`wgpu`/`winit` ever
//! initialized here (the window lives in the short-lived child).
//!
//! Shutdown per spec: Quit flushes persistence (dropping every
//! `ControllerHandle` stops the controller thread, which flushes on
//! channel close), stops services, removes the tray, kills the UI child,
//! and exits.

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rune::{Tray, TrayEvent};
use serde_json::{Value, json};
use tine_engine::controller::{Command, Controller, EngineConfig, Event};
use tine_engine::model::{EndpointId, KeepPins, Macro, MonitorSettings};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;
use tine_engine::shm_client::LinkState;
use tine_engine::win::{ComDeviceWatcher, DeviceEvent};

use crate::hotkeys::{HotkeyAction, HotkeyWatcher};
use crate::power::{PowerEvent, PowerWatcher};
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
    /// Global Win32 hotkey (`Ctrl+Shift+M` / `Ctrl+Shift+B`).
    Hotkey(HotkeyAction),
    /// Windows `MMDevice` plug/unplug/default-device event.
    Device(DeviceEvent),
    /// Windows sleep / resume notification.
    Power(PowerEvent),
}

/// Everything the app loop needs; lives on the loop thread.
struct Shell {
    handle: tine_engine::controller::ControllerHandle,
    paths: EnginePaths,
    model: Option<Arc<tine_engine::model::AppModel>>,
    link: LinkState,
    endpoint_names: Option<Vec<(String, String)>>,
    autostart: bool,
    tray: Option<Arc<Tray>>,
    tray_variant: Option<tray::TrayVariant>,
    ui_child: Option<std::process::Child>,
    soaks: Vec<std::thread::JoinHandle<()>>,
    entered: Instant,
    /// Whether the persisted "Hear Myself" state was applied after boot
    /// (the first `ModelChanged`); before T24 it only started once the
    /// window opened, while `status` already reported it as on.
    monitor_restored: bool,
}

impl Shell {
    fn snapshot(&self) -> TraySnapshot {
        let mut s = TraySnapshot::initial();
        s.linked = self.link.is_linked();
        s.muted = services::is_muted();
        s.autostart = self.autostart;
        s.keep_warm = !services::park_when_unused();
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
    const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(350);
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
    let mut endpoint = settings
        .selected_endpoint_id
        .clone()
        .map(EndpointId::new)
        .or_else(default_capture_endpoint)
        .unwrap_or_else(|| EndpointId::new("default"));
    tracing::info!(target: "tine::shell", endpoint = endpoint.as_str(), "controller starting");
    let (handle, rx_events) = Controller::spawn(EngineConfig::block(
        endpoint.clone(),
        Arc::new(tine_protocol::sync::ParamBlock::new()),
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
        paths,
        model: None,
        link: LinkState::Unlinked,
        endpoint_names: None,
        autostart: autostart::is_enabled().unwrap_or(false),
        tray: None,
        tray_variant: None,
        ui_child: None,
        soaks: Vec::new(),
        entered,
        monitor_restored: false,
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
    let _ = services::set_denoise_engine(settings.denoise_engine);
    services::set_park_when_unused(settings.park_when_unused);
    if settings.selected_endpoint_id.is_some()
        && !capture_endpoint_exists(endpoint.as_str())
        && let Some(fallback) = default_capture_endpoint()
        && fallback != endpoint
    {
        tracing::warn!(
            target: "tine::shell",
            stale = endpoint.as_str(),
            fallback = fallback.as_str(),
            "saved capture endpoint no longer exists; falling back to default physical mic"
        );
        endpoint = fallback.clone();
        let mut updated = settings.clone();
        updated.selected_endpoint_id = Some(fallback.as_str().to_owned());
        if let Err(e) = persist::save_settings(&updated, &shell.paths) {
            tracing::warn!(target: "tine::shell", error = %e, "failed to persist fallback endpoint");
        }
        let _ = handle.send(Command::SelectEndpoint(fallback));
    }
    let routed = services::router_follow(endpoint.as_str());
    shell.link = link_from_router(routed);
    shell.sync_tray(&tx);

    // 6c. Watchers: global hotkeys, Windows MMDevice hotplug, and power sleep/resume.
    let (tx_hk, rx_hk) = channel::<HotkeyAction>();
    let _hotkey_watcher = HotkeyWatcher::start(&settings.hotkey_mute, &settings.hotkey_bypass, tx_hk);
    {
        let tx_forward = tx.clone();
        std::thread::Builder::new()
            .name("tine-hk-forward".to_owned())
            .spawn(move || {
                while let Ok(act) = rx_hk.recv() {
                    if tx_forward.send(Msg::Hotkey(act)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }

    let (tx_dev, rx_dev) = channel::<DeviceEvent>();
    let _device_watcher = match ComDeviceWatcher::start(tx_dev) {
        Ok(w) => {
            let tx_forward = tx.clone();
            std::thread::Builder::new()
                .name("tine-dev-forward".to_owned())
                .spawn(move || {
                    while let Ok(ev) = rx_dev.recv() {
                        if tx_forward.send(Msg::Device(ev)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
            Some(w)
        }
        Err(e) => {
            tracing::warn!(target: "tine::shell", error = %e, "device watcher disabled");
            None
        }
    };

    let (tx_pwr, rx_pwr) = channel::<PowerEvent>();
    let power_watcher = PowerWatcher::start(tx_pwr);
    if power_watcher.is_some() {
        let tx_forward = tx.clone();
        std::thread::Builder::new()
            .name("tine-pwr-forward".to_owned())
            .spawn(move || {
                while let Ok(ev) = rx_pwr.recv() {
                    if tx_forward.send(Msg::Power(ev)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }
    let _power_watcher = power_watcher;

    // 6d. First-launch / explicit user launch (MSI "Run Tine now", Start
    //     Menu shortcut, or double-clicking tine.exe without --background):
    //     open the window immediately so first-run onboarding appears.
    //     Autostart (--background) and --metrics stay headless in the tray.
    if !background && !metrics && !settings.start_minimized {
        shell.open_ui();
    }

    // 7. Event loop.
    let mut code = exit_code::OK;
    let mut last_health_check = Instant::now();
    let mut recovery = RecoveryState::new(routed, Instant::now());
    loop {
        match rx.recv_timeout(HEALTH_CHECK_INTERVAL) {
            Ok(msg) => match msg {
                Msg::Engine(Event::ModelChanged(model)) => {
                    shell.ensure_endpoint_names();
                    shell.model = Some(model);
                    if let Some(endpoint) = shell.model.as_ref().and_then(|m| m.endpoint.as_ref()) {
                        let routed = services::router_follow(endpoint.as_str());
                        shell.link = link_from_router(routed);
                        recovery.record_result(routed, Instant::now());
                    }
                    shell.restore_monitor_once();
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
                Msg::Hotkey(act) => match act {
                    HotkeyAction::ToggleMute => {
                        let muted = services::toggle_muted();
                        tracing::info!(target: "tine::hotkeys", muted, "mute hotkey pressed");
                        shell.sync_tray(&tx);
                    }
                    HotkeyAction::ToggleBypass => {
                        let next = shell.model.as_ref().is_none_or(|m| !m.bypass);
                        tracing::info!(target: "tine::hotkeys", bypass = next, "bypass hotkey pressed");
                        let _ = shell.handle.send(Command::SetBypass(next));
                    }
                },
                Msg::Device(ev) => {
                    let selected_id = shell
                        .model
                        .as_ref()
                        .and_then(|m| m.endpoint.as_ref())
                        .map_or_else(|| endpoint.as_str().to_owned(), |e| e.as_str().to_owned());
                    if recovery.on_device_event(&selected_id, &ev, Instant::now()) {
                        shell.endpoint_names = None;
                    }
                }
                Msg::Power(PowerEvent::Suspending) => {
                    tracing::info!(target: "tine::power", "system suspending; pausing router recovery");
                    recovery.on_suspend();
                    services::router_stop();
                    shell.link = LinkState::Unlinked;
                    shell.sync_tray(&tx);
                }
                Msg::Power(PowerEvent::Resumed) => {
                    tracing::info!(target: "tine::power", "system resumed; scheduling audio recovery");
                    recovery.on_resume(Instant::now());
                }
                Msg::Tray(TrayEvent::Activate { .. }) => shell.open_ui(),
                Msg::Tray(TrayEvent::Menu(id)) if id == menu_id::OPEN => shell.open_ui(),
                Msg::Tray(TrayEvent::Menu(id)) if id == menu_id::QUIT => {
                    tracing::info!(target: "tine::shell", "quit requested from tray");
                    code = exit_code::OK;
                    break;
                }
                Msg::Tray(TrayEvent::Menu(id)) => {
                    shell.on_tray_menu(&id);
                    shell.sync_tray(&tx);
                }
                Msg::Pipe(job) => {
                    if matches!(job.request, PipeRequest::Quit) {
                        tracing::info!(target: "tine::shell", "quit requested via pipe");
                        let _ = job.reply.send(PipeResponse::empty());
                        code = exit_code::OK;
                        break;
                    }
                    shell.on_pipe(job, &tx);
                }
                Msg::Engine(Event::ParamsResolved(params)) => {
                    services::publish_params(&params);
                }
                Msg::Tray(_) => {}
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if last_health_check.elapsed() >= HEALTH_CHECK_INTERVAL || recovery.is_due(now) {
            last_health_check = now;
            let ep = shell
                .model
                .as_ref()
                .and_then(|m| m.endpoint.as_ref())
                .map_or_else(|| endpoint.as_str().to_owned(), |e| e.as_str().to_owned());
            let out_dev = shell.model.as_ref().and_then(|m| m.monitor.output_device.clone());
            let has_err = services::router_has_error();
            if has_err {
                if recovery.on_stream_error(now) {
                    tracing::warn!(target: "tine::shell", endpoint = %ep, "router stream error detected; rebuilding");
                    let routed = services::router_force_rebuild(&ep);
                    shell.link = link_from_router(routed);
                    recovery.record_result(routed, now);
                    if routed {
                        let _ = services::monitor_recover(out_dev.as_deref());
                    }
                    shell.sync_tray(&tx);
                } else {
                    services::router_stop();
                    shell.link = LinkState::Unlinked;
                    shell.sync_tray(&tx);
                }
            } else if recovery.take_due_rebuild(now) {
                let routed = services::router_force_rebuild(&ep);
                shell.link = link_from_router(routed);
                recovery.record_result(routed, now);
                if routed {
                    tracing::info!(target: "tine::shell", endpoint = %ep, "router recovered");
                    let _ = services::monitor_recover(out_dev.as_deref());
                }
                shell.sync_tray(&tx);
            } else if services::monitor_has_error() {
                tracing::warn!(target: "tine::shell", "monitor stream error detected; rebuilding");
                let _ = services::monitor_recover(out_dev.as_deref());
            }
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
    // Write pending settings before the process can exit (nothing joins the
    // controller thread, so the drop-triggered flush alone could be lost).
    if !handle.flush_blocking(Duration::from_secs(2)) {
        tracing::warn!(target: "tine::shell", "settings flush on shutdown timed out");
    }
    drop(shell); // drops its controller handle clone
    drop(handle); // drops the last handle -> controller thread exits
    drop(forwarder);
    drop(guard); // releases `Local\Tine.SingleInstance` and closes the pipe name
    code
}

/// Returns `true` if `target` matches an active capture device (or if
/// enumeration fails, so a transient COM error never overwrites a saved
/// endpoint).
fn capture_endpoint_exists(target: &str) -> bool {
    let Ok(inputs) = tine_engine::audio_io::list_inputs() else {
        return true;
    };
    if inputs.is_empty() {
        return true;
    }
    let raw_target = target.strip_prefix("wasapi:").unwrap_or(target);
    inputs.iter().any(|d| {
        let raw_id = d.id.strip_prefix("wasapi:").unwrap_or(&d.id);
        d.id == target || raw_id == raw_target
    })
}

/// Best-effort default capture endpoint: prefers the Windows default
/// physical mic (excluding virtual cables like VB-Audio / `VoiceMeeter` / VAC / Sonar),
/// then any physical mic, falling back to the first active device only if
/// nothing else exists. COM; used only when nothing is persisted yet.
fn default_capture_endpoint() -> Option<EndpointId> {
    if let Ok(inputs) = tine_engine::audio_io::list_inputs()
        && !inputs.is_empty()
    {
        let chosen = inputs
            .iter()
            .find(|d| d.is_default && !tine_engine::apo_admin::is_virtual_cable_capture(&d.name))
            .or_else(|| {
                inputs
                    .iter()
                    .find(|d| !tine_engine::apo_admin::is_virtual_cable_capture(&d.name))
            })
            .or_else(|| inputs.iter().find(|d| d.is_default))
            .or_else(|| inputs.first());
        if let Some(dev) = chosen {
            return Some(EndpointId::new(dev.id.clone()));
        }
    }
    tine_engine::apo_admin::list_capture_endpoints().ok().and_then(|list| {
        list.iter()
            .find(|ep| !tine_engine::apo_admin::is_virtual_cable_capture(&ep.name))
            .or_else(|| list.first())
            .map(|ep| EndpointId::new(ep.id.clone()))
    })
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
            menu_id::MUTE => {
                let muted = services::toggle_muted();
                tracing::info!(target: "tine::tray", muted, "mute toggled from tray");
            }
            menu_id::BYPASS => {
                let next = self.model.as_ref().is_none_or(|m| !m.bypass);
                let _ = self.handle.send(Command::SetBypass(next));
            }
            menu_id::MONITOR => {
                let current = self.model.as_ref().is_some_and(|m| m.monitor.enabled);
                let next = !current;
                self.apply_monitor(next, None);
            }
            menu_id::KEEP_WARM => {
                let next_park = !services::park_when_unused();
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.park_when_unused = next_park;
                if let Err(e) = persist::save_settings(&settings, &self.paths) {
                    tracing::error!(target: "tine::shell", error = %e, "failed to save park_when_unused");
                }
                services::set_park_when_unused(next_park);
                tracing::info!(target: "tine::shell", keep_warm = !next_park, "keep-warm PTT toggled");
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

    /// Starts "Hear Myself" from the persisted settings once, after the
    /// first model arrives (the router is already following by then; if it
    /// is not, the services start the monitor when routing comes up).
    /// Nothing is re-persisted: the controller already holds this state.
    fn restore_monitor_once(&mut self) {
        if self.monitor_restored {
            return;
        }
        let Some(model) = self.model.as_ref() else {
            return;
        };
        self.monitor_restored = true;
        if model.monitor.enabled {
            services::monitor_apply(true, model.monitor.output_device.as_deref());
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

    fn on_pipe(&mut self, job: PipeJob, tx: &Sender<Msg>) {
        let response = self.execute_pipe(job.request, tx);
        let _ = job.reply.send(response);
    }

    fn execute_pipe(&mut self, req: PipeRequest, tx: &Sender<Msg>) -> PipeResponse {
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
                let notes = if state.get("flow").and_then(Value::as_str) == Some("running") {
                    Vec::new()
                } else {
                    let list = services::take_messages();
                    for note in &list {
                        tracing::warn!(target: "tine::shell", "{note}");
                    }
                    list
                };
                PipeResponse::ok(json!({ "calibrate": state, "notes": notes }))
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
            PipeRequest::CalibratePreview {
                style,
                keep_pins,
                audition_raw,
            } => {
                let parsed = tine_dsp::calibrator::VoiceStyle::from_name(&style)
                    .unwrap_or(tine_dsp::calibrator::VoiceStyle::AutoBalanced);
                let out_dev = self.model.as_ref().and_then(|m| m.monitor.output_device.clone());
                PipeResponse::ok(services::autotune_preview(
                    parsed,
                    keep_pins,
                    audition_raw,
                    out_dev.as_deref(),
                ))
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
            PipeRequest::SetProDrawerOpen { open } => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.pro_drawer_open = open;
                match persist::save_settings(&settings, &self.paths) {
                    Ok(()) => PipeResponse::ok(json!({ "pro_drawer_open": open })),
                    Err(e) => {
                        tracing::error!(target: "tine::shell", error = %e, "failed to save pro_drawer_open");
                        PipeResponse::err(exit_code::FAILURE, format!("Failed to save settings: {e}"))
                    }
                }
            }
            PipeRequest::OnboardingDone => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.onboarding_completed = true;
                match persist::save_settings(&settings, &self.paths) {
                    Ok(()) => PipeResponse::ok(json!({ "onboarding_done": true })),
                    Err(e) => {
                        tracing::error!(target: "tine::shell", error = %e, "failed to persist onboarding completion");
                        PipeResponse::err(exit_code::FAILURE, format!("Failed to save settings: {e}"))
                    }
                }
            }
            PipeRequest::RouterFollow => {
                let endpoint_id = self
                    .model
                    .as_ref()
                    .and_then(|m| m.endpoint.as_ref())
                    .map(|e| e.as_str().to_owned())
                    .or_else(|| persist::load_settings(&self.paths).into_value().selected_endpoint_id)
                    .or_else(|| default_capture_endpoint().map(|e| e.as_str().to_owned()));
                if let Some(ep) = endpoint_id {
                    let routed = services::router_follow(&ep);
                    self.link = link_from_router(routed);
                    self.sync_tray(tx);
                    PipeResponse::ok(json!({ "routed": routed, "endpoint": ep }))
                } else {
                    PipeResponse::err(exit_code::NOT_RUNNING, "no capture endpoint selected")
                }
            }
            PipeRequest::SetDenoiseEngine { engine } => {
                let Some(parsed) = tine_protocol::params::DenoiseEngine::from_name(&engine) else {
                    return PipeResponse::err(
                        exit_code::USAGE,
                        format!("unknown denoiser `{engine}` (rnnoise|deepfilter|off)"),
                    );
                };
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.denoise_engine = parsed;
                if let Err(e) = persist::save_settings(&settings, &self.paths) {
                    tracing::error!(target: "tine::shell", error = %e, "failed to save denoise_engine");
                    return PipeResponse::err(exit_code::FAILURE, format!("Failed to save settings: {e}"));
                }
                let routed = services::set_denoise_engine(parsed);
                if self.model.as_ref().and_then(|m| m.endpoint.as_ref()).is_some() {
                    self.link = link_from_router(routed);
                }
                self.sync_tray(tx);
                PipeResponse::ok(json!({
                    "denoise_engine": parsed.as_str(),
                    "routed": routed,
                }))
            }
            PipeRequest::SetParkWhenUnused { enabled } => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.park_when_unused = enabled;
                if let Err(e) = persist::save_settings(&settings, &self.paths) {
                    tracing::error!(target: "tine::shell", error = %e, "failed to save park_when_unused");
                    return PipeResponse::err(exit_code::FAILURE, format!("Failed to save settings: {e}"));
                }
                services::set_park_when_unused(enabled);
                self.sync_tray(tx);
                PipeResponse::ok(json!({ "park_when_unused": enabled }))
            }
            PipeRequest::MicCheckStart => {
                let params = self
                    .model
                    .as_deref()
                    .map_or_else(services::last_controller_params, tine_engine::model::resolve);
                let out_dev = self.model.as_ref().and_then(|m| m.monitor.output_device.clone());
                PipeResponse::ok(services::mic_check_start(params, out_dev.as_deref()))
            }
            PipeRequest::MicCheckQuery => PipeResponse::ok(services::mic_check_query()),
            PipeRequest::MicCheckSetMode { raw } => {
                let updated = services::mic_check_set_mode(raw);
                PipeResponse::ok(json!({ "raw": raw, "active": updated }))
            }
            PipeRequest::MicCheckStop => {
                services::mic_check_stop();
                PipeResponse::ok(json!({ "flow": "idle" }))
            }
            PipeRequest::Mute { on } => {
                let muted = match on {
                    Some(v) => services::set_muted(v),
                    None => services::toggle_muted(),
                };
                self.sync_tray(tx);
                PipeResponse::ok(json!({ "muted": muted }))
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
            "muted": services::is_muted(),
            "denoise_engine": services::denoise_engine().as_str(),
            "park_when_unused": services::park_when_unused(),
            "state": if self.link.is_linked() {
                if model.is_some_and(|m| m.bypass) { "bypassed" } else { "processing" }
            } else {
                "inactive"
            },
            "link": link_string(&self.link),
            "monitor": model.is_some_and(|m| m.monitor.enabled),
            "monitor_device": model.and_then(|m| m.monitor.output_device.clone()),
            "router": services::router_status(),
            "raw_mic_apps": services::raw_mic_apps(),
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

/// Pure recovery state machine for USB mic unplug/replug & sleep/resume.
struct RecoveryState {
    suspended: bool,
    pending_rebuild_at: Option<Instant>,
    backoff: Duration,
}

impl RecoveryState {
    const MIN_BACKOFF: Duration = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    const REPLUG_SETTLE: Duration = Duration::from_millis(300);
    const RESUME_SETTLE: Duration = Duration::from_millis(600);

    fn new(initially_routed: bool, now: Instant) -> Self {
        Self {
            suspended: false,
            pending_rebuild_at: (!initially_routed).then_some(now + Self::MIN_BACKOFF),
            backoff: Self::MIN_BACKOFF,
        }
    }

    fn on_suspend(&mut self) {
        self.suspended = true;
        self.pending_rebuild_at = None;
    }

    fn on_resume(&mut self, now: Instant) {
        self.suspended = false;
        self.backoff = Self::MIN_BACKOFF;
        self.pending_rebuild_at = Some(now + Self::RESUME_SETTLE);
    }

    /// Returns `true` when the cached endpoint list should be invalidated.
    fn on_device_event(&mut self, selected_id: &str, ev: &DeviceEvent, now: Instant) -> bool {
        let invalidate = matches!(
            ev,
            DeviceEvent::Added(_) | DeviceEvent::Removed(_) | DeviceEvent::StateChanged { .. }
        );
        if self.suspended {
            return invalidate;
        }
        let is_selected = |id: &str| id == selected_id || selected_id == "default";
        match ev {
            DeviceEvent::Added(id) | DeviceEvent::StateChanged { id, active: true } if is_selected(id) => {
                self.backoff = Self::MIN_BACKOFF;
                self.pending_rebuild_at = Some(now + Self::REPLUG_SETTLE);
            }
            DeviceEvent::DefaultChanged(_) if selected_id == "default" => {
                self.backoff = Self::MIN_BACKOFF;
                self.pending_rebuild_at = Some(now + Self::REPLUG_SETTLE);
            }
            DeviceEvent::Removed(id) | DeviceEvent::StateChanged { id, active: false }
                if is_selected(id) && self.pending_rebuild_at.is_none() =>
            {
                self.pending_rebuild_at = Some(now + self.backoff);
            }
            _ => {}
        }
        invalidate
    }

    fn on_stream_error(&mut self, now: Instant) -> bool {
        if self.suspended {
            return false;
        }
        let immediate = self.pending_rebuild_at.is_none();
        if !immediate {
            self.pending_rebuild_at = Some(now + self.backoff);
        }
        immediate
    }

    fn is_due(&self, now: Instant) -> bool {
        !self.suspended && self.pending_rebuild_at.is_some_and(|t| now >= t)
    }

    fn take_due_rebuild(&mut self, now: Instant) -> bool {
        if self.is_due(now) {
            self.pending_rebuild_at = None;
            true
        } else {
            false
        }
    }

    fn record_result(&mut self, ok: bool, now: Instant) {
        if ok {
            self.backoff = Self::MIN_BACKOFF;
            self.pending_rebuild_at = None;
        } else if !self.suspended {
            self.pending_rebuild_at = Some(now + self.backoff);
            self.backoff = (self.backoff * 2).min(Self::MAX_BACKOFF);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryState, link_string};
    use std::time::{Duration, Instant};
    use tine_engine::shm_client::LinkState;
    use tine_engine::win::DeviceEvent;

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

    #[test]
    fn recovery_handles_unplug_backoff_and_fast_replug() {
        let t0 = Instant::now();
        let mut r = RecoveryState::new(true, t0);
        let mic = "usb-mic-1";

        // Unplug fails first rebuild, backing off from 1s -> 2s.
        assert!(r.on_device_event(
            mic,
            &DeviceEvent::StateChanged {
                id: mic.to_owned(),
                active: false
            },
            t0
        ));
        r.record_result(false, t0);
        assert!(!r.take_due_rebuild(t0 + Duration::from_millis(500)));
        assert!(r.take_due_rebuild(t0 + Duration::from_secs(1)));

        r.record_result(false, t0 + Duration::from_secs(1));
        assert!(!r.take_due_rebuild(t0 + Duration::from_secs(2)));

        // Replugging the mic resets backoff and schedules fast 300 ms recovery.
        let replug_at = t0 + Duration::from_millis(2100);
        assert!(r.on_device_event(
            mic,
            &DeviceEvent::StateChanged {
                id: mic.to_owned(),
                active: true
            },
            replug_at
        ));
        assert!(!r.take_due_rebuild(replug_at + Duration::from_millis(200)));
        assert!(r.take_due_rebuild(replug_at + Duration::from_millis(300)));
    }

    #[test]
    fn recovery_suppresses_work_during_suspend_and_recovers_on_resume() {
        let t0 = Instant::now();
        let mut r = RecoveryState::new(true, t0);
        r.on_suspend();
        assert!(!r.on_stream_error(t0 + Duration::from_millis(10)));
        assert!(!r.take_due_rebuild(t0 + Duration::from_secs(5)));

        let resume_at = t0 + Duration::from_secs(10);
        r.on_resume(resume_at);
        assert!(!r.take_due_rebuild(resume_at + Duration::from_millis(300)));
        assert!(r.take_due_rebuild(resume_at + Duration::from_millis(600)));
    }
}
