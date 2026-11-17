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
  - one manual test script for the live system, written as a checklist in `## Result`

## Acceptance criteria
- Fresh Windows user profile: first run → onboarding → APO installed → Auto-Tune → Discord hears processed audio, all in under 2 minutes (timed, recorded).
- Every UI control changes the live sound or state as labelled. The A/B `Space` toggle is click-free.
- Closing the window: GPU memory freed, visualizer stopped, CPU < 0.5 %. Reopen shows current state, settled on the first frame.
- Reboot → same processing before login completes; the UI reopens with the same state.

## Verification
```bash
cargo nextest run -p tine-app
cargo build -p tine-app --release
```

## Result

**Status: complete within this task's ownership; two cross-owns applied with flags and four live-window gaps parked as precise Requests (D-027, R-20, R-21).** This task was authored before decisions D-017 (window in a `--ui` child process; the parent is Rune/wgpu-free) and D-018 (virtual-cable routing, no APO) existed, so the one-process `IntentSink → ControllerHandle` sketch of the spec cannot be wired literally: a second controller would double-persist `settings.json`/profiles, and Rune in the parent breaks the idle budgets. `ui_bridge.rs` therefore hosts **two realizations of the same mapping**: `EngineBridge` — the spec's full in-process app, driven headlessly by the integration suite — and the child glue, which compiles the same adapters against the parent's authoritative controller over the command pipe. All decisions recorded in **D-027**.

### What runs now, headlessly (the spec's flows, proven)

`spectrum()`/services: the parent's `meters` spectrum is Visualizer-fed (`services::spectrum()` — pulls latest-wins frames from a T16 `Visualizer` pump that only exists while a consumer keeps asking; a 10 s-tail reaper tears the pump down), `calibrate` runs the **real T16 `AutoTuneRunner`** with the real `AutoCalibratorFactory` and device capture (`services::autotune_start()` + new additive `autotune_cancel`/`autotune_state`/`take_messages`), and `monitor_apply` starts/restarts/stops the **real T16 `Monitor`** (additive `monitor_status`/`shutdown`). The `Shell`'s three existing call sites are unchanged (status quo kept; signatures preserved).

`tests/e2e_flow.rs` (6 tests, all pass; real `Controller` + in-process `ParamBlock` target + fake audio producer + a session-local mapping with a lease-honoring bypass thread standing in for the processing side):
- **intents → params + files**: `SetMacro(Purity, 0.1)` → published snapshot bit-compares against `resolve()` of the engine model (**exact `DspParams` equality**); `SetParam(DenoiseMix, 0.9)` pins (D-007) and lands in `profiles/{id}.json` (`pins` + `overrides` verified), `settings.json` and the boot-params file exist after the 500 ms debounce; `ToggleBypass` flips the model and the published bypass flag.
- **frames → LiveFrame**: a −20 dBFS 1 kHz fake source lights the mid-spectrum band (level > 0.6 within 2 s; peak band in the 12–24 range); the visualizer only runs while "visible" (start/stop proven by `is_visualizing`, test `failures_surface_and_the_visualizer_follows_visibility`).
- **monitor intents**: `ToggleMonitor`/`PickMonitorDevice` drive the `MonitorHost` (recorder in-test; the production `CpalMonitorHost` is the same code the parent uses), persist `monitor_enabled` both ways, and expose the **41.6 ms** latency badge in the `UiModel` (D-026 formula asserted).
- **endpoint selection**: `SelectEndpoint(1)` → new profile created and persisted (`profiles/{mic-b}.json`, `settings.selected_endpoint_id`), params re-resolved for the fresh midpoint profile; out-of-range picks surface a user message.
- **Auto-Tune, full flow**: `StartAutoTune` (fake-APO lease mapping + scripted 1 s/3 s/5 s capture: LCG noise ≈ −51 dBFS, 150 Hz sine −23 dBFS) through the **real calibrator** reaches the Results card; the report satisfies T11's acceptance semantics (**speech RMS + input gain ≈ −18 dBFS within 1.0 dB, simulated peak inside −6…−3 dBFS, denoise ≥ 0.5**); `ApplyCalibration { keep_pins: false }` adopts it into the model, releases the calibration-driven pin, changes the gate threshold, persists the calibration + updated boot params, and closes the card. Wall time was **5.1–5.7 s per run** (non-real-time capture: the runner reads as fast as `push` allows; T16's measured 0.99 s fixture run).
- **reboot persistence**: drop of every `ControllerHandle` flushes; a fresh controller on the same paths restores macro 0.9 / pin + override 123 Hz; `first_run` flips to false; published params equal `resolve()` of the reloaded model (**exact equality**).
- **error surfacing**: every failure path asserted non-silent (missing processing link, vanished mic, `Hear Myself` start failure against a nonexistent device; the parent-side services mirror the same messages).

Unit coverage in `ui_bridge.rs` (6 tests): band normalization (−90 dBFS floor → 0.0, 0 dBFS → 1.0), `meters`-payload parsing, the D-018 badge state matrix (`ApoState` → the four badge variants), `param_key` ⇄ CLI `param_id_by_key` round-trip for **every** SPECS entry, and the D-026 latency formula (10 + 10 + 21.6 = 41.6).

### What runs now, live on this machine (Release/debug smoke runs in this session)

- **Real child, headless**: `tine --ui --snap … --tree …` (the real window host, headless via Rune tooling) renders the full `tine-ui` tree — 83 elements: a live mic picker showing the machine's real endpoint ("Mic: Microphone (AB13X USB Audio) ▾"), the D-018 badge ("APO: Active"), archetype pills, Auto-Tune button, Hear Myself + device picker, A/B switch, the Golden Line canvas with real macro positions from the persisted profile (Purity 0.30 / Body 0.70 / presence 0.50 / leveler 0.50), and the Pro Rack drawer. `target/t21-child.png` + `t21-child.tree.json`.
- **Two-process pipe smoke**: parent `tine.exe` + CLI `status`/`meters`/`macro body 0.7`/`bypass on`/`calibrate`/`monitor on`; then the headless child while the parent served (exit 0). Logged verbatim: `macro body → value 0.699…` applied; **`Hear Myself started endpoint=… latency_ms=41.6`** (the real T16 Monitor on the real mic, measured badge = the D-026 predicted value); `calibrate: started` followed by the surfaced user message **"Auto-Tune needs a live processing link: start Tine's audio routing first (virtual cable, D-018)."** — the no-silent-failures contract working. State left on the machine was reverted (body 0.5, bypass off, monitor off, verified in settings/profile).
- **Window child, live**: a second launch of `tine.exe` while the parent ran forwarded `open` and the parent spawned the real `--ui` child — the real window opened and ran (log: `UI child spawned` → `UI child starting`), then it was terminated with the parent for cleanup.

### Gaps (recorded, precise; see `## Requests`)

1. **Live-child intents not yet expressible through the frozen pipe protocol** (`SelectEndpoint`, `Unpin`/`UnpinAll`, Auto-Tune progress/results/apply): the child logs a warning ("intent not yet routable…"); the engine side is contract-complete in `EngineBridge` and integration-tested. R-20/D-027.
2. **D-018 audio routing is not started by the shell** (T17's own recorded request): the parent still spawns `EngineConfig::shm` with no mapping in production, so live spectrum/telemetry rests at the floor and the parent's Auto-Tune/D-018 flow reports the surfaced message above. The headless bridge covers the full semantics.
3. **`tine.exe` = 21.44 MB** (release, `debug=false` rebuild identical), over the 20 MB budget recorded when the binary was a stub-less placeholder: the real `tine-ui` views + the Auto-Tune offline verification (`AutoCalibrator` links the full `DspPipeline` incl. the `nnnoiseless` weights) — **R-21** requests a T01-owner decision.

### Manual test checklist for the maintainer (the human-timed fresh-profile run I cannot perform)

Prep: rename `%APPDATA%\Tine` aside (fresh profile), build `--release`, keep a timer and (optionally) a screen recorder running.

1. [ ] **First run (target < 2 min, timed)**: launch `tine.exe` → tray appears (< 150 ms reads in the log) → open the window from the tray → the **onboarding overlay** shows (fresh settings file). Steps: pick the mic → the suggested archetypes are pre-marked (skip Install — D-018: the overlay's cable step shows the remedy if no VB-Cable is present) → `Auto-Tune (8s)` → speak normally → the Results card appears. NB: with the pipe-protocol gap (Requests #1), the in-window wizard cannot receive progress yet — run `tine calibrate` from a terminal instead, and confirm the log's runner messages (`auto-tune capture started` … `report-pending` summary), then Apply via `tine set/macro` checks below. Timing ends when Discord's mic meter reacts
 to Auto-Tune's settings.
2. [ ] **Every control**: with the window open, drag each macro node (audio/meters change within ~50 ms), click each archetype pill, toggle A/B with `Space` (**click-free** — the smoother is per-sample), toggle Hear Myself (badge shows ~42 ms), open the Pro drawer and edit one numeric field (clamped inline message on out-of-range input), pin/unpin via the rack (Unpin needs Requests #1).
3. [ ] **Persistence**: change Body to ~0.7 -> `tine status` reflects it -> Quit, relaunch -> same state on reopen (window shows current profile on the first frame).
4. [ ] **Boot/restart resilience**: with Tine autostarted, set a distinctive macro, quit Tine, and confirm state resumes before the UI reopens - per Gap 2 above, real processed audio awaits Requests #2 wiring; the on-disk `endpoints/{id}.json` correctness is covered integration-side today.
5. [ ] **Close the window**: confirm in Task Manager that the `--ui` child exits (GPU memory released; parent CPU < 0.5 %, Working Set ~16 MB per T17's numbers - re-soak with `tine soak`), the visualizer is stopped (no `tine-visualizer` thread in the parent), and a reopen shows the current state settled on the first frame.
6. [ ] **A fresh Windows user profile** replay of (1) after wiring Requests #1-#2 is the acceptance run to record with timings.

### Verification

- `cargo nextest run -p tine-app`: **56 run, 55 passed, 1 skipped** (registry round-trip, unchanged). Full workspace: **269 passed, 5 skipped** (hw tests excluded by design), 11.3 s.
- `cargo build -p tine-app --release`: builds clean; `tine.exe` = **21,440,512 bytes** (Gap 3 / R-21).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean. `cargo fmt --check -p <pkg>` per package (R-19: `--all` deadlocks on `tine-dsp`): **all crates clean**, no orphaned `rustfmt.exe`.

## Requests (for the orchestrator; not edited by this agent)

1. **Pipe protocol extension** (owner: T17-successor / `proto.rs` + `shell.rs`) - additive `PipeRequest` variants + `shell.rs::execute_pipe` arms + a small `status` payload extension so the live child's parked intents become routable (all engine semantics already implemented + integration-tested in `EngineBridge`): `Endpoint { id }` -> `Command::SelectEndpoint` (+ `services` endpoint-keyed restart), `Unpin { key }` / `UnpinAll` -> `Command::Unpin`/`UnpinAll`, `CalibrateQuery`/`CalibrateCancel`/`CalibrateApply { keep_pins }` -> the new `services::autotune_state`/`autotune_cancel` + a report payload (the parent's runner currently runs with `controller: None` because services cannot reach a `ControllerHandle` - the same patch can pass one in), and `status` additions: `params` + `pins` (or a new `Model` request returning them) so the child's Pro Rack no longer resolves from the profile file. ~40 lines additive; see R-20.
2. **D-018 router wiring in the shell** (owner: T17-successor / `shell.rs` + maybe `services.rs` follow-up): T17's own recorded request - the parent should own a `VirtualCableRouter` on an in-process `ParamBlock` and spawn the controller with `EngineConfig::block(...)` (the shm target is unlinked in production). After that, the visualizer's signals switch from the (absent APO) mapping to `InProcessSignals` (`services.rs` is written shape-compatible for this swap), the Auto-Tune bypass lease becomes a controller-side `SetBypass` during the run, and the fresh-profile acceptance run (checklist item 1) can be timed. The tap/telemetry single-reader invariants are already respected by the pump (see D-026/D-027).
3. **`tine.exe` size budget decision** (owner: T01 / root `ci.yml` + `docs/project/09-engineering-standards.md` §7): the 20 MB gate no longer fits the required content (real `tine-ui` + the Auto-Tune offline verification pipeline, which links `DspPipeline` incl. the `nnnoiseless` weights). Either raise the budget to 24 MB or trade away the in-app offline verification. Measured: 21,440,512 bytes; `debug = false` rebuild measures the same, so it is code+data, not symbols. See R-21/D-027.
4. **`ui.rs` deletion + lib-target split** (FYI, cross-owns applied under the view-swap work item): the placeholder view module `crates/tine-app/src/ui.rs` was deleted (its whole content was the placeholder the task replaces); `main.rs` is a thin binary over the new `tine_app` lib target (needed so `tests/e2e_flow.rs` can drive the app's own bridge - a binary cannot be imported by integration tests). Bless `crates/tine-app/Cargo.toml` ([lib] section, `tine-dsp` dep for the calibrator presets, `Win32_System_Memory` windows feature for the fake-APO test mapping, `tempfile` dev-dep) and `Cargo.lock`.
