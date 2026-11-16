# T14 — APO Install / Health / Repair / Uninstall

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | B — Windows / APO |
| Depends on | T13 |
| Unblocks | T21, T23 |
| Owns | `crates/tine-engine/src/apo_admin/**`, `crates/tine-engine/src/win/**` (shared Win32/COM helpers; coordinate with T15, which consumes them) |
| Size | L |

## Goal
Safe, reversible, verifiable management of the APO registration per capture endpoint. Usable from the UI with a single UAC prompt and from the CLI. Detects automatically when Windows removed it.

## Read first
T02 `## Result` (registry recipe, ACL procedure, reload procedure) · [02-architecture.md](../project/02-architecture.md) §8 · R-05, R-06, R-12 · [09-engineering-standards.md](../project/09-engineering-standards.md) §5

## Contract (provides, `tine_engine::apo_admin`)
```rust
pub enum ApoState { Active { heartbeat_age_ms: u32 }, InstalledNotLoaded, RemovedBySystem, NotInstalled, EnhancementsDisabled, Error(ApoError) }
pub struct CaptureEndpoint { pub id: String, pub name: String, pub form_factor: FormFactor, pub suggested: Archetype, pub state: ApoState }
pub fn list_capture_endpoints() -> Result<Vec<CaptureEndpoint>, ApoError>;           // non-elevated
pub fn health(endpoint: &str) -> ApoState;                                            // non-elevated
pub fn request(op: AdminOp) -> Result<AdminOutcome, ApoError>;                        // relaunches elevated helper, waits, returns typed result
pub fn run_elevated_helper(args: &[OsString]) -> ExitCode;                            // entry point used by `tine.exe --elevated-helper`
pub struct DeviceWatcher;  // IMMNotificationClient → channel of DeviceEvent { Added, Removed, DefaultChanged, PropertyChanged(id) }
```

## Steps
- [ ] `win/`: RAII wrappers (`ComInit`, `CoTaskMem`, `PropVariant`, registry key, `HANDLE`), `IMMDeviceEnumerator` helpers, `PKEY_AudioEndpoint_FormFactor` → archetype suggestion (Headset/Headphones → Headset; Microphone built-in → Laptop; USB mic → Condenser or Dynamic, defaulting to Dynamic; let the user change it).
- [ ] Elevated helper:
  - `ShellExecuteExW` with `runas` on the current exe, `--elevated-helper <op> <args> --result-pipe <name>`
  - whitelisted ops: `install`, `uninstall`, `repair`, `uninstall-all`
  - the endpoint id is validated against the live enumeration
  - result returned as JSON over a named pipe whose DACL allows only the calling user's SID
- [ ] Install:
  1. Back up the original FxProperties values to `%ProgramData%\Tine\backup\<endpoint>.json`. Never overwrite an existing backup.
  2. Record any vendor CLSID for wrapping.
  3. Register the CLSID and the APO registration keys (machine-wide, once).
  4. Write the endpoint's FxProperties.
  5. Ensure enhancements are enabled.
  6. Reload the endpoint (procedure from T02).
  7. Verify the heartbeat within 5 s; on failure, roll back automatically and return a typed error.
- [ ] Health: classify each endpoint as `ApoState` from registry + shm heartbeat. `DeviceWatcher` triggers a re-check within 2 s of a property change.
- [ ] Uninstall restores the backup exactly and deletes it only after the restore is verified. `uninstall-all` covers every backed-up endpoint and unregisters the CLSID (the MSI calls it).
- [ ] Security review checklist in `## Result`:
  - argument validation
  - pipe DACL
  - no arbitrary path writes
  - SDDL of the shm and ProgramData folders
  - TOCTOU on backup files

## Acceptance criteria
- Install → verify → uninstall on USB, Realtek and headset endpoints leaves the registry identical to before (`reg export` diff empty).
- Deleting the Tine FxProperties value by hand is detected within 2 s and fixed by `repair`.
- The helper rejects unknown ops, malformed ids, and a pipe connection from another user.
- Install on an endpoint with a vendor SFX keeps the vendor effect audible.
- Non-elevated code paths never write HKLM (proved by a test running non-elevated).

## Verification
```bash
cargo nextest run -p tine-engine apo_admin
tine apo status
tine apo install --endpoint "<id>"
tine apo uninstall --endpoint "<id>"
```
(CLI wiring lands in T17. Until then, run the same ops through the `apo_admin` example binary in this task.)
