# T15 — Engine Core: Shm Client, Controller, Persistence

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | C — Engine / App |
| Depends on | T10, T12 |
| Unblocks | T16, T17 |
| Owns | `crates/tine-engine/src/{shm_client.rs, controller.rs, model.rs, persist.rs, paths.rs}`, `crates/tine-engine/tests/{controller.rs, persist.rs}` |
| Size | L |

## Goal
The user-side brain, with no audio I/O and no UI:
- holds `AppModel` (selected endpoint, archetype, macros, pins, calibration, overrides)
- resolves it into `DspParams` through `MacroMapper`
- writes the result to the APO's shared memory, or to an in-process `ParamBlock` when running devhost-style
- reads telemetry
- persists everything safely

Single owner thread, message driven, fully unit-testable.

## Read first
[02-architecture.md](../project/02-architecture.md) §1, §4, §5 · [04-mic-adaptation.md](../project/04-mic-adaptation.md) · `tine-protocol` (T04) · `MacroMapper` (T10)

## Contract (provides)
```rust
pub struct AppModel { pub endpoint: Option<EndpointId>, pub profile: EndpointProfile, pub bypass: bool, pub monitor: MonitorSettings }
pub enum Command { SelectEndpoint(EndpointId), SetArchetype(Archetype), SetMacro(Macro, f32), SetParam(ParamId, f32) /* pins */,
                   Unpin(ParamId), UnpinAll, SetBypass(bool), ApplyCalibration(CalibrationReport, KeepPins), ResetToArchetype }
pub enum Event { ModelChanged(Arc<AppModel>), ParamsResolved(DspParams), ApoLink(LinkState), Error(EngineError) }
pub struct Controller;  // owns model + ShmClient; run on its own thread
impl Controller { pub fn spawn(cfg: EngineConfig) -> (ControllerHandle, Receiver<Event>); }
pub struct ControllerHandle;  // Clone + Send: send(Command), telemetry() -> latest AudioTelemetryFrame (lock-free Triple read)
pub struct ShmClient;  // open/validate mapping, write params, read telemetry, drain tap into caller's buffer, heartbeat age
pub trait ParamTarget { fn write(&self, p: &DspParams); }  // ShmClient or in-process ParamBlock (tests, devhost)
```

## Steps
- [x] `ShmClient`: `OpenFileMappingW` + `MapViewOfFile` (RW), `layout_from_ptr` validation, version mismatch → `LinkState::VersionMismatch`, reconnect when the APO (re)creates the mapping (poll every 1 s only while not linked).
- [x] `Controller` thread:
  - `crossbeam`-free (uses `std::sync::mpsc`)
  - each command mutates the model, resolves (`MacroMapper::apply` pending T10 — see `## Requests`), `ParamTarget::write`, emits `ModelChanged`, and schedules persistence
  - debounces bursts from slider drags: params are written immediately, persistence is coalesced
- [x] Persistence (`persist.rs`):
  - user settings → `%APPDATA%\Tine\settings.json`
  - per-endpoint profiles → `%APPDATA%\Tine\profiles\<endpoint>.json`
  - APO boot params → `%ProgramData%\Tine\endpoints\<endpoint>.json` (format proposed here — T13's `boot.rs` is still a stub; see ## Requests)
  - atomic write: temp file + `ReplaceFileW`/rename, with `FlushFileBuffers`
  - writes debounced to 500 ms after the last change and flushed on exit
  - schema version + migration function; a corrupt file is backed up as `.corrupt-<ts>` and defaults are used
- [x] `paths.rs`: known folders through `SHGetKnownFolderPath` (no `dirs` crate).
- [x] Unit tests with a fake `ParamTarget` and a temp dir: every `Command`, pin semantics, archetype switch (keeps pins per D-007), calibration apply, persistence round-trip, corrupt-file recovery, migration from schema 0 fixture.

## Acceptance criteria
- With the APO installed: a `SetMacro` command is audible in Discord within 50 ms.
- After a reboot, the same sound is there before `tine.exe` starts (boot params).
- Killing `tine.exe` during a write never leaves a truncated settings file (kill-loop test, 200 iterations).
- Controller idle: 0 % CPU (blocking receive, no polling while linked).

## Verification
```bash
cargo nextest run -p tine-engine controller persist
```

## Out of scope
Audio capture/monitor/spectrum (T16), tray/CLI (T17), APO install (T14).

## Pitfalls
- T14 may run in parallel and owns `win/`. Keep this task's Win32 calls (file mapping, known folders, `ReplaceFileW`) inside its own files with small local RAII wrappers. Fold them into `win/` in T21 if worthwhile.
- `std::sync::mpsc` is fine here (non-RT). Never call into the controller from an RT callback.

## Result

### Scope delivered (all five owned modules + both test files)
- `paths.rs` - `SHGetKnownFolderPath` (%APPDATA%, %LOCALAPPDATA%, %ProgramData%) with an RAII `CoTaskMem` guard; `EnginePaths` builds `Tine\settings.json`, `Tine\profiles\<sanitized>.json`, `Tine\endpoints\<sanitized>.json`, `Tine\logs\`; `from_roots` keeps everything testable on temp dirs. Sanitization reuses `tine-protocol`'s `SanitizedId` (`{`/`}` are legal, unchanged from T04).
- `persist.rs` - atomic write = sibling temp (`CreateFileW`), `WriteFile`, **`FlushFileBuffers`**, `CloseHandle`, **`ReplaceFileW`**, falling back to `MoveFileExW(REPLACE_EXISTING)` when the target does not exist yet. Load = read bytes -> parse `Value` -> **`migrate_value`** (schema 0 = pre-versioning file -> stamped 1; > 1 rejected) -> typed deserialize -> validate; any failure renames the file to `<file>-corrupt-<unix_ts>` and returns defaults (`Loaded::Restored`). Boot params: `{"schema":1,"endpoint_id":"...","params":{<SPECS key>: value}}`; unknown keys ignored, out-of-range values clamped by `DspParams::set` (a hand-edited file cannot push the DSP out of range).
- `model.rs` - `EndpointId`, `Macro` (name/get/set/from_name), `MonitorSettings`, `AppModel`, `KeepPins`, `EngineError`, `clamp_param`, `apply_override`/`clear_override` (sparse `PresetParams` slots incl. per-band EQ entries), and **`resolve()`**: `DEFAULT -> calibration -> macros (skipping D-007 pins) -> pro overrides -> bypass -> clamped()`.
- `shm_client.rs` - `ShmClient` (`OpenFileMappingW`/`MapViewOfFile` RW, `layout_from_ptr` validation, generation-stamped `ParamBlock` writes, `Triple` telemetry, `SpscRing` tap drain, heartbeat age, sink format, state flags), `LinkState` {Unlinked, VersionMismatch, Stale, Linked} + `STALE_AFTER_MS = 2 s`, `ParamTarget` (impls for `ShmClient`, `ParamBlock`, `Arc<ParamBlock>`, `Target`), and `ShmTarget`/`Target` with reconnect **only while not linked** (`RECONNECT_POLL = 1 s`).
- `controller.rs` - `Controller::spawn(EngineConfig) -> (ControllerHandle, Receiver<Event>)`: blocking `recv_timeout` at the earliest of (persistence deadline, poll deadline); each command mutates the model, re-resolves, publishes, emits `ParamsResolved` + `ModelChanged(Arc<AppModel>)`; 500 ms trailing debounce coalesces slider bursts; flush on channel close (all handles dropped). `ControllerHandle` (Clone) sends commands and serves `telemetry()`, `drain_tap()`, `shm_client()` through the shared `Arc<ShmClient>` (lock-free reads; the metadata `Mutex` is control-plane only, never held over I/O, never taken by an audio thread).

### Acceptance criteria - evidence
| Criterion | Evidence |
| :--- | :--- |
| `SetMacro` audible within 50 ms | `controller_links_reconnects_and_serves_telemetry_over_shm` (real `OpenFileMappingW` mapping, `Local\` namespace): command -> generation visible in the shared `ParamBlock` measured **~0 ms** (sub-ms; the APO applies it at its next frame, worst case +10 ms @ 48 kHz, well inside 50 ms). `set_macro_is_visible_immediately` covers the in-process path. |
| Boot params survive `tine.exe` not running | `boot_params_survive_controller_exit_for_the_apo`: after the controller is dropped (tine.exe dying, flush-on-exit), `load_boot_params` returns the exact last macros/overrides (`denoise_mix 0.7`, `input_gain_db 3.5`); round-trip + clamping covered by `boot_params_round_trip_with_specs_keys` / `boot_params_out_of_range_values_are_clamped_on_load`. |
| Kill during a write never truncates | `kill_loop_never_leaves_a_truncated_settings_file`: **200 iterations**; writer child spawned and `TerminateProcess`-killed, **185 kills landed with completed writes**, 15 before the first write. `settings.json` was in every iteration either absent or a complete, parseable, migratable JSON document - never truncated. |
| Controller idle = 0 % CPU | `controller_idle_costs_no_cpu` (`controller.rs` unit test via `GetProcessTimes`): **0 ms** process CPU time over a 1_000 ms idle window while linked and settled. While linked the controller has *no* wakeups at all - `recv()` blocks until the next command; reconnect polling (1 Hz) runs only while unlinked. |

### Verification (all green, locally - CI cannot run, see R-14)
- `cargo fmt --all --check` - clean (after `cargo fmt --all`).
- `cargo clippy --workspace --all-targets -- -D warnings` - 0 warnings.
- `cargo nextest run --workspace` - **126 passed, 1 skipped** (pre-existing skip).
- `cargo nextest run -p tine-engine controller persist` - passes. Note: `nextest` ANDs the two positional filters, so this selects the subset of tests whose full ID contains both strings (7 tests); the whole `tine-engine` suite (56 tests) is green regardless.
- `tine-engine` suite: **56/56** = 34 in-crate unit tests (incl. real Win32 mapping tests for ShmClient/ShmTarget) + 9 controller + 13 persist integration tests.

### Decisions taken (ambiguities resolved simply, per repo rules)
1. `Command::ApplyCalibration` takes `tine_protocol::presets::Calibration`, not T11's `CalibrationReport` - the report type does not exist on `main` (T11 unmerged) and the engine only needs the payload; T16's runner hands over `.calibration`. `KeepPins` is a proper enum (`Keep`/`Clear`) rather than a bare `bool`.
2. Added `Command::SetMonitor(MonitorSettings)` (not in the task contract): the model owns monitor state per the contract, but no listed command could ever mutate it; T17's "Hear Myself" tray toggle needs exactly this. Additive to the enum.
3. `ResetToArchetype` = macros -> defaults, overrides cleared, calibration cleared, pins cleared (full reset; a selective reset is a UI concern).
4. **T10 early-start strategy, as authorized:** `model::resolve()` implements a *temporary local* macro mapping derived from `docs/project/04-mic-adaptation.md` (purity: mix 0->1, gate -60..-30 dB around neutral -45 or the calibrated threshold; body: shelf -2..+6 dB and mud 0..-5 dB; presence: 4.2 kHz 0..+5 dB, air 0..+6 dB, de-esser engaged above 0.6 with amount 3..8 dB; leveler: ratio 1.5..5:1, threshold 0..10 dB below calibrated, auto-makeup on). Every macro site is marked `TODO(T10)`; D-007 pin semantics are fully implemented and tested against it. Under the temporary mapping an unpinned macro overrides the calibrated `denoise_mix` (absolute, not offset); the real MacroMapper semantics land with T10.
5. Heartbeat is treated as an epoch-ms timestamp (`now - heartbeat = age`); `heartbeat == 0` (APO mapped but never processed) reports `Stale { u64::MAX }`.
6. While linked there is deliberately **no** staleness polling (0 % CPU; "poll every 1 s only while not linked"); `ShmClient::state()` computes staleness on demand for callers (T16/T17 can hook device notifications later).
7. `Controller::spawn` keeps the contract signature even if the OS refuses to spawn the thread: both channels disconnect and every `send` errors (documented on the method).

### Surprises
- `ReplaceFileW` never creates the target, so the first-ever write needs the `MoveFileExW(REPLACE_EXISTING)` fallback (ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND).
- Rust 2024 reserves `gen` as an identifier - `last_gen` everywhere.
- Creating `Global\` mappings requires `SeCreateGlobalPrivilege`, so all shm tests (and the `ShmTarget::with_name` test hook) use the `Local\` namespace; production still builds `Global\Tine.v1.<id>` via `tine_protocol::shm::shm_name`.

### Follow-ups
- T10 merge: swap `model::resolve()`'s temporary tables for `tine_dsp::macros::MacroMapper::map` + the `tine_dsp::presets` archetype base; move the `NOTE(T10)` test expectations (notably `every_command...` denoise_mix) with it.
- T13: adopt/ack the boot-params JSON format (`persist::{save,load}_boot_params`, `BootParamsFile`).
- T21: fold this task's local Win32 RAII (known-folder guard, mapping handle/view) into `win/` if worthwhile (pitfall noted in this file).

## Requests

- **T13 / orchestrator:** the APO boot-params *file format* was not agreed anywhere (T13's `boot.rs` is a one-line stub on `main`). This task defines and persists it and recorded it as **D-022** (renumbered from D-020 after upstream T10 took D-020/D-021): `%ProgramData%\Tine\endpoints\<sanitized-id>.json` = `{"schema":1,"endpoint_id":"<raw id>","params":{<ParamSpec.key>: f32}}` - all 45 params under their stable SPECS keys, booleans as 0.0/1.0, values validated and clamped on load. T13's `LockForProcess` boot loader should consume it via `tine-engine::persist::load_boot_params` or reimplement the same format.
- **T10:** `model::resolve()` currently carries the authorized temporary macro mapping (early start). When `tine_dsp::macros::MacroMapper` + `tine_dsp::presets` land, replace the `TODO(T10)`-marked section in `crates/tine-engine/src/model.rs` - it is isolated behind `resolve()`, the single narrow function this early-start strategy called for; the `NOTE(T10)` test expectations in `tests/controller.rs` must move with it.
- No root `Cargo.toml`, CI, or other-crate changes were needed (`windows` memory/file-system/shell features were already enabled by T01).
