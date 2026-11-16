# T16 — Engine Audio Services: Spectrum, Auto-Tune Runner, Hear Myself

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | C — Engine / App |
| Depends on | T11, T15 |
| Unblocks | T17, T20 (wizard data), T21 |
| Owns | `crates/tine-engine/src/{spectrum.rs, visualizer.rs, autotune.rs, monitor.rs}`, `crates/tine-engine/tests/{spectrum.rs, autotune.rs}` |
| Size | M |

## Goal
The three audio-facing services `tine.exe` runs next to the controller. Each one costs nothing while unused.

## Read first
[02-architecture.md](../project/02-architecture.md) §4, §7 · [05-ui-ux.md](../project/05-ui-ux.md) · `audio_io` (T12) · `AutoCalibrator` (T11) · `ShmClient` (T15)

## Contract (provides)
```rust
pub struct SpectrumAnalyzer;  // 2048-pt realfft, Hann, 50 % overlap, 32 log bands 20 Hz–20 kHz, attack 10 ms / decay 300 ms ballistics
impl SpectrumAnalyzer { pub fn new(fs: f32) -> Self; pub fn feed(&mut self, s: &[f32]); pub fn bands(&self) -> &[f32; 32]; }
pub struct UiFrame { pub telemetry: AudioTelemetryFrame, pub spectrum: [f32; 32], pub heartbeat_ok: bool }
pub struct Visualizer;  // runs only while enabled: drains tap at 60 Hz, publishes UiFrame through Triple<UiFrame>
pub struct AutoTuneRunner;  // start(endpoint) -> progress via channel; acquires bypass lease, captures raw mic, feeds AutoCalibrator
pub struct Monitor;  // "Hear Myself": capture processed endpoint → ring → playback device; start/stop; stats
```

## Steps
- [ ] `SpectrumAnalyzer`:
  - window and twiddles precomputed
  - band edges precomputed as bin ranges with fractional weights
  - output in dBFS with −90 floor
  - a band covering < 1 bin interpolates from its neighbours
- [ ] `Visualizer`:
  - dedicated thread that exists only while the window is visible (started/stopped by T17)
  - 60 Hz tick (waitable timer, high resolution), drains `tap` via `ShmClient`, reads telemetry, publishes `UiFrame`
  - stopping joins the thread; there is no idle wakeup
- [ ] `AutoTuneRunner`:
  1. Set `bypass_lease_until_ms = now + 15 s` (renew every 5 s) and wait for one telemetry frame that confirms bypass.
  2. Open the endpoint with `audio_io::open_capture` (shared mode, so the APO output = raw while bypassed).
  3. Feed `AutoCalibrator::push` from a non-RT thread through the ring; report `CalProgress` at 30 Hz.
  4. On finish/cancel/error, clear the lease.
  5. Return `CalibrationReport` or `CalFailure`.
- [ ] `Monitor`:
  - capture the processed endpoint (shared mode, so the APO's output) → ring → `open_playback` to the chosen device, with drift correction
  - latency badge value = measured capture + playback buffer + DSP latency
  - follows its own toggle; restarts on device change; a failure reports an error and doesn't panic
- [ ] Tests:
  - spectrum: a sine lands in the right band within ±1 band, pink noise is flat within ±3 dB
  - Auto-Tune runner against a fake capture source (feed fixture audio)
  - lease renewal and expiry

## Acceptance criteria
- Visualizer: < 1 % CPU while running; 0 threads and 0 CPU when stopped.
- Auto-Tune on a real mic completes in ≈ 9 s and returns a report.
- Killing `tine.exe` mid-run: the APO leaves bypass within 15 s.
- Monitor: 30 min with 0 underruns after warm-up; the reported latency is recorded.

## Verification
```bash
cargo nextest run -p tine-engine spectrum autotune
```
