# 02 — Architecture

> Reflects decisions in [06-decisions.md](06-decisions.md). Where this differs from older text, this file is current.

## 1. Runtime topology

On Windows, Tine runs code in **two OS processes**, but the user owns only **one Tine process**:

```text
┌──────────────────────── audiodg.exe (Windows, LocalService, Session 0) ───────────────────────┐
│  Physical mic driver ─► [Vendor SFX APO (wrapped)] ─► [tine_apo.dll  = tine-dsp pipeline] ─► │──► every app that opens the mic
│                                                       │ reads params  │ writes meters + tap     │    (Discord, CS2, OBS, browsers)
└───────────────────────────────────────────────────────┼───────────────┼─────────────────────────┘
                                                        │  Global\Tine.v1.<endpoint> (named file mapping, repr(C))
┌──────────────────────── tine.exe (user session, single process) ─────┼─────────────────────────┐
│  Main thread: Rune event loop (windowless, keep_running, release_gpu_when_closed)             │
│  Tray thread: rune::Tray (Shell_NotifyIcon on its own message-only window)                    │
│  Controller thread: AppModel → MacroMapper → ParamBlock; persistence (blocked when idle)       │
│  Visualizer thread: only while window visible — tap → spectrum → UiFrame (60 Hz)              │
│  On demand: Auto-Tune capture (cpal), "Hear Myself" monitor (cpal), elevated helper (UAC)     │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **The DSP runs inside `audiodg.exe`** as an APO on the selected capture endpoint. Every application that opens that microphone gets processed audio, with no virtual cable.
- **`tine.exe` never sits in the live audio path.** If it crashes or quits, the APO keeps running with the last saved parameters.
- **The UI is a window, not a process.** Rune's `App` keeps the event loop and state alive. Closing the window destroys the window *and* the GPU device (`release_gpu_when_closed`); reopening recreates them with the same state.

## 2. Cargo workspace & module map

```text
tine/
├── Cargo.toml            # workspace (resolver 3, edition 2024), all external deps declared once
├── crates/
│   ├── tine-protocol/    # no_std: params & ranges, lock-free primitives, shm layout, telemetry; serde types (feature)
│   ├── tine-dsp/         # no_std + alloc: primitives, 6 stages + meter, pipeline, curve, macros, presets (const), calibrator
│   ├── tine-apo/         # cdylib tine_apo.dll: COM APO hosting tine-dsp in audiodg
│   ├── tine-engine/      # control plane: shm client, controller, persistence, cpal I/O, spectrum, Auto-Tune runner, monitor, APO admin
│   ├── tine-ui/          # Rune views, view model, theme, headless scenes (pure; no engine dependency)
│   ├── tine-app/         # bin tine.exe: wires engine + Rune App + tray + CLI + elevated helper
│   └── tine-devhost/     # bin: cpal mic → DSP → headphones, for DSP work without the APO
├── spikes/               # throwaway spike crates (T02, T03); not default members
└── assets/presets/       # archetype JSON (user-facing, mirrored by const tables in tine-dsp)
```

**Dependency direction** (CI-enforced, no cycles):

```text
tine-protocol ◄── tine-dsp ◄── tine-apo
      ▲              ▲  ▲
      │              │  └──── tine-engine ◄── tine-devhost
      └── tine-ui ───┘             ▲
             ▲                     │
             └──────── tine-app ───┘
```

- `tine-dsp`: no `windows`, `cpal`, `serde_json`, `rune`, `std` (except under the `std` feature for tests/examples).
- `tine-ui`: no `tine-engine`, `cpal`, `windows`. It uses `tine-dsp` only for pure math (`curve`, `MacroMapper::explain`).
- `tine-apo`: no `cpal`, `rune`, `tine-engine`.

### Module map (T01 creates every module as an empty stub so tasks own disjoint files)

| Crate | Modules → owning task |
| :--- | :--- |
| `tine-protocol` | `params`, `sync`, `shm`, `telemetry`, `presets`, `settings`, `ids` → T04 |
| `tine-dsp` | `primitives::{db, smoother, biquad, envelope, delay, sliding_max, fifo}`, `rt`, `pipeline`, `meter`, `stages::mod` → T05 · `stages::{input, cleanup}`, `resample` → T06 · `stages::{eq, deesser}`, `curve` → T07 · `stages::{compressor, limiter}` → T08 · `macros`, `presets` → T10 · `calibrator` → T11 |
| `tine-apo` | `com`, `process`, `formats`, `shm_host`, `boot`, `wrap`, `test_host` → T13 |
| `tine-engine` | `audio_io` → T12 · `win`, `apo_admin` → T14 · `shm_client`, `controller`, `model`, `persist`, `paths` → T15 · `spectrum`, `visualizer`, `autotune`, `monitor` → T16 |
| `tine-ui` | `model`, `intent`, `state`, `theme`, `fake`, `scenes`, `views::root` → T18 · `views::{header, canvas}` → T19 · `views::{pro_rack, autotune, onboarding}` → T20 |
| `tine-app` | everything → T17 · `ui_bridge` → T21 |

### Crate manifest plan (T01 writes these; later tasks change only their own crate)

| Crate | Dependencies | `windows` features |
| :--- | :--- | :--- |
| `tine-protocol` | `bytemuck`; opt: `serde`, `serde_json` (`std`+`serde` features); dev: `loom`, `proptest`, `criterion` | — |
| `tine-dsp` | `tine-protocol`, `nnnoiseless`, `rubato`; dev: `hound`, `realfft`, `proptest`, `criterion`, `assert_no_alloc`, `serde_json` | — |
| `tine-apo` | `tine-protocol`, `tine-dsp`, `windows` (+ `windows-core` only if the `implement` macro needs it, pinned to the exact version `windows` uses); dev: `assert_no_alloc`, `proptest` | `Win32_Foundation`, `Win32_System_Com`, `Win32_System_Com_StructuredStorage`, `Win32_System_Variant`, `Win32_Media_Audio`, `Win32_Media_Audio_Apo`, `Win32_Media_KernelStreaming`, `Win32_Media_Multimedia`, `Win32_System_Memory`, `Win32_Security`, `Win32_Security_Authorization`, `Win32_UI_Shell_PropertiesSystem`, `Win32_System_SystemServices` |
| `tine-engine` | `tine-protocol` (std, serde), `tine-dsp` (std), `cpal`, `rubato`, `realfft`, `serde`, `serde_json`, `thiserror`, `tracing`, `windows`; dev: `tempfile`, `proptest` | the `tine-apo` set + `Win32_System_Registry`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_Storage_FileSystem`, `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`, `Win32_Devices_FunctionDiscovery`, `Win32_System_IO` |
| `tine-ui` | `tine-protocol`, `tine-dsp`, `rune` (+`tooling` behind feature) | — |
| `tine-app` | `tine-engine`, `tine-ui`, `rune`, `tracing`, `tracing-subscriber`, `tracing-appender`, `lexopt`, `serde_json`, `windows` | `Win32_System_Console`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_Security` |
| `tine-devhost` | `tine-engine`, `tine-dsp` (std), `tine-protocol`, `lexopt`, `hound` | — |

`windows` feature names are verified against the `windows` 0.62 docs in T01. If a task needs one that's missing, it adds it to **its own** crate manifest.

## 3. Threads and real-time rules

| Context | Real-time? | Allowed |
| :--- | :--- | :--- |
| `APOProcess` (audiodg RT thread) | **Hard RT** | Atomics, preallocated buffers, arithmetic. **No** alloc, locks, syscalls, logging, panics. |
| `Initialize` / `LockForProcess` (APO) | No | Allocate DSP state, open shm, read boot params. |
| cpal callbacks (devhost, Auto-Tune capture, monitor) | Hard RT | Same as `APOProcess`. |
| Rune main thread, tray thread, controller, visualizer | No | Anything, but never wait on an RT thread. |

Full rules: [09-engineering-standards.md](09-engineering-standards.md) §3–5.

## 4. Shared-memory contract (APO ⇄ tine.exe)

A **versioned `#[repr(C)]` block** (`tine-protocol::shm::ShmLayout`) in the named file mapping `Global\Tine.v1.<endpoint>`.
The APO creates it, with a DACL for SYSTEM, LocalService, and interactive users (read/write). The same lock-free primitives
are also used in-process (one implementation, D-009):

| Region | Direction | Primitive |
| :--- | :--- | :--- |
| `ShmHeader`: magic, layout version, APO build, format, heartbeat, state flags, writer claim, bypass lease | APO ⇄ app | atomics |
| `ParamBlock`: all params as `AtomicU32` + `generation: AtomicU64` | app → APO | app stores fields, then bumps the generation (Release); APO reloads when it changes (Acquire) and clamps |
| `Triple<AudioTelemetryFrame>` | APO → app | lock-free triple buffer |
| `SpscRing<32768>`: post-DSP mono tap | APO → app | wait-free SPSC ring; the app computes the spectrum |

If the mapping is absent, the APO isn't loaded on that endpoint (UI: "Not installed / inactive").

## 5. Persistence

| File | Written by | Read by |
| :--- | :--- | :--- |
| `%APPDATA%\Tine\settings.json`: selected endpoint, UI state, monitor device, autostart | tine.exe | tine.exe |
| `%APPDATA%\Tine\profiles\<endpoint>.json`: archetype, macros, pins, calibration, overrides | tine.exe | tine.exe |
| `%ProgramData%\Tine\endpoints\<endpoint>.json`: resolved `DspParams` (boot params) | tine.exe (debounced 500 ms) | APO at `LockForProcess` |
| `%ProgramData%\Tine\backup\<endpoint>.json`: original FxProperties | elevated helper | elevated helper (uninstall) |

All writes are atomic (temp file + replace), versioned (`schema`), and recover from corruption.

## 6. Signal-format handling

- The APO accepts f32 and PCM 16/24/32, 1–8 channels, 16–192 kHz. It downmixes to mono, processes, and writes mono to every channel.
- Every filter is designed for the **actual** rate (D-006). Only the RNNoise path resamples to and from 48 kHz.
- Host block sizes vary; internal FIFOs decouple 480-sample RNNoise frames from host blocks.

## 7. Latency budget

| Source | Latency |
| :--- | :--- |
| RNNoise frame FIFO | 10.0 ms (480 samples @ 48 kHz) |
| Limiter lookahead | 1.5 ms |
| RNNoise resampler (non-48 kHz only) | ~1–2 ms |
| **Total added at 48 kHz** | **≈ 11.5 ms**, constant at runtime |

## 8. Failure modes

| Failure | Behaviour |
| :--- | :--- |
| Panic / NaN / Inf in DSP | Latch pass-through, flag in shm; UI shows "Processing paused — click to restart". |
| `tine.exe` dies during Auto-Tune | Bypass lease expires (≤ 15 s); APO resumes processing. |
| Shm missing / version mismatch | APO runs from boot params; UI offers repair/update. |
| Windows/driver update removes FxProperties | Health check (startup + device property change) → one-click repair (UAC). |
| APO fails to load after install | Installer verifies the heartbeat within 5 s and rolls back with a typed error. |
| Several APO instances on one endpoint | One claims the telemetry writer role; every instance reads params. |
