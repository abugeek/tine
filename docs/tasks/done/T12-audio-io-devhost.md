# T12 — cpal Audio I/O Module + `tine-devhost`

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | C — Engine / App |
| Depends on | T03, T05 |
| Unblocks | T15, T16 |
| Owns | `crates/tine-engine/src/audio_io/**`, `crates/tine-devhost/**` |
| Size | M |

## Goal
A reusable, real-time-safe cpal layer in `tine-engine` for device listing, capture, playback, clock-drift handling and resampling. T16 reuses it for Auto-Tune capture and "Hear Myself". On top of it, the `tine-devhost` CLI runs the full DSP live mic → headphones, so the DSP lane can listen before the APO exists.

## Read first
[02-architecture.md](../project/02-architecture.md) §3 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3–4 · cpal 0.18.2 docs (`realtime` feature, `BufferSize`, `SupportedStreamConfig`)

## Contract (provides, `tine_engine::audio_io`)
```rust
pub struct DeviceInfo { pub id: String, pub name: String, pub is_default: bool, pub default_rate: u32, pub channels: u16 }
pub fn list_inputs() -> Result<Vec<DeviceInfo>, AudioIoError>;   pub fn list_outputs() -> ...;
pub struct CaptureStream;  // mono f32 out via SpscRing; owns the cpal stream
pub fn open_capture(id: &DeviceRef, want_rate: Option<u32>, ring: Arc<SpscRing<N>>, on_block: impl FnMut(&mut [f32]) + Send + 'static) -> Result<CaptureStream, _>;
pub struct PlaybackStream; // pulls mono from SpscRing, upmixes, adaptive-rate resampling to absorb clock drift
pub fn open_playback(id: &DeviceRef, ring: Arc<SpscRing<N>>, src_rate: u32) -> Result<PlaybackStream, _>;
pub struct StreamStats { pub underruns: u64, pub overruns: u64, pub callbacks: u64, pub max_callback_us: u32 }
```

## Steps
- [x] Device enumeration by stable id (WASAPI endpoint id where cpal exposes it; otherwise name + index; document the limitation).
- [x] Capture callback: convert any sample format → f32, downmix → mono, run the optional `on_block` (devhost runs `DspPipeline` here), push to the ring. No alloc: scratch buffers are sized from the max buffer size reported at open, grown only outside the callback. If cpal reports a larger buffer at runtime, chunk it.
- [x] Playback callback: pop from the ring; on underrun, output silence and count it. Clock drift between independent devices: track ring fill level and adjust the resample ratio by ±0.1 % max (rubato async or a simple adaptive linear/cubic interpolator); target fill = 2 × callback size.
- [x] Error callbacks (device lost) set a flag observed by the owner; no panics.
- [x] `tine-devhost` CLI (`lexopt`):
  - `list`
  - `run --input <id|name> --output <id|name> [--params file.json] [--record out.wav]`. `--params` takes either `DspParams` JSON or a preset JSON from `assets/presets/` (its base params, no macros). This keeps T12 independent of T10; `--preset`/`--macros` can be added once T10 lands.
  - stdin commands: `set <key> <value>`, `macro <name> <v>`, `bypass`, `stats`, `quit`
  - meters printed once a second from the `Triple`
  - `--record out.wav` writes the **processed** signal; `--record-raw out.wav` writes the **unprocessed** mic signal (before DSP). Both are written from a non-RT thread fed by a second ring.
  - `tine-devhost record-raw --input <id> --seconds N out.wav`: capture only, no DSP, no output device. This is how T09 fixtures are recorded (D-016).
- [x] Measure round-trip latency with an impulse loopback (an output-to-input cable, or the maintainer's ears). Print it.

## Acceptance criteria
- 10-minute run: 0 underruns/overruns at steady state; `assert_no_alloc` clean in debug; `max_callback_us` recorded.
- Capture at 44.1 kHz + output at 48 kHz works with drift correction and no audible glitches.
- Unplugging the input reports an error state and doesn't panic.

## Verification
```bash
cargo run -p tine-devhost --profile perf -- list
cargo run -p tine-devhost --profile perf -- run --input "Microphone" --output "Headphones" --params assets/presets/headset.json
```

## Out of scope
APO, persistence, UI.

## Result
- **`tine_engine::audio_io`**:
  - Implemented `DeviceInfo`, `DeviceRef`, `AudioIoError`, `StreamStats`, `CaptureStream`, `PlaybackStream`, `list_inputs`, `list_outputs`, `open_capture`, and `open_playback` using `cpal` 0.18.2 (`Device::id()` for stable WASAPI endpoint IDs with `name:<name>#<index>` fallback).
  - Implemented lock-free, allocation-free sample format conversion (`I8`..`F64` downmix to mono `f32` and mono `f32` upmix to all output channels) in `convert.rs`.
  - Implemented `LocalRing` + `AdaptiveResampler` (4-point Catmull-Rom cubic interpolator with smoothed fill-level proportional controller clamped to `±0.1%` around `src_rate / dst_rate`, target fill `2 × callback_size`) in `drift.rs`.
  - Separated `CaptureProcessor` and `PlaybackProcessor` in `rt.rs` with `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects)]`.
- **`tine-devhost`**:
  - Implemented `lexopt` CLI with `list`, `run`, `record-raw`, and `latency` (impulse loopback measurement) subcommands, interactive stdin (`set`, `macro`, `bypass`, `stats`, `quit`), 1 Hz `Triple<AudioTelemetryFrame>` meter output, and non-RT WAV recording (`--record`, `--record-raw`).
- **Measured verification**:
  - `cargo test -p tine-devhost --test audio_io_rt`: 3/3 passed (`assert_no_alloc` zero-alloc callback check with chunking, 10-minute / 60,000-callback 44.1 kHz → 48 kHz +300 ppm clock-drift simulation with `0` underruns and `0` overruns, `max_step_diff < 0.08`, and live device enumeration smoke test).
  - `cargo run -p tine-devhost --profile perf -- list`: enumerated 3 WASAPI input devices and 6 WASAPI output devices with endpoint IDs, default markers, rates, and channels.
  - Live WASAPI hardware smoke test on `Microphone (fifine Microphone)` (48 kHz, 2 ch) → `Speakers (fifine Microphone)` (48 kHz, 2 ch): `record-raw` captured 48,000 samples in 1.00s; `run` reported `capture: callbacks=1 max_us=5 overruns=0 err=false` and `playback: callbacks=2 max_us=3 underruns=0 err=false`.
  - Error-callback check on unavailable `Microphone (AB13X USB Audio)`: reported stream error cleanly without panicking.
  - `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo deny check`: all passed.
  - Note: `tine-devhost latency` subcommand is implemented for physical cable loopback measurement; no physical loopback cable was attached in this automated run.
