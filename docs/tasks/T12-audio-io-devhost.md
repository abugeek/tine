# T12 — cpal Audio I/O Module + `tine-devhost`

| Field | Value |
| :--- | :--- |
| Status | todo |
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
- [ ] Device enumeration by stable id (WASAPI endpoint id where cpal exposes it; otherwise name + index; document the limitation).
- [ ] Capture callback: convert any sample format → f32, downmix → mono, run the optional `on_block` (devhost runs `DspPipeline` here), push to the ring. No alloc: scratch buffers are sized from the max buffer size reported at open, grown only outside the callback. If cpal reports a larger buffer at runtime, chunk it.
- [ ] Playback callback: pop from the ring; on underrun, output silence and count it. Clock drift between independent devices: track ring fill level and adjust the resample ratio by ±0.1 % max (rubato async or a simple adaptive linear/cubic interpolator); target fill = 2 × callback size.
- [ ] Error callbacks (device lost) set a flag observed by the owner; no panics.
- [ ] `tine-devhost` CLI (`lexopt`):
  - `list`
  - `run --input <id|name> --output <id|name> [--params file.json] [--record out.wav]`. `--params` takes either `DspParams` JSON or a preset JSON from `assets/presets/` (its base params, no macros). This keeps T12 independent of T10; `--preset`/`--macros` can be added once T10 lands.
  - stdin commands: `set <key> <value>`, `macro <name> <v>`, `bypass`, `stats`, `quit`
  - meters printed once a second from the `Triple`
  - `--record out.wav` writes the **processed** signal; `--record-raw out.wav` writes the **unprocessed** mic signal (before DSP). Both are written from a non-RT thread fed by a second ring.
  - `tine-devhost record-raw --input <id> --seconds N out.wav`: capture only, no DSP, no output device. This is how T09 fixtures are recorded (D-016).
- [ ] Measure round-trip latency with an impulse loopback (an output-to-input cable, or the maintainer's ears). Print it.

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
