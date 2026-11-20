# Tine v0.1.0 — Community Launch & SignPath Foundation Re-Application Playbook

This playbook is the copy-paste-ready launch kit for publishing Tine `v0.1.0`, generating public open-source adoption/visibility signals, and re-applying to **SignPath Foundation** for free OSS Authenticode code signing (per [D-041](../project/06-decisions.md)).

---

## Section 1: Measured Engineering Highlights Table

All Tine and SteelSeries GG / Sonar measurements below were captured on the project reference workstation (Ryzen 7 9800X3D, 32 GB RAM, RTX 5070, Windows 11, RivaTuner OSD active; see [`T24-perf-competitor-followup.md`](T24-perf-competitor-followup.md) and [`06-decisions.md`](../project/06-decisions.md) D-017, D-033, D-034, D-037–D-041). Voicemeeter rows reflect architectural comparison against Tine's measured features.

| Capability / Metric | **Tine v0.1.0** (Measured) | **SteelSeries GG / Sonar** (Measured / Observed) | **Voicemeeter Banana / Potato** |
| :--- | :--- | :--- | :--- |
| **Idle Private Bytes (Window Closed)** | **3.9–5.5 MB** (`tine.exe` parent; `/DELAYLOAD` prevents RivaTuner 51 MB DXGI commit) | ~355 MB private (~650 MB WS across 5 processes; ~1.4 GB with GG window open) | Multi-component host + virtual audio engine + persistent UI |
| **Idle GPU VRAM (Window Closed)** | **0 MB** (D-017: `--ui` child process exits on close, releasing 100% of DX12/wgpu VRAM) | Persistent Chromium/Electron host keeps GPU surfaces allocated | GDI/windowed mixer stays resident when minimized to tray |
| **Idle CPU When No App Uses Mic** | **0.16 %** of one core (38 wakeups/s; `0 %` `audiodg.exe` cable work via event-driven WASAPI session notifications, D-034/D-039) | Idles when unused; **8.6–12.6 %** of one core + 1,000+ wakeups/s while mic is in use | Continuous audio engine polling / routing loop |
| **DSP Execution Time (480 samples @ 48 kHz)** | **~41.5 µs** (RNNoise default; **~17.3 µs** in `Off` Low-Latency mode; 0 RT heap allocations) | Closed-source multi-process pipeline | Closed-source studio routing engine |
| **Added DSP Latency @ 48 kHz** | **1.58 ms** (Low-Lat `Off`, 76 samples) · **21.58 ms** (RNNoise, 1036 samples) · **41.58 ms** (DeepFilterNet3 Studio Clean, 1996 samples) | Variable per Sonar streamer/mic chain | Buffer-size dependent + manual ASIO/WDM routing |
| **Mic Adaptation & Calibration** | **8s Auto-Tune Wizard** (3s room noise floor + 5s vocal reference + offline simulation) + 4 Hardware Archetypes | Manual EQ presets; no 8s room+voice self-calibration | Manual 3-band/15-band EQ & compressor; steep learning curve |
| **A/B Verification & Diagnostics** | **5s Sample-Aligned A/B Mic Check** modal + **Raw-Mic App Detector** banner + `Space` / `Ctrl+Shift+B` bypass | Live mic test; no raw-mic app detector | Manual routing toggles |
| **Push-to-Talk & Hotkeys** | **Keep Warm (PTT)** toggle (`tine ptt on`) + global `Ctrl+Shift+M` (click-free 5 ms RT mute) & `Ctrl+Shift+B` | Macro keybinds inside GG suite | MacroButtons companion app |
| **Telemetry / Account / Bloat** | **Zero** accounts, **zero** HTTP/telemetry code, MIT OR Apache-2.0, single `< 30 MiB` native binary | SteelSeries account prompts, ads/moments/clipping modules, closed source | Donationware nag screen (Potato), closed source |

---

## Section 2: Real Showcase Metrics Table

Generated reproducibly via `pwsh -File scripts/demo/build-showcase.ps1` (or `powershell -ExecutionPolicy Bypass -File scripts/demo/build-showcase.ps1`) running `cargo run --release -p tine-dsp --features deepfilter --example process_wav` against the repository's deterministic 48 kHz mono FLAC fixtures (`target/showcase/SHOWCASE-REPORT.md`).

| Showcase | Input Fixture | Preset / Denoiser | Latency | Raw Input | Tine Output | Max Gate GR | Max De-Esser GR | Max Comp GR | Pass-Through Latched |
| :--- | :--- | :--- | :--- | :--- | :--- | ---: | ---: | ---: | :--- |
| **Noisy Quiet (RNNoise)** | `noisy_quiet.flac` | `laptop` / `rnnoise` | 1036 samples (21.58 ms) | RMS `-32.75 dBFS`, peak `-14.96 dBFS` | RMS `-26.83 dBFS`, peak `-9.58 dBFS` (`+5.92 dB` RMS, `+5.39 dB` peak) | `-55.22 dB` | `0.00 dB` | `0.00 dB` | `false` |
| **Noisy Quiet (Studio Clean / DeepFilterNet3)** | `noisy_quiet.flac` | `laptop` / `deepfilter` | 1996 samples (41.58 ms) | RMS `-32.75 dBFS`, peak `-14.96 dBFS` | RMS `-27.97 dBFS`, peak `-9.55 dBFS` (`+4.78 dB` RMS, `+5.42 dB` peak) | `-60.00 dB` | `0.00 dB` | `0.00 dB` | `false` |
| **Dynamic Speech (RNNoise)** | `dynamic_speech.flac` | `dynamic` / `rnnoise` | 1036 samples (21.58 ms) | RMS `-18.00 dBFS`, peak `-3.59 dBFS` | RMS `-15.28 dBFS`, peak `-1.00 dBFS` (`+2.72 dB` RMS, `+2.59 dB` peak) | `-55.81 dB` | `0.00 dB` | `4.32 dB` | `false` |
| **Dynamic Speech (Low-Latency / Off)** | `dynamic_speech.flac` | `dynamic` / `off` | 76 samples (1.58 ms) | RMS `-18.00 dBFS`, peak `-3.59 dBFS` | RMS `-15.24 dBFS`, peak `-1.00 dBFS` (`+2.76 dB` RMS, `+2.59 dB` peak) | `-53.19 dB` | `0.00 dB` | `4.34 dB` | `false` |
| **Sibilant Condenser (RNNoise)** | `sibilant.flac` | `condenser` / `rnnoise` | 1036 samples (21.58 ms) | RMS `-19.81 dBFS`, peak `-4.14 dBFS` | RMS `-16.44 dBFS`, peak `-2.16 dBFS` (`+3.37 dB` RMS, `+1.98 dB` peak) | `-51.32 dB` | `3.00 dB` | `3.69 dB` | `false` |

### Generated Artifact SHA-256 Hashes

| Output WAV | Size (bytes) | SHA-256 |
| :--- | ---: | :--- |
| `target/showcase/noisy_quiet_rnnoise.wav` | 1,152,068 | `699da88cb54a43142ada96dc0e05767d9ce2ee6fa20546a1a0821973c2cff7ac` |
| `target/showcase/noisy_quiet_studio_clean.wav` | 1,152,068 | `f00cabb4e8091244a7931b7b94e2be22e01f732ab5f3cde734214abfc230c6aa` |
| `target/showcase/dynamic_speech_rnnoise.wav` | 1,152,068 | `ffa4b2df4f04dbe203dbeb1d986e59071711b18b34acd8792fb3d31b39990ac4` |
| `target/showcase/dynamic_speech_low_latency.wav` | 1,152,068 | `a16eabfc54bd2394866837d79e8a38626532130c9c3bc1ad2079cc82a163e289` |
| `target/showcase/sibilant_condenser.wav` | 768,068 | `587f1241d9b84449726fbd63a8d84bef04f23e161aeff87e040e29171237de7b` |

---

## Section 3: Ready-to-Post Community Launch Copy

### 3.1 Hacker News (`Show HN`)

**Title:**
```text
Show HN: Tine – Ultra-lightweight native Rust microphone DSP for Windows (<5MB idle RAM, 0 GPU)
```

**Body (first comment / text post):**
```text
Hi HN,

I built Tine (https://github.com/abugeek/tine), an open-source (MIT OR Apache-2.0) real-time microphone DSP engine and conditioner for Windows 10/11 written in Rust.

Why I built it:
Gaming suites like SteelSeries GG Sonar sit around ~355 MB private / ~650 MB working set with multiple background processes and account prompts, while Voicemeeter has a steep studio-mixer learning curve. I wanted a single native binary that makes cheap headsets, laptop mics, condensers, and dynamic XLR/USB mics sound clean and consistent in Discord, OBS, or games—without keeping a browser/GPU runtime alive in the tray.

How it works:
- 7-stage `no_std` mono f32 DSP chain (`tine-dsp`): Input Gain + DC/Butterworth HPF → Pluggable AI Denoiser (`nnnoiseless` RNNoise at 21.58 ms, embedded `dfn3_rs` DeepFilterNet3 "Studio Clean" at 41.58 ms, or `Off` "Low-Lat" at 1.58 ms) + VAD-assisted downward expander → 5-band TDF-II parametric EQ → split-band De-Esser → vocal compressor (with 110 Hz sidechain HPF) → 4x oversampled lookahead true-peak limiter (-1.0 dBFS).
- Zero-idle-GPU architecture: the always-on tray/audio parent runs headless and spawns `tine.exe --ui` as a separate short-lived child process over a named pipe. Closing the window terminates the child and frees 100% of DX12/wgpu VRAM.
- Measured footprint (Ryzen 7 9800X3D, Win11): 3.9–5.5 MB private bytes in the background parent, 0 MB GPU VRAM when closed, 0.16% CPU when no app is recording the virtual cable (event-driven WASAPI `IAudioSessionNotification` parks the audio loop), and ~41.5 µs per 480-sample frame @ 48 kHz.
- 8-Second Auto-Tune Wizard & Dual-Tier UI: 3s ambient noise floor + 5s speech calibration that tunes gate, denoiser wet mix, input gain, and compressor threshold/makeup via offline simulation—plus 4 "Golden Line" macro nodes or a full Pro rack with parameter pinning.
- Built-in 5s sample-aligned A/B Mic Check, raw-mic app detector banner (flags apps still reading your hardware mic instead of the virtual cable), global hotkeys (`Ctrl+Shift+M` click-free 5 ms mute, `Ctrl+Shift+B` bypass), and zero network/telemetry code.

Architectural note on Windows audio:
We initially prototyped an in-process COM APO inside `audiodg.exe`, proved stock Windows 11 24H2+ blocks unsigned third-party `FxProperties` APOs, and pivoted v0.1 to user-session routing into existing WHQL-signed virtual cables (VB-Audio Virtual Cable, Hi-Fi Cable, Cables A–D, VoiceMeeter VAIO/AUX, VAC).

Happy to answer questions about real-time Rust audio, WASAPI clock-drift PI resampling, or defeating GPU overlay DLL injections in tray apps!
```

---

### 3.2 `r/rust` (Architecture Deep Dive)

**Title:**
```text
Tine: A native Rust real-time microphone DSP for Windows (no_std lock-free chain, <5 MB idle private bytes, 0 idle VRAM)
```

**Body:**
```markdown
I just released **Tine v0.1.0** (`MIT OR Apache-2.0`), an open-source real-time microphone DSP engine and virtual-cable router for Windows written in Rust:
GitHub: https://github.com/abugeek/tine

A few Rust systems engineering details that might interest `r/rust`:

### 1. Strict `no_std` + allocation-free RT DSP (`tine-dsp` & `tine-protocol`)
- The 7-stage DSP pipeline (HPF, pluggable denoiser + VAD-assisted expander, 5-band TDF-II biquad EQ, split-band de-esser, vocal compressor with 110 Hz sidechain HPF, 4x oversampled lookahead limiter) is `no_std + alloc` (preallocated `Box<[f32]>` in `prepare()`, zero allocations during `process()`).
- Real-time paths deny `clippy::{unwrap_used, expect_used, panic, indexing_slicing, arithmetic_side_effects}` and are verified with `assert_no_alloc`, `proptest`, and `loom`/`miri` on our `#[repr(C)]` lock-free primitives (`Triple<T>`, `SpscRing`, generation-stamped `ParamBlock`).
- **IEEE-754 fast dB math (`tine_dsp::primitives::db`)**: replacing per-sample transcendental `log10`/`pow10` in dynamics detectors with IEEE-754 bit-manipulation (`fast_lin_to_db` / `fast_db_to_lin`, < 0.01 dB error) dropped the full 480-sample @ 48 kHz pipeline execution time to **~41.5 µs** (~17.3 µs with denoiser `Off` / 1.58 ms latency mode).
- If any stage ever produces `NaN`/`Inf`, the pipeline resets state and latches pass-through so a user's mic never blasts noise or goes silent.

### 2. Defeating RivaTuner's 51 MB injection & achieving 0 MB idle GPU VRAM
- **Process-split UI (D-017)**: One `tine.exe` binary hosts both the always-on tray/audio parent and the GPU-accelerated UI (`rune` + `wgpu` DX12) spawned as `tine.exe --ui` over a user-SID named pipe. Closing the window exits the child process and releases 100% of VRAM and window memory.
- **The `/DELAYLOAD` catch**: Even with the UI split out, we measured **58.2 MB** private bytes in the background parent on a machine with RivaTuner OSD installed! Bisecting in-process showed that statically importing `dxgi.dll` caused overlay hook DLLs to treat the tray window as a 3D app and commit 51 MB of private memory at `CreateWindowExW`.
- Adding MSVC `/DELAYLOAD` for `dxgi.dll`, `dwrite.dll`, `uiautomationcore.dll`, `dwmapi.dll`, and `imm32.dll` in `crates/tine-app/build.rs` keeps the parent completely clear of graphics DLLs (**3.9–5.5 MB** private bytes idle), loading them only inside the `--ui` child.

### 3. Event-driven WASAPI session gating & drift PI loop
- A `tine-listener-gate` thread watches the virtual cable's capture side with `IAudioSessionNotification` + `IAudioSessionEvents` (`OnStateChanged` / `OnSessionDisconnected`). When no app is recording the virtual mic, the routing loop parks (**0.16% CPU**, 0% `audiodg.exe` cable load) and wakes in ~0.25s on session start—or stays permanently hot with `Keep Warm (PTT)`.
- Clock drift between physical USB mic and virtual cable uses an `AdaptiveResampler` (`rubato`) with a 2x start-up prime drained explicitly at `+0.5%` before handing over to a PI controller (preventing integral windup underruns under scheduler jitter).

Would love feedback on the `tine-dsp` architecture, `assert_no_alloc` testing patterns, or Win32/WASAPI edge cases!
```

---

### 3.3 `r/obs`, `r/microphone`, and `r/pcgaming` (User-Focused Guide)

**Title (`r/obs` & `r/microphone`):**
```text
I built Tine: a tiny open-source Windows app that tunes any mic (dynamic, condenser, headset, laptop) in 8 seconds with <5 MB idle RAM
```

**Title (`r/pcgaming`):**
```text
Tired of 650 MB gaming audio bloatware? I built Tine – open-source mic DSP for Windows (<5 MB RAM, 0 idle GPU, 1.58 ms FPS mode)
```

**Body:**
```markdown
If you've ever fought with SteelSeries GG Sonar eating hundreds of megabytes of RAM/VRAM, or tried setting up Voicemeeter + VST chains just to get clean Discord/OBS mic audio, I built an open-source alternative called **Tine**:

**GitHub & Releases:** https://github.com/abugeek/tine

### What it does in 8 seconds
1. Pick your microphone archetype (**Dynamic USB/XLR**, **Studio Condenser**, **Gaming Headset**, or **Laptop/Webcam**) and click **Auto-Tune (8s)**.
2. Stay quiet for 3 seconds (measures your room noise floor) and speak normally for 5 seconds (measures vocal RMS & crest factor).
3. Tine automatically sets your noise gate, AI noise suppression mix, input gain, and vocal compressor/de-esser/limiter—then lets you verify the difference immediately with a built-in **5-second sample-aligned A/B Mic Check**.

### Why gamers, streamers, and mic nerds care
- **Practically zero footprint:** **3.9–5.5 MB** RAM in the tray, **0 MB GPU VRAM** when you close the window (the UI runs in a separate child process that completely exits on close), and **0.16% CPU** when no app is using your mic.
- **3 Denoiser Modes:**
  - **Low-Latency (`Off` — 1.58 ms added latency):** Gate + 5-band EQ + De-Esser + Compressor + Limiter with zero AI frame delay for competitive FPS comms.
  - **RNNoise (21.58 ms, default):** Crisp, lightweight everyday voice cleanup.
  - **Studio Clean (DeepFilterNet3, 41.58 ms):** Built-in studio-grade neural denoising for noisy rooms/fans without external plugins.
- **Keep Warm (PTT) + Global Hotkeys:** Turn on `Keep Warm` if you use Push-to-Talk so the first syllable is never clipped; press `Ctrl+Shift+M` anywhere for a click-free 5 ms mute or `Ctrl+Shift+B` for instant Raw vs. Tine A/B comparison.
- **Raw-Mic App Detector:** Warns you inside Tine if Discord, OBS, or a game is still reading your hardware mic directly instead of your virtual cable (`CABLE Output`, Hi-Fi Cable, VoiceMeeter VAIO/AUX, or VAC).
- **Zero spyware/accounts:** No internet access at all, MIT OR Apache-2.0 open source.

**Setup tip:** Tine runs in user-space without kernel drivers—install any virtual audio cable (like VB-Cable, or run `scripts/setup-virtual-cable.ps1`), pick your real mic inside Tine, and pick **CABLE Output** in Discord/OBS/Steam.
```

---

## Section 4: 15-Second `assets/demo.gif` & 60-Second YouTube A/B Recording Script

### 4.1 15-Second `assets/demo.gif` Capture Spec (for README)

Target specs from `README.md` (`1280×800`, `~15 s`, `< 5 MB`, `20–24 fps` crisp UI capture):

| Timestamp | Action on Screen | Visual Goal |
| :--- | :--- | :--- |
| **0.0s – 2.5s** | Show Tine window open (`1280×800`) on the **Golden Line** canvas with live 32-band voice spectrum moving as you speak into your mic. | Immediate visual hook: Ember palette, glowing response curve, live spectrum. |
| **2.5s – 6.5s** | Click **`Auto-Tune (8s)`** (or show the final 3 seconds of Voice Calibration → Results card) and click **Apply**. | Show how effortless the 8s calibration is—zero knob guesswork. |
| **6.5s – 10.5s** | Drag **Body** (`0.50 → 0.72`) and **Presence** (`0.50 → 0.68`) macro nodes smoothly; hover tooltip expands showing exact EQ/de-esser changes underneath. | Demonstrate the Golden Line macro philosophy + smooth spring animation. |
| **10.5s – 13.0s** | Click **`[Pro Controls ▾]`** to slide open the Surgical Pro Rack showing live Gate / De-Esser / Compressor GR meters dancing. | Prove Pro depth & real-time metering exist behind the simple surface. |
| **13.0s – 15.0s** | Briefly toggle **A/B Bypass** (`Space`) once and close the window; cut/overlay Task Manager showing `tine.exe` at **~4.5 MB** private bytes & **0% GPU**. | Land the core differentiator: zero idle UI/GPU footprint. |

---

### 4.2 60-Second YouTube A/B Recording Script

Use OBS Studio (1080p60) recording two audio tracks simultaneously (Track 1: Raw hardware mic; Track 2: `CABLE Output` processed by Tine) plus desktop video, then edit cleanly so viewers hear exact Raw vs. Tine transitions.

- **0:00 – 0:08 (Hook — Raw Mic):**
  > *"This is what my [budget headset / dynamic / laptop] mic sounds like raw in Windows—room noise, boxy low-mids, and zero vocal polish. And I am NOT installing a 650 MB gaming suite to fix it."*
  *(Show Task Manager / Resource Monitor briefly showing SteelSeries Sonar vs. Tine's ~4.5 MB RAM & 0 MB GPU).*
- **0:08 – 0:20 (Switch to Tine ON — Instant Difference):**
  > *"This is Tine—a free, open-source Rust DSP engine taking under 5 megabytes of RAM."*
  *(Toggle Tine Bypass OFF → ON while speaking; show the 32-band spectrum and Golden Line glowing).*
- **0:20 – 0:34 (The 8-Second Auto-Tune & A/B Mic Check):**
  > *"You don't need to know what a compressor knee or a 320 Hz mud cut is. Pick your mic type, click Auto-Tune, stay quiet for 3 seconds, talk for 5 seconds, and Tine calibrates the entire 7-stage chain to your room and voice. Then hit Mic Check to A/B test Raw versus Tine sample-aligned right inside the app."*
- **0:34 – 0:48 (3 Denoiser Modes + Pro Rack):**
  > *"Switch between 1.58-millisecond Low-Latency mode for FPS games, RNNoise for everyday Discord, or Studio Clean DeepFilterNet3 for loud fans—and if you want full surgical control, open the Pro Rack to pin any EQ, gate, de-esser, or compressor parameter."*
- **0:48 – 1:00 (Closing Window = 0 VRAM & Call to Action):**
  > *"Best part: close the window and the GPU process exits completely—zero VRAM used while you game. Links to GitHub and the Windows installer are in the description."*

---

## Section 5: SignPath Foundation Re-Application Checklist & Email Template

Per [D-041](../project/06-decisions.md), SignPath Foundation declined our initial 2026-11-20 OSS request strictly on public visibility signals (GitHub stars, contributors, external community references) and invited a re-application once Tine gains broader public adoption.

### 5.1 Pre-Submission Traction Milestone Checklist

Do **not** send the re-application email until every box below is checked and verifiable via public URLs:

- [ ] **GitHub Release `v0.1.0` published** (non-draft) at `https://github.com/abugeek/tine/releases/tag/v0.1.0` with:
  - [ ] `tine-0.1.0-x64.msi`, `tine-0.1.0-portable-win-x64.zip`, `tine-0.1.0-pdb-win-x64.zip`, and `SHA256SUMS.txt` attached
  - [ ] Cryptographic **GitHub Build Provenance Attestation** live (`gh attestation verify tine-0.1.0-x64.msi -R abugeek/tine` verified clean)
  - [ ] `assets/demo.gif` embedded in `README.md` + `SECURITY.md` contact email updated from placeholder
- [ ] **Package Manager Submissions Live:**
  - [ ] `microsoft/winget-pkgs` PR submitted/merged (`abugeek.tine` `0.1.0` — link: `https://github.com/microsoft/winget-pkgs/pull/XXXXX`)
  - [ ] Scoop bucket manifest live (`packaging/scoop/tine.json`)
- [ ] **Public Community Launch Links Active:**
  - [ ] Hacker News `Show HN` submission link: `https://news.ycombinator.com/item?id=XXXXXXXX`
  - [ ] `r/rust` architectural discussion link: `https://reddit.com/r/rust/comments/...`
  - [ ] `r/obs` / `r/microphone` / `r/pcgaming` user thread link(s): `https://reddit.com/r/.../comments/...`
- [ ] **Public Adoption Signals Recorded:**
  - [ ] GitHub Stars: `___` (target: ≥ 50–100+ stars)
  - [ ] GitHub Forks / Watchers / Downloads: `___` forks · `___` MSI/zip downloads
  - [ ] External issues / discussions / community beta reports / contributors: `___`

---

### 5.2 Direct Email Reply Template to Phillip (SignPath Foundation)

```text
Subject: Re: SignPath Foundation OSS Application — Tine (abugeek/tine)

Hi Phillip,

Thank you again for reviewing Tine earlier and for the clear guidance on reapplying once the project had stronger public visibility and community traction.

Since our initial application on November 20, we published Tine v0.1.0 (dual-licensed MIT OR Apache-2.0) with reproducible GitHub Actions builds and cryptographic GitHub Artifact Build Provenance attestations, and shared the release across the Windows, Rust, and audio communities.

Here are the updated public links and adoption milestones for your review:

- Repository: https://github.com/abugeek/tine
- v0.1.0 Release & Build Provenance: https://github.com/abugeek/tine/releases/tag/v0.1.0
  (Every artifact is built on GitHub Actions from tag v0.1.0 and verifiable via `gh attestation verify`)
- WinGet Package PR (`abugeek.tine`): <PASTE_WINGET_PR_URL>
- Community Discussions:
  - Hacker News (Show HN): <PASTE_HN_URL> (<POINTS> points, <COMMENTS> comments)
  - Reddit r/rust: <PASTE_R_RUST_URL>
  - Reddit r/obs / r/pcgaming: <PASTE_R_OBS_OR_PCGAMING_URL>
- Current Repository Metrics: <STARS> GitHub stars, <FORKS> forks, <DOWNLOADS> release downloads, <ISSUES_OR_PRS> community beta/issue reports.

Because Tine ships a native Windows desktop MSI and tray executable (`tine.exe`) that processes microphone audio in user space, Authenticode signing via SignPath Foundation would remove the Windows SmartScreen "Unknown publisher" friction for everyday Windows users adopting an open-source alternative to proprietary gaming suites.

Our GitHub Actions release workflow (`.github/workflows/release.yml`) is already wired for deterministic build-before-sign-before-hash verification (`scripts/verify-signing.ps1`) and SignPath's artifact-signing model.

Please let me know if there is any additional information or repository policy adjustment you need to reconsider Tine for SignPath Foundation!

Best regards,
Abdulloh
Maintainer, Tine (https://github.com/abugeek/tine)
```
