# T24 — Competitor benchmark follow-up: stream health, idle cost, memory

| Field | Value |
| :--- | :--- |
| Status | review (implemented in the working tree; not committed) |
| Lane | Z — Integration & ship |
| Depends on | T22 |
| Unblocks | — |
| Owns | `scripts/bench/**` (new harness), `crates/tine-app/build.rs` (new), `crates/tine-engine/src/listeners.rs` (new), `crates/tine-engine/src/audio_io/{mod.rs,rt.rs,drift.rs}`, `crates/tine-engine/src/win.rs` (append: `CaptureSessionWatcher`), `crates/tine-engine/src/controller.rs` (`Command::Flush`, `flush_blocking`), `crates/tine-engine/src/virtual_cable.rs` (`stats().xruns`), `crates/tine-engine/src/lib.rs` (module line), `crates/tine-app/src/{services.rs,shell.rs,console.rs}` |
| Size | M |

## Goal

Act on the head-to-head resource benchmark of Tine vs SteelSeries GG / Sonar (scripts in `scripts/bench`):
fix every Tine defect it exposed and re-measure.

| # | Finding (first run) | Fix |
| :--- | :--- | :--- |
| 1 | Any `cpal` stream error, including the benign WASAPI data-discontinuity `Xrun`, latched the fatal flag: the router tore down and rebuilt twice per start (125 rebuilds in the log); `devhost record-raw` aborted; a Sonar-mic input could never route. | `audio_io` classifies errors (D-032): `Xrun`/`DeviceChanged`/`RealtimeDenied` are counted in `StreamStats::xruns`, only real failures latch. `xruns` is in `tine status`. |
| 2 | "Hear Myself" persisted as on was only started when the window opened; `status` claimed it was on. | The shell applies the persisted monitor once on the first `ModelChanged` after boot. |
| 3 | Background `tine.exe` held 58 MB private bytes (budget < 4 MB, hard 20 MB): one 51 MB commit made by an injected overlay hook DLL at the first window because `tine.exe` statically imported `dxgi.dll`. | `build.rs` delay-loads the window-stack DLLs (D-033); the parent never maps them. |
| 4 | Frame-pacing probe: more late frames at 144 Hz with Tine routing. | Root cause is inherent (MMCSS "Audio" threads, 10 ms period, plus `audiodg` cable work); no cheaper priority class exists. The listener gate (5) removes it whenever no app listens. |
| 5 | Tine processed audio while nobody recorded the virtual mic; Sonar idles. | Listener gate (D-034): park the routing loop after 10 s without a listener or internal consumer; resume on a listener within ~0.25 s, synchronously for Hear Myself / meters / Auto-Tune. |
| 5b | (found while testing 5) A router starting while an app already records the cable underran once in ~1 of 5 starts. | Start-up prime is 2x the steady target, drained explicitly at +0.5 % with the PI loop frozen (D-035). A first version that let the PI loop drain it wound up and caused 164 underruns 20–50 s after start; caught by the re-benchmark, modelled, fixed, regression-tested. |
| 6a | `tine status > file` / `| findstr` printed nothing (console rebind clobbered redirected handles). | Rebind only std slots without a pipe/file handle. |
| 6b | Window process ~105 MB WS. | Measured: dominated by NVIDIA driver images (shader compiler 110 MB, D3D12 UMD 89 MB resident, shared); Tine-unique 56 MB, all released on close. No app-side fix; documented. |
| — | (found while testing) A setting changed < 500 ms before `quit` was lost: the drop-triggered flush raced process exit. | `Command::Flush` + `ControllerHandle::flush_blocking`; the shell flushes before dropping handles. |

## Read first
[09-engineering-standards.md](../project/09-engineering-standards.md) §7 · [06-decisions.md](../project/06-decisions.md) D-018, D-027, D-032..D-035 · `scripts/bench/README.md`

## Steps
- [x] Harness: `compare.ps1` (phases incl. `tine_parked`), `ab_game.ps1 -Mode routing|parked`, `startup_underruns.ps1`, sampler, load generators, recorder, dropout checker, pipe client, report.
- [x] Fixes 1–6 and the two defects found while testing (table above), each with a unit test where the logic is testable without hardware.
- [x] Definition of done: fmt, clippy `-D warnings`, `cargo nextest run --workspace` (305 passed, 6 skipped).
- [x] Re-run the full benchmark and both A/B modes; record numbers below.

## Acceptance criteria
- 0 router rebuilds caused by benign xruns; `xruns` visible in `status`.
- Background `tine.exe` private bytes < 20 MB hard (target < 4 MB) with an overlay hook DLL present.
- Parked: ~0 % CPU, no `audiodg` cable work, resume ≤ 0.3 s with 0 underruns.
- 0 underruns over ≥ 8 cold starts with an active listener and ≥ 5 park/resume cycles.

## Verification
```powershell
cargo nextest run --workspace
powershell -ExecutionPolicy Bypass -File scripts\bench\compare.ps1
powershell -ExecutionPolicy Bypass -File scripts\bench\ab_game.ps1 -Mode routing
powershell -ExecutionPolicy Bypass -File scripts\bench\ab_game.ps1 -Mode parked
powershell -ExecutionPolicy Bypass -File scripts\bench\startup_underruns.ps1 -Starts 8
```

## Requests
- `scripts/` had no owner; this task owns `scripts/bench/**`.
- `crates/tine-engine/src/{controller.rs,win.rs,audio_io/*}` belong to done tasks T15/T12/T14; edited here as follow-up fixes (no frozen contract touched: `StreamStats` gained a field, `Command` gained a unit variant).

## Result

Machine: Ryzen 7 9800X3D (16 threads), RTX 5070, 32 GB, Windows 11; fifine USB mic; VB-Cable; SteelSeries GG/Sonar
running throughout; RivaTuner present. `compare.ps1` 60 s phases; run folders `target/bench/full` (before) and
`target/bench/final` (after). CPU = % of one core.

| Metric (background `tine.exe`) | Before | After | Budget |
| :--- | :--- | :--- | :--- |
| Private bytes | 58.2 MB | **3.9–5.5 MB** | < 4 MB target, 20 MB hard |
| Working set | 17.6–33.3 MB | **16.6–18.4 MB** | < 12 MB target, 20 MB hard |
| Router rebuilds per start (log) | 2 | **0** | — |
| Router underruns / overruns / xruns, all 9 routing phases incl. CPU saturation + GPU load | 0 / 0 / n.a. | **0 / 0 / 0** | 0 |
| Parked (no listener, Hear Myself off): CPU · wakeups/s · `audiodg` | n.a. (always routing: ~1 % · 310 · ~2 %) | **0.16 % · 38/s · 0 %** | — |
| Resume when an app opens `CABLE Output` | n.a. | **~0.25 s, 0 underruns in 5/5** | ≤ 0.3 s |
| Cold start with a listener already recording | 1 underrun in 5 | **0 in 8** | 0 |
| Hear Myself after boot | not running (status said on) | running at boot | — |
| `tine status > file` | empty | output captured | — |
| Window open (child) | +110 MB WS, 61 MB VRAM | +106 MB WS (~200 MB of resident NVIDIA driver images, shared; 56 MB Tine-unique), 61 MB VRAM, all freed on close | — |

Competitor (unchanged, for reference): Sonar 5 processes, ~650 MB WS / ~355 MB private with its window closed
(~1.4 GB with the GG window open), 8.6–12.6 % of a core while its mic is in use, 1,000+ wakeups/s under game load.

Game-impact A/B (`ab_game.ps1`, 5 alternating rounds × 40 s, 14 workers + 144 Hz frame probe):

| Mode | Late frames > 2 ms (median, on vs off) | p99 lateness (on vs off) | Game throughput |
| :--- | :--- | :--- | :--- |
| routing (an app records Tine's mic in both arms) | 36 vs 20 | 1.13 vs 0.86 ms | −1.1 % |
| parked (nobody listens) | 28 vs 29 | 1.03 vs 0.98 ms | ±0 % |

Routing cost is inherent to real-time audio (MMCSS "Audio" threads, 10 ms period, cable work in `audiodg`); it now
exists only while an app actually listens.

Regression caught by this task's own re-benchmark: the first D-035 version (2× prime drained by the PI loop) wound
up and produced 164–425 underruns per phase 20–50 s after start. Modelled (`scripts/bench/drift_model.py`), replaced by the explicit
drain, regression-tested (`startup_surplus_drains_without_windup_or_undershoot`), re-measured clean above.

Not done / open: no known-signal injection (one virtual cable); Sonar could not be stopped for an `audiodg`
baseline; the scripted GG window launch did not open it (figures from the initial snapshot); R-24 PTT first-syllable
risk to verify in the app matrix; working set still above the 12 MB stretch target (inside the 20 MB hard budget).
