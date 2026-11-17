# 02 — Architecture

> Reflects decisions in [06-decisions.md](06-decisions.md). Where this differs from older text, this file is current.

## 1. Runtime topology

On Windows, Tine runs entirely in the user session (D-018: no APO inside `audiodg.exe`, no virtual-cable installer bundled, no runtime elevation), split into an always-on parent and an on-demand UI child process (D-017):

```text
┌──────────────────────── tine.exe (user session, always-on parent) ────────────────────────────┐
│  Physical mic (cpal) ─► [VirtualCableRouter = tine-dsp] ─► "CABLE Input" (virtual cable) ─►  │
│  Tray thread: rune::Tray (Shell_NotifyIcon on its own message-only window)                     │  apps pick
│  Controller thread: AppModel → MacroMapper → ParamBlock; persistence (blocked when idle)       │  "CABLE Output"
│  Visualizer thread: tap → spectrum → UiFrame (60 Hz)                                           │  (Discord, CS2,
│  Auto-Tune capture (tap-fed, D-029), "Hear Myself" input monitor (cpal, D-029)                 │   OBS, browsers)
└───────────────────────────────────────┬───────────────────────────────────────────────────────┘
                                        │  \\.\pipe\tine-<user-sid> (JSON line IPC)
┌──────────────────────── tine.exe --ui (on-demand child process, D-017) ──────────────────────┐
│  Rune window + wgpu DX12 surface; exits on window close so parent retains 0 MB GPU memory.     │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **The DSP runs inside `tine-engine`'s `VirtualCableRouter`** (D-018). Every application that selects `CABLE Output (VB-Audio Virtual Cable)` receives processed audio.
- **The UI is a short-lived child process (`tine.exe --ui`)** (D-017). Closing the window exits the child process and releases all DX12/GPU driver memory; the parent tray and audio routing continue uninterrupted.

## 2. Cargo workspace & module map

```text
tine/
├── Cargo.toml            # workspace (resolver 3, edition 2024), all external deps declared once
├── crates/
│   ├── tine-protocol/    # no_std: params & ranges, lock-free primitives, shm layout, telemetry; serde types (feature)
│   ├── tine-dsp/         # no_std + alloc: primitives, 6 stages + meter, pipeline, curve, macros, presets (const), calibrator
│   ├── tine-engine/      # control plane + audio router: cpal I/O, VirtualCableRouter, controller, persistence, spectrum, Auto-Tune runner, monitor
│   ├── tine-ui/          # Rune views, view model, theme, headless scenes (pure; no engine dependency)
│   ├── tine-app/         # bin + lib tine.exe: wires engine + Rune App (--ui child) + tray + CLI + ui_bridge
│   └── tine-devhost/     # bin: cpal mic → DSP → headphones, for DSP work
└── assets/presets/       # archetype JSON (user-facing, mirrored by const tables in tine-dsp)
```

**Dependency direction** (CI-enforced, no cycles):

```text
tine-protocol ◄── tine-dsp
      ▲              ▲
      │              └──── tine-engine ◄── tine-devhost
      └── tine-ui ───┘          ▲
             ▲                  │
             └────── tine-app ──┘
```

- `tine-dsp`: no `windows`, `cpal`, `serde_json`, `rune`, `std` (except under the `std` feature for tests/examples).
- `tine-ui`: no `tine-engine`, `cpal`, `windows`. It uses `tine-dsp` only for pure math (`curve`, `MacroMapper::explain`).

### Module map

| Crate | Modules → owning task |
| :--- | :--- |
| `tine-protocol` | `params`, `sync`, `shm`, `telemetry`, `presets`, `settings`, `ids` → T04 |
| `tine-dsp` | `primitives::{db, smoother, biquad, envelope, delay, sliding_max, fifo}`, `rt`, `pipeline`, `meter`, `stages::mod` → T05 · `stages::{input, cleanup}`, `resample` → T06 · `stages::{eq, deesser}`, `curve` → T07 · `stages::{compressor, limiter}` → T08 · `macros`, `presets` → T10 · `calibrator` → T11 |
| `tine-engine` | `audio_io` → T12 · `win`, `apo_admin` (virtual-cable discovery/health) → T14 · `shm_client`, `controller`, `model`, `persist`, `paths` → T15 · `spectrum`, `visualizer`, `autotune`, `monitor` → T16 |
| `tine-ui` | `model`, `intent`, `state`, `theme`, `fake`, `scenes`, `views::root` → T18 · `views::{header, canvas}` → T19 · `views::{pro_rack, autotune, onboarding}` → T20 |
| `tine-app` | everything → T17 · `ui_bridge`, `services` → T21 |

### Crate manifest plan

| Crate | Dependencies | `windows` features |
| :--- | :--- | :--- |
| `tine-protocol` | `bytemuck`; opt: `serde`, `serde_json` (`std`+`serde` features); dev: `loom`, `proptest`, `criterion` | — |
| `tine-dsp` | `tine-protocol`, `nnnoiseless`, `rubato`; dev: `hound`, `realfft`, `proptest`, `criterion`, `assert_no_alloc`, `serde_json` | — |
| `tine-engine` | `tine-protocol` (std, serde), `tine-dsp` (std), `cpal`, `rubato`, `realfft`, `serde`, `serde_json`, `thiserror`, `tracing`, `windows`; dev: `tempfile`, `proptest` | `Win32_Foundation`, `Win32_System_Com`, `Win32_System_Com_StructuredStorage`, `Win32_System_Variant`, `Win32_Media_Audio`, `Win32_Media_KernelStreaming`, `Win32_Media_Multimedia`, `Win32_System_Memory`, `Win32_Security`, `Win32_Security_Authorization`, `Win32_UI_Shell_PropertiesSystem`, `Win32_System_SystemServices`, `Win32_System_Registry`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_Storage_FileSystem`, `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`, `Win32_Devices_FunctionDiscovery`, `Win32_System_IO` |
| `tine-ui` | `tine-protocol`, `tine-dsp`, `rune` (+`tooling` behind feature) | — |
| `tine-app` | `tine-engine`, `tine-ui`, `rune`, `tracing`, `tracing-subscriber`, `lexopt`, `serde_json`, `windows` | `Win32_System_Console`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_Security` |
| `tine-devhost` | `tine-engine`, `tine-dsp` (std), `tine-protocol`, `lexopt`, `hound` | — |

## 3. Threads and real-time rules

| Context | Real-time? | Allowed |
| :--- | :--- | :--- |
| cpal audio callbacks (`VirtualCableRouter`, `devhost`, `Monitor`) | **Hard RT** | Atomics, preallocated buffers, arithmetic. **No** alloc, locks, syscalls, logging, panics. |
| Stream setup / `DspPipeline::new` | No | Allocate DSP state, initialize rings, read boot params. |
| Tray thread, controller, visualizer, `tine.exe --ui` child | No | Anything, but never wait on an RT thread. |

Full rules: [09-engineering-standards.md](09-engineering-standards.md) §3–5.

## 4. Lock-free state & IPC contracts

The lock-free primitives (`tine-protocol::sync`: `ParamBlock`, `Triple<T>`, `SpscRing<N>`) are `#[repr(C)]` and used in-process between `VirtualCableRouter` and the controller/visualizer (D-009):

| Region | Direction | Primitive |
| :--- | :--- | :--- |
| `ParamBlock`: all params as `AtomicU32` + `generation: AtomicU64` | controller → router | controller stores fields, then bumps generation (Release); router reloads when it changes (Acquire) and clamps |
| `Triple<AudioTelemetryFrame>` | router → visualizer | lock-free triple buffer |
| `SpscRing<32768>`: post-DSP mono tap | router → visualizer / Auto-Tune | wait-free SPSC ring; visualizer computes the spectrum (or Auto-Tune drains raw bypassed samples, D-029) |
| `Local\Tine.v1.autotune-shim-*` | Auto-Tune runner ⇄ router | session-local mapping for the 15 s bypass lease (D-029) |
| `\\.\pipe\tine-<user-sid>` | `tine.exe` parent ⇄ `--ui` child | JSON line IPC for live status, meters, and intents (D-017, D-028, D-029) |

## 5. Persistence

| File | Written by | Read by |
| :--- | :--- | :--- |
| `%APPDATA%\Tine\settings.json`: selected endpoint, UI state, monitor device, autostart | tine.exe | tine.exe |
| `%APPDATA%\Tine\profiles\<endpoint>.json`: archetype, macros, pins, calibration, overrides | tine.exe | tine.exe |
| `%ProgramData%\Tine\endpoints\<endpoint>.json`: resolved `DspParams` (boot params, D-022) | tine.exe (debounced 500 ms) | tine.exe |

All writes are atomic (temp file + replace), versioned (`schema`), and recover from corruption.

## 6. Signal-format handling

- `tine-engine` captures and plays back via `cpal` WASAPI, downmixing multi-channel input to mono and writing mono to every output channel.
- Every filter is designed for the **actual** rate (D-006). Only the RNNoise path resamples to and from 48 kHz.
- Host block sizes vary; internal FIFOs decouple 480-sample RNNoise frames from host blocks.

## 7. Latency budget

| Source | Latency |
| :--- | :--- |
| RNNoise frame FIFO | 10.0 ms (480 samples @ 48 kHz) |
| RNNoise OLA synthesis delay (D-019) | 10.0 ms (480 samples @ 48 kHz) |
| Limiter lookahead + true-peak FIR group delay (D-019) | 1.58 ms (72 + 4 = 76 samples @ 48 kHz) |
| RNNoise resampler (non-48 kHz only) | ~1–2 ms |
| **Total added DSP latency at 48 kHz** | **21.58 ms (1036 samples)**, constant at runtime |

## 8. Failure modes

| Failure | Behaviour |
| :--- | :--- |
| Panic / NaN / Inf in DSP | Latch pass-through, flag in telemetry; UI shows "Processing paused — click to restart". |
| `tine.exe` dies during Auto-Tune | Bypass lease expires (≤ 15 s); bypass state is never persisted (D-029). |
| Virtual cable missing or disconnected | Health check detects missing `CABLE Input`; UI onboarding/badge shows install/remedy steps. |
| Corrupt settings / profile JSON | Backed up as `.corrupt-<ts>` and defaults restored. |
