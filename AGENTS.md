# AGENTS.md — Tine Architecture & Implementation Specification

> **Start here, then read `docs/`.** This file holds the rules every agent must follow. Details live in
> [`docs/project/`](docs/project/) (permanent reference) and work packages in [`docs/tasks/`](docs/tasks/)
> (start with [`docs/tasks/00-roadmap.md`](docs/tasks/00-roadmap.md)). When this file and
> [`docs/project/06-decisions.md`](docs/project/06-decisions.md) disagree, the decisions log wins. Fix this file in the same change.

## 1. Project Overview & Core Philosophy

Tine is an ultra-lightweight, native, open-source microphone DSP (Digital Signal Processing) engine and audio conditioner written in Rust. It bridges the gap between bloated consumer gaming suites (SteelSeries GG Sonar) and overly complex studio mixers (Voicemeeter Potato). Why it exists and who it is for: [`docs/project/01-vision.md`](docs/project/01-vision.md).

**Scope of v0.1 (D-001, D-003, D-015):** Windows 10 22H2+ / Windows 11, x86_64, MSVC toolchain, public open-source release under `MIT OR Apache-2.0`. `tine-dsp` and `tine-protocol` stay platform-neutral so macOS/Linux can follow.

### Non-Negotiable Engineering Constraints

- **Universal Microphone Adaptability**: Tine must make any microphone (Dynamic USB/XLR, Condenser, Headset/Lapel, or Webcam/Laptop mic) sound broadcast-ready using a combination of Hardware Archetype Base Curves, an 8-Second Auto-Calibration Wizard, and a Dual-Tier UI (Golden Line Macro Mode vs. Surgical Pro Mode).
- **Never Break the User's Audio**: Every audio callback is panic-free; any NaN/Inf in the DSP resets state and latches pass-through so the user's mic never silences or clips. (v0.1 runs in the user session via virtual-cable routing per D-018, never inside `audiodg.exe`.)
- **Real-Time Audio Safety**: Every real-time path (`DspPipeline::process` in `tine-dsp`, `cpal` callbacks in `tine-engine`/`tine-devhost`) must be 100% lock-free, allocation-free and panic-free. Zero `Mutex::lock()`, `RwLock`, `Vec::push()`, `Box::new()`, logging, or syscalls inside the audio callback. Enforced with `assert_no_alloc` in tests and `clippy::{unwrap_used, expect_used, panic, indexing_slicing}` denied in RT crates.
- **Zero Idle GPU/UI Footprint**: `tine.exe` (D-017) keeps a lightweight tray + control plane + audio router parent always on, and spawns `tine.exe --ui` as a short-lived child process when the window opens. Closing the window exits `tine.exe --ui` and releases all `wgpu`/DX12 VRAM. With the window closed `tine.exe` targets < 12 MB Working Set (< 4 MB Private Bytes, hard limit 20 MB) and < 0.5 % CPU.
- **Efficient From Day One**: Every performance budget in [`docs/project/09-engineering-standards.md` §7](docs/project/09-engineering-standards.md#7-performance-budgets-ci-enforced-where-marked-) is a requirement, not a wish. Key budgets: < 150 µs per 480-sample frame for the whole DSP, zero allocations on RT paths, `tine.exe` < 24 MiB (R-23), added DSP latency ≤ 21.6 ms at 48 kHz (1036 samples, D-019), and cold start to tray < 150 ms. CI enforces the measurable ones.
- **Rate-Agnostic Pipeline, 48 kHz Native** (D-006): DSP runs on mono `f32`. 48 kHz is the native rate (matches `nnnoiseless`'s 480-sample / 10 ms frame). Every stage designs coefficients from the actual sample rate; only the denoiser resamples to/from 48 kHz when needed.

---

## 2. Runtime Topology & Cargo Workspace

Full detail: [`docs/project/02-architecture.md`](docs/project/02-architecture.md).

```text
tine.exe (user session, always-on parent)                 tine.exe --ui (child, on-demand)
  mic (cpal) ─► [VirtualCableRouter = tine-dsp]            Rune window + wgpu DX12 surface
             ─► "CABLE Input" (virtual cable)              exits on close (0 VRAM when closed)
             ─► apps pick "CABLE Output" (D-018)             ▲
  tray + controller + visualizer + monitor (cpal) ───────────┴── \\.\pipe\tine-<user-sid>
```

```text
tine/
├── Cargo.toml                 # Workspace root (resolver 3, edition 2024), every external dep declared once
├── AGENTS.md                  # This file
├── docs/project/              # Permanent reference docs (start with 09-engineering-standards.md)
├── docs/tasks/                # Temporary work packages T01..T23, organized in parallel lanes
├── crates/
│   ├── tine-protocol/         # no_std: params & ranges (single source of truth), lock-free primitives, repr(C) shm layout, telemetry, serde types
│   ├── tine-dsp/              # no_std + alloc: DSP primitives, 7-stage pipeline, response curve, MacroMapper, presets (const), AutoCalibrator
│   ├── tine-engine/           # Control plane + router: cpal I/O, VirtualCableRouter, controller, persistence, spectrum, Auto-Tune runner, monitor
│   ├── tine-ui/               # Anomaly Rune views, view model, theme, headless scenes (pure; no engine dependency)
│   ├── tine-app/              # bin + lib `tine.exe`: parent tray/router + `--ui` child spawner + CLI + `ui_bridge`
│   └── tine-devhost/          # bin: cpal mic → DSP → headphones harness for development
└── assets/
    └── presets/               # Built-in archetype JSON (mirrored by const tables in tine-dsp)
```

Dependency direction (CI-enforced, no cycles): `protocol ← dsp ← {engine, ui}`, `protocol ← ui`, `{engine, ui} ← app`, `{dsp, engine} ← devhost`.
- `tine-dsp` never depends on `windows`, `cpal`, `serde_json`, `rune`, or anything that allocates at runtime.
- `tine-ui` never depends on `tine-engine`, `cpal` or `windows`.

The module map (which task owns which file) and each crate's manifest are in [`docs/project/02-architecture.md` §2](docs/project/02-architecture.md#2-cargo-workspace--module-map).

### Approved Dependencies (`Cargo.toml`)

Latest stable versions checked on **2026-11-16**. Re-check them on crates.io when T01 runs. Every entry is declared with
`default-features = false`, then only the needed features are enabled. Policy: [`docs/project/09-engineering-standards.md` §2](docs/project/09-engineering-standards.md#2-dependency-policy).

```toml
[workspace.dependencies]
# Audio
cpal          = { version = "0.18.2", default-features = false, features = ["realtime"] } # WASAPI + RT thread priority (tine-engine, tine-devhost only)
nnnoiseless   = { version = "0.5.2",  default-features = false }  # Pure-Rust RNNoise, 480-sample frames @ 48 kHz
rubato        = { version = "5.1.0",  default-features = false }  # Resampling (RNNoise path at non-48 kHz; device clock drift); alloc-free process_into_buffer
realfft       = { version = "3.5.1",  default-features = false }  # Spectrum in tine.exe only (never in audiodg)
# Windows
windows       = { version = "0.62.2", default-features = false }  # COM APO, WASAPI policy, registry, file mapping; features per crate
# Data & errors
bytemuck      = { version = "1.25.2", default-features = false, features = ["derive"] }
serde         = { version = "1.0.229", default-features = false, features = ["derive", "std"] }
serde_json    = { version = "1.0.154", default-features = false, features = ["std"] }
thiserror     = { version = "2.0.21", default-features = false }
# App
tracing            = { version = "0.1.44", default-features = false, features = ["std", "attributes"] }
tracing-subscriber = { version = "0.3.23", default-features = false, features = ["fmt", "std"] }
tracing-appender   = { version = "0.2.5",  default-features = false }
lexopt             = { version = "0.3.2",  default-features = false }
# UI (tine-ui, tine-app only). Pinned dev revision (2026-11-16); bump only in a dedicated PR.
rune = { git = "https://github.com/anomalyco/rune", rev = "631e8a67cb3c40225efa3f6c4f57b80e7b18f190", default-features = false, features = ["accessibility", "embedded-fonts"] }
# Dev / test
proptest       = "1.12.0"
loom           = "0.7.3"
criterion      = { version = "0.8.2", default-features = false, features = ["cargo_bench_support"] }
hound          = "3.5.1"
assert_no_alloc = "1.1.2"
tempfile       = "3.27.0"

# Rune requires every app that builds it to apply its patched crates (see Rune's Cargo.toml):
[patch.crates-io]
winit             = { git = "https://github.com/anomalyco/rune", rev = "631e8a67cb3c40225efa3f6c4f57b80e7b18f190" }
wgpu-hal          = { git = "https://github.com/anomalyco/rune", rev = "631e8a67cb3c40225efa3f6c4f57b80e7b18f190" }
parley_engine     = { git = "https://github.com/anomalyco/rune", rev = "631e8a67cb3c40225efa3f6c4f57b80e7b18f190" }
accesskit_windows = { git = "https://github.com/anomalyco/rune", rev = "631e8a67cb3c40225efa3f6c4f57b80e7b18f190" }
```

Tools (CI): `cargo-nextest` 0.9.152, `cargo-deny` 0.20.2, `cargo-fuzz` 0.13.2, Rust 1.99.1 (pinned in `rust-toolchain.toml`, same as Rune).

**Deliberately not used** (decisions in `docs/project/06-decisions.md`):
- `biquad`: in-house filters (D-010)
- `rtrb` and `triple_buffer`: one in-house lock-free implementation shared with shared memory; `triple_buffer` 9 is also MPL-2.0 (D-009)
- `tray-icon`: `rune::Tray` (D-011)
- `clap`, `dirs`, `anyhow`: binary-size budget (D-013)

New dependencies need a reason, a license from the allow-list in `deny.toml`, `default-features = false`, and a passing `cargo deny check`.

> **Note for Windows GNU (`x86_64-pc-windows-gnu`) builds with Rune**: Prefer `x86_64-pc-windows-msvc`. If building under GNU, ensure MinGW-w64 `dlltool.exe` is on `PATH` for `raw-dylib` imports.

---

## 3. Universal Microphone Profiling & Auto-Calibration (`tine-dsp`)

Full detail (preset JSON schema, Auto-Tune rejections, mapping tables): [`docs/project/04-mic-adaptation.md`](docs/project/04-mic-adaptation.md).

Because every microphone capsule and room environment is different, Tine never hardcodes a single static EQ. Instead, it uses a 3-Layer Adaptation System:

### Layer A: Microphone Archetype Base Profiles

| Archetype | Acoustic Problem to Solve | Baseline DSP Starting Curve |
| :--- | :--- | :--- |
| **1. Dynamic USB / XLR** (e.g., Fifine AM8, SM7B, PodMic) | Dark highs, boxy low-mids (300–400 Hz), requires high gain. | HPF @ 80 Hz, Low-Shelf +2.0 dB @ 110 Hz, Peaking -3.0 dB @ 340 Hz (Q=1.4), High-Shelf +3.5 dB @ 5.5 kHz. |
| **2. Studio Condenser** (e.g., HyperX QuadCast, Blue Yeti, AT2020) | Sensitive to room echo, harsh sibilance (S/T spikes at 6–8 kHz), plosives. | HPF @ 95 Hz, Low-Shelf +0.5 dB @ 120 Hz, Peaking -2.0 dB @ 280 Hz, De-Esser active @ 6.8 kHz, gentle High-Shelf +1.0 dB @ 10 kHz. |
| **3. Gaming Headset / Boom Mic** | Thin bass, nasal mids (800–1200 Hz), close-proximity breath bursts. | HPF @ 100 Hz, Low-Shelf +4.0 dB @ 130 Hz (synthetic body), Peaking -3.5 dB @ 950 Hz (Q=1.2), High-Shelf +2.0 dB @ 4.5 kHz. |
| **4. Laptop / Webcam / Lav** | Distant, roomy, high background noise floor, zero warmth. | HPF @ 110 Hz, RNNoise Wet Mix 90%, Expander Ratio 4:1, Peaking +3.0 dB @ 2.5 kHz (speech intelligibility). |

### Layer B: The 8-Second Auto-Calibration Wizard ("Tune My Mic")

`tine.exe` puts the DSP in bypass (via the session-local Auto-Tune lease, D-029), captures the raw mic (via the tap in D-018), and feeds the pure `AutoCalibrator` state machine:

1. **Phase 1 — Room Noise Floor (3 Seconds of Silence):**
   - Measure peak and RMS dBFS of ambient room noise.
   - Set Expander Threshold to `Noise_RMS_dB + 6.0 dB`.
   - If `Noise_RMS_dB > -48.0 dBFS`, set `nnnoiseless` AI Wet/Dry mix to `0.85`; otherwise `0.50` to preserve natural vocal texture.
2. **Phase 2 — Vocal Reference (5 Seconds of Normal Speaking):**
   - Measure integrated vocal RMS, peak crest factor, and spectral tilt (energy < 350 Hz vs. 1 kHz – 6 kHz).
   - **Auto-Gain**: Set Pre-Gain so average speech hits `-18.0 dBFS` RMS.
   - **Auto-Compressor**: Set Compressor Threshold to `Vocal_RMS_dB + 2.0 dB` with makeup gain chosen by offline simulation of the full pipeline so output peaks sit between `-6.0 dBFS` and `-3.0 dBFS`.

### Layer C: Dual-Tier Control Model ("The Golden Line" vs. Pro Mode)

Both modes mutate the exact same underlying `DspParams` struct:

- **Golden Line Mode (Default)**: 4 Macro Nodes (`0.0` to `1.0`), centred on the effective base at `0.5` (a = preset value, or the calibrated value when one exists; D-021 — exact equations in [`docs/project/04-mic-adaptation.md`](docs/project/04-mic-adaptation.md) Layer C):
  - **Purity (Noise & Gate)**: RNNoise wet mix (`a ± 0.5`) and Expander threshold (`a ± 15 dB`; more purity trusts the denoiser and gates less).
  - **Body (Warmth & Mud Control)**: Low-Shelf (`110 Hz`, `b ± 4 dB`) while cutting the Mud Filter (`320 Hz`, down to `−5 dB` above centre — never boosts the mud).
  - **Presence (Clarity & De-Ess)**: Presence Peak (`4.2 kHz`, up to `+5 dB` above centre), Air Shelf (`8.5 kHz`, up to `+6 dB` above centre), de-esser on above `0.6` (or when the archetype base enables it), amount `3 → 8 dB` over `0.6 → 1.0`.
  - **Leveler (Compression & Voice Focus)**: Compressor Ratio (`b ± 1.75`), Threshold lowered by up to `10 dB` from the effective threshold, auto-makeup gain (a calibration supplies its verified manual makeup instead).
- **Pro Mode (Expanded Drawer)**: Exact HPF cutoff (`20 Hz – 200 Hz`), Gate Ratio/Hysteresis/Attack/Release, 5-Band Fully Parametric EQ (Freq, Gain, Q, FilterType), De-Esser Frequency/Threshold/Amount, Compressor (Threshold, Ratio, Attack, Release, Knee, Makeup), and Limiter Ceiling (optional true-peak).
- **Pinning rule (D-007)**: Editing a parameter in Pro mode pins it; macros stop driving pinned parameters until unpinned.

---

## 4. Complete 7-Stage Real-Time DSP Pipeline (`tine-dsp`)

Full detail and the authoritative parameter table (ranges, defaults): [`docs/project/03-dsp-pipeline.md`](docs/project/03-dsp-pipeline.md).

Every audio buffer passes through this deterministic chain in order. Parameter transitions use a One-Pole Smoother ($\tau = 10\text{ ms}$) per sample to eliminate zipper noise.

```text
[Input f32, mono (downmixed), endpoint rate — 48 kHz native]
   │
   ├─► Stage 1: Input Gain & DC Blocker + Butterworth HPF (2nd Order Biquad)
   ├─► Stage 2: Hybrid AI Denoiser (`nnnoiseless` 480-sample FIFO + 480-sample OLA synthesis delay; 20 ms total per D-019) + Soft-Knee Hysteresis Expander
   ├─► Stage 3: 5-Band Parametric EQ (Cascaded in-house TDF-II Biquads)
   │     Band 1: Low-Shelf (Warmth)      | Band 2: Peaking (Mud Cut ~320Hz)
   │     Band 3: Peaking (Nasal/Mid)     | Band 4: Peaking (Presence ~4.2kHz)
   │     Band 5: High-Shelf (Air ~8.5kHz)
   ├─► Stage 4: Dynamic Split-Band De-Esser (sidechain 5.5–9.5 kHz, GR on high band only)
   ├─► Stage 5: Program-Dependent Vocal Compressor (RMS/Peak, soft knee 6 dB, auto-makeup)
   ├─► Stage 6: Brickwall Lookahead Limiter (1.5 ms lookahead + 4-sample FIR delay = 76 samples, ceiling -1.0 dBFS)
   └─► Stage 7: Metering Tap — meters + GR to telemetry; post-limiter samples to a tap ring.
         The 32-band spectrum is computed outside the RT audio callback by the visualizer service.
```

Added pipeline latency at 48 kHz: **1036 samples (21.58 ms)** (D-019: 960 denoiser + 76 limiter). Performance contract: 480 samples, all stages on, < 150 µs on one core, zero heap allocations after `DspPipeline::new`. Output is always finite (NaN/Inf → reset + pass-through latch).

---

## 5. Lock-Free State & Telemetry Contract (`tine-protocol`)

The DSP (in `audiodg.exe`) and the controller (`tine.exe`) live in **different processes** (D-005). Crates like `triple_buffer`/`rtrb` hold in-process heap pointers and cannot live in shared memory, so Tine has one `#[repr(C)]` implementation of each primitive (`tine-protocol::sync`), used everywhere:

- **Cross-process (APO ⇄ tine.exe)**: a versioned `#[repr(C)]` block in the named file mapping `Global\Tine.v1.{endpoint-guid}`, created by the APO with a DACL granting LocalService and interactive users access:
  - `ShmHeader` (magic, `layout_version`, APO version, format, heartbeat, state flags incl. pass-through latch)
  - `ParamBlock`: one `AtomicU32` (f32 bits) per parameter + `generation: AtomicU64`. Writer stores fields, then bumps generation (`Release`); the APO re-reads when generation changes (`Acquire`) and clamps every value.
  - `ShmTriple<AudioTelemetryFrame>`: hand-rolled atomic triple buffer.
  - `ShmSpscRing`: wait-free f32 ring for the post-DSP audio tap.
  - Only fixed-width integers/floats; no pointers, `usize`, `bool` or niche enums. Size/alignment static asserts; layout changes bump `layout_version`.
- **In-process (controller ⇄ UI, devhost, monitor)**: the *same* `tine-protocol::sync` primitives on the heap (D-009); `AtomicF32` for single values.

```rust
use core::sync::atomic::{AtomicU32, Ordering};

/// Helper for storing f32 inside AtomicU32 with Relaxed ordering
#[repr(transparent)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub const fn new(val: f32) -> Self { Self(AtomicU32::new(val.to_bits())) }
    #[inline(always)]
    pub fn load(&self) -> f32 { f32::from_bits(self.0.load(Ordering::Relaxed)) }
    #[inline(always)]
    pub fn store(&self, val: f32) { self.0.store(val.to_bits(), Ordering::Relaxed); }
}

/// Published ~60 Hz by the APO via ShmTriple. Spectrum is NOT here; tine.exe derives it from the tap ring.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioTelemetryFrame {
    pub sequence: u64,
    pub flags: u32,                 // pass-through latch, bypass, clipping
    pub input_rms_db: f32,
    pub input_peak_db: f32,
    pub output_rms_db: f32,
    pub output_peak_db: f32,
    pub gate_attenuation_db: f32,
    pub comp_gain_reduction_db: f32,
    pub deesser_reduction_db: f32,
}
```

The parameter set (`ParamId`, `SPECS` ranges/defaults, `DspParams`) is defined once in `tine-protocol` and must match the table in [`docs/project/03-dsp-pipeline.md`](docs/project/03-dsp-pipeline.md). It includes stage enables, EQ filter types, gate ratio/hysteresis/attack, de-esser frequency, compressor knee and auto-makeup. These fields were missing from the original `SharedDspState` (D-008). `monitor_loopback` ("Hear Myself") is controller state in `tine.exe`, not a DSP parameter.

---

## 6. UI Architecture & Agent Verification (`tine-ui` with Anomaly Rune)

Full detail: [`docs/project/05-ui-ux.md`](docs/project/05-ui-ux.md).

`tine-ui` is built with Rune ([github.com/anomalyco/rune](https://github.com/anomalyco/rune), MIT, pre-release). Agents writing `tine-ui` MUST follow Rune's functional view model (`fn view(s: &AppState, cx: &mut Cx) -> El`) and use Rune's headless rendering to verify every UI change. Check the API against Rune's README/examples at the pinned rev; do not invent API.

### Visual Layout & UX Requirements

- **Top Header Bar**:
  - Input Device Selector (Dropdown of physical mics) + **Virtual-Cable status badge** (`Active` / `Cable missing` / `Inactive` / `Error`, D-018). There is no output-target selector: `tine-engine` automatically routes to `CABLE Input` (D-008, D-018).
  - Archetype Selector Pill (Dynamic, Condenser, Headset, Laptop) + `"Auto-Tune (8s)"` button.
  - `"Hear Myself"` (Monitor Toggle) with latency warning badge & A/B Bypass Toggle (hotkey `Space` while window focused to compare Raw vs. Tine).
- **Center Canvas — "The Golden Line"**:
  - A dark-mode canvas rendering the real-time 32-band voice spectrum underneath a glowing response spline.
  - 4 Draggable Macro Nodes (**Purity**, **Body**, **Presence**, **Leveler**). Hovering or dragging a node uses Rune's spring animations to expand a tooltip showing what it is doing under the hood.
- **Bottom Collapsible Drawer — "Surgical Pro Rack"**:
  - Toggled via a `[Pro Controls ▾]` pill.
  - Precision sliders and numeric readouts for stages 1–6 (generated from `SPECS`), pin indicators, plus live Gain Reduction meters for the Gate, De-Esser, and Compressor.

### Mandatory Headless Verification Protocol for Coding Agents

Any agent modifying `tine-ui` must execute this self-check loop before marking a task complete:

1. Define named test scenes: `scene_golden_line`, `scene_pro_drawer_open`, `scene_auto_tune_wizard`, `scene_apo_not_installed`.
2. Render each headless to PNG (1280×800 and 960×640) and dump the JSON element/accessibility tree.
3. Check the JSON tree for: no overlapping bounding boxes among interactive siblings, no clipping, text contrast ≥ 4.5:1 from theme tokens, and correct keyboard focus order (`focusable(true)` with role + label on all macro nodes, pills and switches).
4. Compare against blessed snapshots (`UPDATE_SNAPSHOTS=1` to bless intentional changes).

---

## 7. Windows Output Routing Strategy (D-018, supersedes D-004)

**v0.1 — Virtual-cable output from `tine-engine` (no native APO).** T02 proved an unsigned Rust COM APO (`apo_spike.dll`) is not loaded by `audiodg.exe` via `FxProperties` on stock Windows 11 24H2+. Under **D-018** and **D-029**, `tine-engine`'s `VirtualCableRouter` captures the physical mic via `cpal` WASAPI, runs `DspPipeline`, and outputs to `CABLE Input (VB-Audio Virtual Cable)`. Every app (Discord, CS2, OBS, browsers) selects `CABLE Output` as its microphone.

- **No runtime elevation**: `tine.exe` runs entirely in the user session. Per-endpoint boot params are persisted in `%ProgramData%\Tine\endpoints\{guid}.json` (Users: modify ACL created by the MSI, D-030).
- **Auto-Tune**: `services` hosts a session-local `Local\Tine.v1.autotune-shim-*` bypass lease and feeds the calibrator from the routing tap while bypassed (D-029).
- **"Hear Myself"**: `tine.exe` monitors the microphone *input* signal to the chosen headphones via `cpal` (processed-capture monitoring requires WASAPI render-loopback, deferred post-v0.1; D-029).
- **Known limits**: apps opening the mic in exclusive mode bypass shared-mode capture and must point to `CABLE Output`. A driver-packaged/signed APO is deferred post-v0.1.

---

## 8. Agent Execution Plan (parallel lanes)

Work packages live in [`docs/tasks/`](docs/tasks/). The lane layout, dependencies and parallelism are in
[`docs/tasks/00-roadmap.md`](docs/tasks/00-roadmap.md); the task lifecycle is in [`docs/tasks/README.md`](docs/tasks/README.md).
A task is ready when every task it depends on is done. Each task lists the files it **owns**; two tasks in flight never share a file.

| Lane | Tasks |
| :--- | :--- |
| 0 — Foundation | T01 workspace/toolchain/CI · T04 protocol contracts · T05 DSP primitives + `DspPipeline` skeleton |
| A — DSP | T06 ‖ T07 ‖ T08 stages → T09 integration + perf gate → T10 presets + macros → T11 Auto-Tune math |
| B — Windows / APO | **T02 APO spike (go/no-go)** → T13 `tine_apo.dll` → T14 install/health/repair |
| C — Engine / App | T03 Rune lifecycle spike → T12 cpal I/O + devhost → T15 engine core → T16 audio services → T17 `tine.exe` shell |
| D — UI | T18 foundation + headless harness → T19 header + Golden Line → T20 Pro rack + Auto-Tune + onboarding |
| Z — Integration & ship | T21 end-to-end → T22 QA → T23 release |

To assign a task, tell the agent: *"Read `AGENTS.md`, `docs/project/09-engineering-standards.md`, then `docs/tasks/<file>` and do it."*

### Definition of done (every task)

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace` pass.
- Only files listed under the task's **Owns** were changed (or the change is listed under `## Requests` and approved).
- Every acceptance criterion is met, with measured numbers in the task's `## Result` section.
- No rule in [`docs/project/09-engineering-standards.md`](docs/project/09-engineering-standards.md) broken.
- Lasting knowledge (new decision, changed range, new risk) is folded into `docs/project/`, and this file is updated if a rule changed.
