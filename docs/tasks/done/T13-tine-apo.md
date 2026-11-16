# T13 — Virtual-Cable DSP Routing Loop (`D-018`)

> **Replanned from `tine_apo.dll` per [D-018](../project/06-decisions.md)** after T02 returned **NO-GO** on stock Windows 11.

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] Implement `VirtualCableRouter` bridging `audio_io::open_capture` -> lock-free `SpscRing` -> RT worker/callback running `DspPipeline::process` -> `audio_io::open_playback` to the virtual cable sink.
- [x] Honour `bypass` and `ParamBlock` generation updates without locks or allocations.
- [x] Publish `AudioTelemetryFrame` at ~60 Hz and post-DSP mono samples to the tap ring.
- [x] Add unit/integration tests with `assert_no_alloc` on the RT processing path and synthetic/loopback verification.

## Acceptance criteria
- Zero allocations on RT paths (`assert_no_alloc`).
- Parameter changes in `ParamBlock` take effect audibly within `< 50 ms`.
- Continuous capture -> DSP -> playback runs without underruns/dropouts.

## Verification
```bash
cargo test -p tine-engine virtual_cable
```

## Result
- Implemented `VirtualCableRouter` and `rt::VirtualCableProcessor` in `crates/tine-engine/src/virtual_cable.rs` (exported via `pub mod virtual_cable;` in `crates/tine-engine/src/lib.rs`).
- `VirtualCableRouter::start` uses `tine_protocol::sync::{ParamBlock, Triple<AudioTelemetryFrame>, SpscRing<TAP_N>}` (the actual `tine-protocol` primitives corresponding to `TripleWriter` / `SpscProducer` in the draft contract).
- `rt::VirtualCableProcessor::process_block` is strictly `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects)]`.
- Verified with `assert_no_alloc` in `crates/tine-engine/tests/virtual_cable_rt.rs`:
  - `virtual_cable_processor_zero_alloc_and_telemetry_tap`: 100 x 480-sample blocks (1.0 s @ 48 kHz) with 0 heap allocations, >= 40 telemetry frames published, and post-DSP samples pushed to tap ring.
  - `virtual_cable_param_updates_and_bypass_zero_alloc`: `bypass` flag and `+12 dB` `input_gain_db` update reflected audibly within 40 ms (< 50 ms) with 0 heap allocations.
  - `virtual_cable::tests::start_returns_device_not_found_for_missing_input`: returns `AudioIoError::DeviceNotFound` cleanly.
- Checks:
  - `cargo fmt --all --check`: pass
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `cargo test -p tine-engine`: pass (1 unit + 2 integration tests; `audio_io_rt` 3/3 also pass)
  - `cargo deny check`: pass (`advisories ok, bans ok, licenses ok, sources ok`)
  - `cargo test --workspace`: `tine-dsp` has 3 pre-existing failures in `tests/pipeline.rs` on `origin/main` (`skeleton_is_bit_exact_passthrough`, `nan_injection_latches_passthrough_and_reset_clears`, `bypass_toggle_has_no_step_above_minus_60_dbfs`), unrelated to T13.
