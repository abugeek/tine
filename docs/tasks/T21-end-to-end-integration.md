# T21 — End-to-End Integration in `tine.exe`

| Field | Value |
| :--- | :--- |
| Status | in-progress (t21-worker, Mistral Vibe) |
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
- [ ] `ui_bridge.rs`:
  - `IntentSink` implementation maps each `Intent` to `ControllerHandle::send`, `AutoTuneRunner`, `Monitor` or `apo_admin::request` (the latter on a worker thread, with the result posted back through `AppHandle::update`)
  - engine `Event::ModelChanged` → `handle.update(|s, _| s.model = map(model))`
  - `Visualizer` frames → `handle.update` at ≤ 60 Hz, only while visible (coalesce: an update in flight drops the older frame)
- [ ] Endpoint list + `DeviceWatcher` events → model updates; the APO badge comes from `health()`.
- [ ] First-run detection (no settings file) → onboarding flow.
- [ ] Error surfacing: every engine and admin error maps to a user message + log entry; no silent failures.
- [ ] Integration tests:
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
