# T15 — Engine Core: Shm Client, Controller, Persistence

| Field | Value |
| :--- | :--- |
| Status | todo |
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
- [ ] `ShmClient`: `OpenFileMappingW` + `MapViewOfFile` (RW), `layout_from_ptr` validation, version mismatch → `LinkState::VersionMismatch`, reconnect when the APO (re)creates the mapping (poll every 1 s only while not linked).
- [ ] `Controller` thread:
  - `crossbeam`-free (use `std::sync::mpsc`)
  - each command mutates the model, then calls `MacroMapper::apply`, `ParamTarget::write`, emits `ModelChanged`, and schedules persistence
  - debounces bursts from slider drags: params are written immediately, persistence is coalesced
- [ ] Persistence (`persist.rs`):
  - user settings → `%APPDATA%\Tine\settings.json`
  - per-endpoint profiles → `%APPDATA%\Tine\profiles\<endpoint>.json`
  - APO boot params → `%ProgramData%\Tine\endpoints\<endpoint>.json` (format agreed with T13)
  - atomic write: temp file + `ReplaceFileW`/rename, with `FlushFileBuffers`
  - writes debounced to 500 ms after the last change and flushed on exit
  - schema version + migration function; a corrupt file is backed up as `.corrupt-<ts>` and defaults are used
- [ ] `paths.rs`: known folders through `SHGetKnownFolderPath` (no `dirs` crate).
- [ ] Unit tests with a fake `ParamTarget` and a temp dir: every `Command`, pin semantics, archetype switch (keeps pins per D-007), calibration apply, persistence round-trip, corrupt-file recovery, migration from schema 0 fixture.

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
