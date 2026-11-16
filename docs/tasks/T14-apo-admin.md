# T14 — Virtual Cable Discovery, Device Watcher & Endpoint Admin (`D-018`)

> **Replanned from APO registry admin per [D-018](../project/06-decisions.md)** after T02 returned **NO-GO** on stock Windows 11.

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: T14-worker) |
| Lane | B — Virtual Cable Routing (was Windows / APO) |
| Depends on | T12 |
| Unblocks | T15, T21, T23 |
| Owns | `crates/tine-engine/src/apo_admin.rs`, `crates/tine-engine/src/win.rs` |
| Size | M |

## Goal
Provide Windows endpoint enumeration, form-factor archetype suggestion, `DeviceWatcher` hot-plug notifications, and virtual-audio-cable auto-discovery (`CABLE Input (VB-Audio Virtual Cable)` / `Virtual Audio Driver`) with zero UAC prompts required.

## Read first
- [06-decisions.md](../project/06-decisions.md) (D-018) · [02-architecture.md](../project/02-architecture.md) §8 · [09-engineering-standards.md](../project/09-engineering-standards.md) §5

## Contract (provides, `tine_engine::apo_admin` & `tine_engine::win`)
```rust
pub enum ApoState { Active { heartbeat_age_ms: u32 }, InstalledNotLoaded, RemovedBySystem, NotInstalled, EnhancementsDisabled, Error(ApoError) }
pub struct CaptureEndpoint { pub id: String, pub name: String, pub form_factor: FormFactor, pub suggested: Archetype, pub state: ApoState }
pub struct VirtualSinkInfo { pub id: String, pub name: String, pub is_available: bool }
pub fn list_capture_endpoints() -> Result<Vec<CaptureEndpoint>, ApoError>;
pub fn find_virtual_cable_sink() -> Result<Option<VirtualSinkInfo>, ApoError>;
pub fn health(endpoint: &str) -> ApoState; // reflects virtual-cable sink availability & active routing state
pub struct DeviceWatcher; // IMMNotificationClient -> channel of DeviceEvent { Added, Removed, DefaultChanged, PropertyChanged(id) }
```

## Steps
- [ ] `win.rs`: RAII wrappers (`ComInit`, `CoTaskMem`, `PropVariant`, `HANDLE`), `IMMDeviceEnumerator` helpers, `PKEY_AudioEndpoint_FormFactor` -> `Archetype` suggestion (Headset/Headphones -> Headset; Microphone built-in -> Laptop; USB mic -> Dynamic/Condenser).
- [ ] `apo_admin.rs`: implement `list_capture_endpoints()`, `find_virtual_cable_sink()` (scanning WASAPI render endpoints for `CABLE Input` / `VB-Audio` / `Virtual Audio`), `health()`, and `DeviceWatcher`.
- [ ] Ensure non-elevated execution (no HKLM writes, no UAC prompts).

## Acceptance criteria
- Enumerates live capture endpoints with accurate form-factor -> `Archetype` suggestions.
- Detects `CABLE Input (VB-Audio Virtual Cable)` when installed, and cleanly reports `NotInstalled` when absent.
- `DeviceWatcher` delivers device add/remove/default-change events within 2 s.

## Verification
```bash
cargo test -p tine-engine apo_admin
```
