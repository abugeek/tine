# T13 — `tine_apo.dll` Production APO

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | B — Windows / APO |
| Depends on | T02 (GO), T05 |
| Unblocks | T14, T15 (shm present), T21 |
| Owns | `crates/tine-apo/**` |
| Size | L |

## Goal
Turn the T02 spike into the production APO. It hosts `DspPipeline` inside `audiodg.exe`, owns the shared-memory block, starts from persisted params, wraps any vendor APO, and can never take down system audio. It's built against the T05 skeleton, so real DSP arrives automatically as lane A lands.

## Read first
T02 `## Result` (interface set, slot, formats, instance model) · [02-architecture.md](../project/02-architecture.md) §1, §3–8 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3, §5 · R-03, R-05, R-07, R-08, R-12

## Contract
- **Provides:** COM class `CLSID_TineApo` (fixed GUID recorded in `tine-protocol::ids`); shm mapping `Global\Tine.v1.<endpoint>` with `ShmLayout`; boot-param file format `%ProgramData%\Tine\endpoints\<sanitized endpoint>.json` (`EndpointProfile` + resolved `DspParams`) that it reads.
- **Consumes:** `DspPipeline`, `ShmLayout`, `ParamBlock`, `Triple`, `SpscRing`, `DspParams::DEFAULT` (neutral fallback; no dependency on T10).

## Steps
- [ ] `src/com/`: class factory, `DllGetClassObject`, `DllCanUnloadNow` (object + lock counts), interface impls from T02. Every method body runs inside `ffi_guard(|| ...)` (`catch_unwind` → `E_FAIL`, and latches pass-through).
- [ ] `Initialize` (non-RT):
  - parse `APOInitSystemEffects2/3` for the endpoint id and the device's property store
  - create or open the mapping with an explicit DACL (SDDL: `D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GRGW;;;IU)`; adjust per T02 findings)
  - claim the telemetry writer role through `writer_claim` (first instance wins; others write params-only state)
- [ ] `IsInputFormatSupported` / `LockForProcess` (non-RT):
  - accept f32, and PCM 16/24/32 (convert in and out), any channel count up to 8, rates 16–192 kHz (RNNoise path resamples per D-006)
  - allocate `DspPipeline::new(fs, max_frames)` and scratch buffers
  - load boot params (`std::fs` is fine here); if missing or invalid, fall back to the neutral `DspParams::DEFAULT` (safe: gentle processing, never surprising)
- [ ] `APOProcess` (RT):
  - `DenormalGuard`; heartbeat += 1
  - read `params` if the generation changed, else keep the cached copy (clamp everything, D-005)
  - honour `bypass_lease_until_ms`: when the Auto-Tune lease expires without renewal, bypass turns off automatically. Protects against `tine.exe` dying mid-calibration.
  - `BUFFER_SILENT` → process zeros (keeps filters and detectors consistent) and mark output silent if still silent
  - downmix → `process` → write mono to every output channel (with format conversion)
  - publish telemetry through `ShmMeterSink`, tap through `ShmTapSink` (writer instance only)
- [ ] Vendor APO wrapping per T02 design: instantiate the recorded original CLSID as a child in `Initialize`; forward format negotiation; process child first, then Tine. Any child failure → skip the child, keep Tine running, set a flag.
- [ ] `GetLatency` → `latency_samples()` as `HNSTIME`.
- [ ] `src/test_host.rs` (`cfg(test)`): instantiate the COM object in-process and drive `Initialize` / `LockForProcess` / `APOProcess` with synthetic `APO_CONNECTION_PROPERTY` buffers.
- [ ] Fuzzing: `fuzz/` target (cargo-fuzz, or a proptest fallback on Windows) over format × frame count × flags × random params × random shm garbage. Pass criteria: no panic, no NaN out, no out-of-bounds.
- [ ] Debug-only log file `%ProgramData%\Tine\logs\apo.log`, written only from non-RT methods.

## Acceptance criteria
- Test host:
  - output equals the `DspPipeline` reference for f32 mono, f32 stereo, i16 stereo and i24 4-channel at 44.1 and 48 kHz
  - zero allocations in `APOProcess`
  - a forced panic and a forced NaN both give pass-through, with the flag set
- Fuzz: 1 h with no findings.
- Live (installed with the T02 spike CLI until T14 lands):
  - Discord hears processed audio
  - writing params from a test client is audible within 50 ms
  - with `tine.exe` absent, boot params apply after reboot
- `APOProcess` CPU < 0.3 % of a core at 48 kHz (`QueryThreadCycleTime` sampling in a debug-stats build, or WPA). Record the number.
- `tine_apo.dll` < 2 MB.

## Verification
```bash
cargo nextest run -p tine-apo
cargo build -p tine-apo --release
```

## Out of scope
Registry install/uninstall UX (T14), UI.

## Pitfalls
- Several instances can coexist per endpoint (streams and modes), and all map the same shm. Only the claimed writer publishes telemetry and tap. Every instance reads params.
- Release the writer claim in `UnlockForProcess`/`Release`, and recover from a stale claim through heartbeat age.
- Never block in `APOProcess`, not even on a spin wait.
