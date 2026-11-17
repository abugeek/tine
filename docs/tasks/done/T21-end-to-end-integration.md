# T21 — End-to-End Integration in `tine.exe`

| Field | Value |
| :--- | :--- |
| Status | done (t21-worker, Mistral Vibe) |
| Lane | Z — Integration |
| Depends on | T14, T17, T20 |
| Unblocks | T22 |
| Owns | `crates/tine-app/src/ui_bridge.rs`, `crates/tine-app/src/main.rs` (wiring only), integration tests in `crates/tine-app/tests/**` |
| Size | M |

## Goal
Replace the placeholder view in `tine.exe` with `tine-ui`. Bridge `Intent` ⇄ engine `Command`/`Event` and `UiFrame` → `LiveFrame`, and run the full flows live: onboarding → APO install → Auto-Tune → macro tweaks → Pro edits → reboot persistence.

## Read first
Contracts of T15–T18 · [05-ui-ux.md](../project/05-ui-ux.md) · [01-vision.md](../project/01-vision.md) success criteria

## Steps
- [x] `ui_bridge.rs`:
  - `IntentSink` implementation maps each `Intent` to `ControllerHandle::send`, `AutoTuneRunner`, `Monitor` or `apo_admin::request` (the latter on a worker thread, with the result posted back through `AppHandle::update`)
  - engine `Event::ModelChanged` → `handle.update(|s, _| s.model = map(model))`
  - `Visualizer` frames → `handle.update` at ≤ 60 Hz, only while visible (coalesce: an update in flight drops the older frame)
- [x] Endpoint list + `DeviceWatcher` events → model updates; the APO badge comes from `health()`.
- [x] First-run detection (no settings file) → onboarding flow.
- [x] Error surfacing: every engine and admin error maps to a user message + log entry; no silent failures.
- [x] Integration tests:
  - drive the full app headlessly with a fake engine backend (`ParamTarget` in-process, fake audio source)
  - assert intents produce the expected params and persisted files
  - one manual test script for the live system, written as a checklist in `## Result

**Status: done — including the orchestrator's follow-up items (R-20 pipe extension, D-018 production router wiring, R-21 trim measurements), all completed inside this PR under explicit authorization.** This task was authored before decisions D-017 (window in a `--ui` child process; the parent is Rune/wgpu-free) and D-018 (virtual-cable routing, no APO) existed, so the one-process `IntentSink → ControllerHandle` sketch of the spec cannot be wired literally: a second controller would double-persist `settings.json`/profiles, and Rune in the parent breaks the idle budgets. `ui_bridge.rs` therefore hosts **two realizations of the same mapping**: `EngineBridge` — the spec's full in-process app, driven headlessly by the integration suite — and the child glue, which compiles the same adapters against the parent's authoritative controller over the command pipe. Decisions recorded in **D-027**; the follow-up's production wiring in **D-028**.

### What runs now, headlessly (the spec's flows, proven)

`spectrum()`/services: the parent's `meters` spectrum is Visualizer-fed (`services::spectrum()` — pulls latest-wins frames from a T16 `Visualizer` pump that only exists while a consumer keeps asking; a 10 s-tail reaper tears the pump down), `calibrate` runs the **real T16 `AutoTuneRunner`** with the real `AutoCalibratorFactory` and capture (`services::autotune_start()` + additive `autotune_cancel`/`autotune_state`/`autotune_take_pending`/`autotune_discard`), and `monitor_apply` starts/restarts/stops the **real T16 `Monitor`** (additive `monitor_status`/`shutdown`).

`tests/e2e_flow.rs` (6 tests; real `Controller` + in-process `ParamBlock` target + fake audio producer + a session-local mapping with a lease-honoring bypass thread standing in for the processing side):
- **intents → params + files**: `SetMacro(Purity, 0.1)` → published snapshot bit-compares against `resolve()` of the engine model (**exact `DspParams` equality**); `SetParam(DenoiseMix, 0.9)` pins (D-007) and lands in `profiles/{id}.json` (`pins` + `overrides` verified), `settings.json` and the boot-params file exist after the 500 ms debounce; `ToggleBypass` flips the model and the published bypass flag.
- **frames → LiveFrame**: a −20 dBFS 1 kHz fake source lights the mid-spectrum band (level > 0.6 within 2 s; peak band in the 12–24 range); the visualizer only runs while "visible" (start/stop proven by `is_visualizing`).
- **monitor intents**: `ToggleMonitor`/`PickMonitorDevice` drive the `MonitorHost` (recorder in-test; the production `CpalMonitorHost` is the same code the parent uses), persist `monitor_enabled` both ways, and expose the **41.6 ms** latency badge in the `UiModel` (D-026 formula asserted).
- **endpoint selection**: `SelectEndpoint(1)` → new profile created and persisted (`profiles/{mic-b}.json`, `settings.selected_endpoint_id`), params re-resolved for the fresh midpoint profile; out-of-range picks surface a user message.
- **Auto-Tune, full flow**: `StartAutoTune` (fake-APO lease mapping + scripted 1 s/3 s/5 s capture: LCG noise ≈ −51 dBFS, 150 Hz sine −23 dBFS) through the **real calibrator** reaches the Results card; the report satisfies T11's acceptance semantics (**speech RMS + input gain ≈ −18 dBFS within 1.0 dB, simulated peak inside −6…−3 dBFS, denoise ≥ 0.5**); `ApplyCalibration { keep_pins: false }` adopts it into the model, releases the calibration-driven pin, changes the gate threshold, persists the calibration + updated boot params, and closes the card. Wall time was **5.1–5.7 s per run** (non-real-time capture).
- **reboot persistence**: drop of every `ControllerHandle` flushes; a fresh controller on the same paths restores macro 0.9 / pin + override 123 Hz; `first_run` flips to false; published params equal `resolve()` of the reloaded model (**exact equality**).
- **error surfacing**: every failure path asserted non-silent (missing processing link, vanished mic, `Hear Myself` start failure; the parent-side services mirror the same messages).

`tests/pipe_flow.rs` (new, follow-up item 1): **the newly unparked intents end-to-end through the real named pipe** — a real `single_instance` pipe server thread resolves `shell::pipe_control_command` (the same seam `Shell::execute_pipe` uses) into a real controller spawned on an in-process `ParamBlock`: `set` (pins, D-007) → `unpin { key }` release arrives as `Event::ModelChanged` with the pin gone → `unpin_all`; plus the usage-error path (`unknown key` → `ok=false`, exit code 2) and `calibrate_query` answering the flow state over the same pipe.

Unit coverage: `ui_bridge.rs` (8 tests: band normalization, `meters`-payload parsing, the D-018 badge matrix, `param_key` ⇄ CLI round-trips for every SPECS entry, the D-026 latency formula, and the new **wizard wire adapter** — running phase → `AutoTuneStep::Silence`, `report-pending` → `Results` + a `CalibrationUi` card with SPECS-key → `ParamId` diff rows (label/unit), `failed` → the typed `CalFailureUi` card, `idle`, unknown flows skipped); `services.rs` (6 tests: spectrum floor without a link, the surfaced missing-link message, idle-side accessors, **the lease shim** — renews the lease through the runner's own `renew_lease`, confirms the `BYPASS` state-flag handshake, asserts DSP bypass on the shared block, restores on `clear_lease` — and the monitor pair); `proto.rs` round-trips all 18 requests incl. the seven new ones; `shell.rs` link strings.

`tests/hw_router_live.rs` (new, `#[ignore = "hardware: …"]`, run on demand, excluded from CI): the live composite of the whole D-018 stack — real default mic → `VirtualCableProcessor` → the discovered virtual cable sink → the T16 visualizer on the shared tap ring — proving the tap drains (8192-sample direct pop) and the analyzer reads real (floor-level, silent-room) bands.

### What runs now, live on this machine (Release smoke runs in this session, final round)

- **Two-process parent with real audio routing (follow-up item 2)**: `tine.exe --background` spawns the controller against `EngineConfig::block` + starts the **`VirtualCableRouter` in the parent**: `status` answers `state: processing`, `link: linked` (D-028: the router's health is the badge), and a live `router` block — on this machine the sink discovery resolves **"SteelSeries Sonar - Chat (SteelSeries Sonar Virtual Audio Device)"**, `{callbacks: 778…20082, underruns: 0, overruns: 0, max_callback_us: 196, rate_hz: 48000}`. The tray icon flips to Active when the loop starts.
- **Live meters** (`meters` over the pipe): real post-DSP telemetry (input −78…−80 dBFS room floor, gate attenuation −18…−22 dB tracking the room, output at the floor) with the telemetry sequence advancing monotonically across pulls, and the 32 spectrum bands at the analyzer floor (−87…−90 dBFS) — consistent with a silent room behind a closed gate; the gate reacts live to a played tone (−18.3 → −9.2 dB), proving real audio flows through the DSP chain in production. A brief pump-boot transient on the first pull after a 10 s idle is by design (the reaper).
- **Live Auto-Tune (the D-018 lease shim + tap-fed capture)**: `calibrate` over the pipe → the wizard state machine runs **live**: `Countdown → Noise (0.12…0.80, level −78…−80 dBFS real room noise) → Voice (0.09…0.92, level −76.5…−79.5)` → terminal at t+9 s with the **typed rejection `NotEnoughSpeech { voiced_ms: 0 }`** — the honest outcome for a silent 6 AM room nobody spoke into (same class of evidence T16 recorded: `TooQuiet` on the real silent mic). No bypass residue afterwards (`status.bypass: false`; persisted state byte-identical after restore).
- **The newly unparked intents, live round trips** (follow-up item 1): `endpoint {id}` → ack + model/event round trip; `unpin {key}`; `unpin_all`; `calibrate_discard` — all `ok` over the same pipe, all state-changing effects verified, machine state restored from backup afterwards (`diff -r` identical).
- **Earlier rounds this session** (kept for the record): the real child renders the full `tine-ui` tree (83 elements, `target/t21-child.png` + `t21-child.tree.json`); the two-process pipe smoke (`status`/`meters`/`macro body 0.7`/`bypass on`/`calibrate`/`monitor on`) logged verbatim at the time, incl. **`Hear Myself started latency_ms=41.6`** (the real T16 Monitor on the real mic = the D-026 predicted badge); a second launch forwarded `open` and the parent spawned the real `--ui` child.

### Two real bugs found and fixed by the live smoke (recorded honesty)

1. A **triple single-reader violation** in my first meters implementation: reading the routing loop's telemetry `Triple` from the shell thread raced the visualizer pump (D-026). Fixed by reading only the pump-**published** `UiFrame` — one triple read per pull under the services lock, cached (`FrameSnapshot` now carries the frame's telemetry); `meters` pulls spectrum first, then serves telemetry from the cache.
2. The AB13X USB mic **refuses a second shared-mode capture client** (the first live calibrate attempt surfaced `Auto-Tune failed: capture stream lost` — correctly, no silent failure). Fixed inside `services.rs`: the calibrator now captures via a `SampleSource` that drains the routing tap while the pump is parked (single tap reader), which is the raw bypassed signal during the lease (D-028).

### Open items (kept precise)

1. **`tine.exe` = 21,627,392 bytes (20.63 MiB)**, over the 20 MB budget (R-21). The orchestrator's three legitimate trims were **measured and reverted** where they failed: `strip = true` → 21,628,416 (a 512-byte no-op on MSVC; the PDB already carries the line tables via `split-debuginfo = "packed"`); `lto = "thin"` → 22,895,616 (**larger** by 1.25 MB; also risks the DSP perf gate); `codegen-units` already 1. The budget row (T01-owned `ci.yml` + §7) was deliberately not touched — the size decision stays with the maintainer (raise to 24 MB, or trade Auto-Tune's in-app offline verification).
2. **"Hear Myself" monitors the input signal**: the processed-capture variant needs WASAPI render-loopback, which lives outside T12's audio IO (documented in D-028).

### Manual test checklist for the maintainer (the human-timed fresh-profile run I cannot perform)

Prep: rename `%APPDATA%\Tine` aside (fresh profile), build `--release`, keep a timer and (optionally) a screen recorder running.

1. [ ] **First run (target < 2 min, timed)**: launch `tine.exe` → tray appears (the log shows `cold_start_tray_ms` ≈ 12–15) and the badge reads Active (the router is up). Open the window → the **onboarding overlay** shows (fresh settings file). Steps: pick the mic → the suggested archetypes are pre-marked (skip Install — D-018: the cable step shows the remedy if no VB-Cable is present) → `Auto-Tune (8s)` → **the in-window wizard now tracks progress live** (the child polls `calibrate_query` at ~10 Hz; countdown ring → silence phase → speak phase with the live level ring) → speak normally **during the voice phase** → the Results card appears with the measured dBFS numbers and per-parameter diffs → Apply. Timing ends when Discord's mic meter reacts to Auto-Tune's settings.
2. [ ] **Every control**: with the window open, drag each macro node (audio/meters change within ~50 ms), click each archetype pill, toggle A/B with `Space` (**click-free**), toggle Hear Myself (badge shows ~42 ms; note it replays the *input* signal — D-028 limit), open the Pro drawer and edit one numeric field (clamped inline message on out-of-range input), switch the mic in the picker (endpoint switching is live now), pin/unpin via the rack (both live), and run Auto-Tune again from the window (cancel mid-run also covered).
3. [ ] **Persistence**: change Body to ~0.7 → `tine status` reflects it → Quit, relaunch → same state on reopen (the window shows the current profile on the first frame).
4. [ ] **Boot/restart resilience**: with Tine autostarted, set a distinctive macro, quit Tine, and confirm state resumes before the UI reopens; real processed audio flows through the virtual cable from boot (this round: the parent starts the router before the event loop).
5. [ ] **Close the window**: confirm in Task Manager that the `--ui` child exits (GPU memory released; parent CPU < 0.5 %), the visualizer is stopped (no `tine-visualizer` thread in the parent after the 10 s tail), and a reopen shows the current state settled on the first frame.
6. [ ] **A fresh Windows user profile** replay of (1) is the acceptance run to record with timings.

### Verification

- `cargo nextest run --workspace`: **275 passed, 6 skipped** (the registry round-trip + hardware-only composites excluded by design), all green. `tine-app` alone: 61 run, 61 passed, 2 skipped (hw composites on demand: `cargo nextest run -p tine-app --run-ignored ignored-only`).
- `cargo build -p tine-app --release`: `tine.exe` = **21,627,392 bytes** (open item 1 / R-21).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean. `cargo fmt --check -p <pkg>` per package (R-19: `--all` deadlocks on `tine-dsp`): **all crates clean**, no orphaned `rustfmt.exe`.
- Live smoke evidence above; machine Tine state (`%APPDATA%\Tine`) restored from backup and verified byte-identical (`diff -r`) after the smoke; no `tine.exe` left running.

## Requests (historical: items 1–2 were completed inside this PR under the orchestrator's follow-up authorization; remaining decisions below)

1. **Pipe protocol extension** — DONE in this PR (follow-up): the additive `PipeRequest` variants, `execute_pipe` arms, `shell::pipe_control_command`, and the child routing landed, with `tests/pipe_flow.rs` covering the newly unparked intents end-to-end through the real pipe and a live round-trip smoke. R-20 closed. (Historical: the ~40-line estimate was close — ~110 additive line pairs including tests.)
2. **D-018 router wiring in the shell** — DONE in this PR (follow-up): `EngineConfig::block` on `services::param_block`, the `VirtualCableRouter` in the parent (endpoint-following restarts), the D-028 lease shim + tap-fed calibrator, and the meters single-reader fix; the live smoke shows the `processing`/`linked` badge, live telemetry and bands, and the full in-window wizard flow. (Historical request superseded; the detailed evidence lives in the Result above.)
3. **`tine.exe` size budget decision** (owner: T01 / root `ci.yml` + `docs/project/09-engineering-standards.md` §7): **still open — the orchestrator's legitimate trims did not reach the 20 MB gate.** Measured on this branch: 21,627,392 bytes (21,440,512 before the router/wizard integration); `strip = true` is a no-op on MSVC (21,628,416 — the PDB already carries line tables via `split-debuginfo = "packed"`), `lto = "thin"` is *larger* (22,895,616) and risks the DSP perf gate, `codegen-units` is already 1. Either raise the budget to 24 MB or trade away the in-app offline verification (which links `DspPipeline` incl. the `nnnoiseless` weights). See R-21/D-027/D-028.
4. **`ui.rs` deletion + lib-target split** (FYI, cross-owns applied under the view-swap work item): the placeholder view module `crates/tine-app/src/ui.rs` was deleted (its whole content was the placeholder the task replaces); `main.rs` is a thin binary over the new `tine_app` lib target (needed so `tests/e2e_flow.rs` can drive the app's own bridge - a binary cannot be imported by integration tests). Bless `crates/tine-app/Cargo.toml` ([lib] section, `tine-dsp` dep for the calibrator presets, `Win32_System_Memory` windows feature for the fake-APO test mapping, `tempfile` dev-dep) and `Cargo.lock`.
