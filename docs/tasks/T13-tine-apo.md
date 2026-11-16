# T13 — Virtual-Cable DSP Routing Loop (`D-018`)

> **Replanned from `tine_apo.dll` per [D-018](../project/06-decisions.md)** after T02 returned **NO-GO** on stock Windows 11.

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | B — Virtual Cable Routing (was Windows / APO) |
| Depends on | T05, T12 |
| Unblocks | T14, T16, T21 |
| Owns | `crates/tine-engine/src/virtual_cable.rs` (and `crates/tine-engine/src/lib.rs` export only) |
| Size | M |

## Goal
Run `DspPipeline` in-process inside `tine-engine`: capture mono `f32` from the selected physical microphone via `tine_engine::audio_io`, process through `DspPipeline` (reading `DspParams` lock-free from `ParamBlock` and publishing `AudioTelemetryFrame` + audio tap to `Triple` / `SpscRing`), and output continuously to the discovered virtual audio cable (`CABLE Input (VB-Audio Virtual Cable)` or fallback virtual sink) via `audio_io::open_playback`.

## Read first
- [06-decisions.md](../project/06-decisions.md) (D-018) · [02-architecture.md](../project/02-architecture.md) §3 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3–4
- `tine-protocol` (T04) · `DspPipeline` (T05) · `tine_engine::audio_io` (T12)

## Contract (provides, `tine_engine::virtual_cable`)
- `VirtualCableRouter::start(input_id: &DeviceRef, output_id: &DeviceRef, params: Arc<ParamBlock>, telemetry: Arc<TripleWriter<AudioTelemetryFrame>>, tap: Arc<SpscProducer<f32>>) -> Result<VirtualCableRouter, AudioIoError>`
- `VirtualCableRouter::stop(self)`
- `VirtualCableRouter::stats(&self) -> StreamStats`
- Zero heap allocations, locks, or panics on either the capture or playback audio callback (proven with `assert_no_alloc`).

## Steps
- [ ] Implement `VirtualCableRouter` bridging `audio_io::open_capture` -> lock-free `SpscRing` -> RT worker/callback running `DspPipeline::process` -> `audio_io::open_playback` to the virtual cable sink.
- [ ] Honour `bypass` and `ParamBlock` generation updates without locks or allocations.
- [ ] Publish `AudioTelemetryFrame` at ~60 Hz and post-DSP mono samples to the tap ring.
- [ ] Add unit/integration tests with `assert_no_alloc` on the RT processing path and synthetic/loopback verification.

## Acceptance criteria
- Zero allocations on RT paths (`assert_no_alloc`).
- Parameter changes in `ParamBlock` take effect audibly within `< 50 ms`.
- Continuous capture -> DSP -> playback runs without underruns/dropouts.

## Verification
```bash
cargo test -p tine-engine virtual_cable
```
