# Competitor resource benchmark (`scripts/bench`)

Measures Tine head-to-head against another mic/audio suite (default: SteelSeries
GG / Sonar) with nobody at the keyboard: idle, live voice path, windows open and
closed, game-like CPU load, saturated CPU and GPU load.

```powershell
# full run (~16 min, 60 s phases). Results in target\bench\<timestamp>\summary.md
powershell -ExecutionPolicy Bypass -File scripts\bench\compare.ps1

# quicker smoke run
powershell -ExecutionPolicy Bypass -File scripts\bench\compare.ps1 -PhaseSeconds 10 -SkipCompetitorUi

# repeated, alternating Tine on/off frame-pacing A/B (~8 min)
powershell -ExecutionPolicy Bypass -File scripts\bench\ab_game.ps1 -Rounds 5 -Seconds 40
```

Needs: Windows, Python 3 with `psutil`, `pywin32`, `numpy`; VB-Audio Virtual Cable;
the competitor installed and running. The script builds `tine.exe` and the bench
tools, measures the competitor as it finds it (never stops it), and restores Tine's
running state and "Hear Myself" setting at the end.

| File | Role |
|---|---|
| `compare.ps1` | Orchestrator: runs the phases, starts/stops Tine, loads, recorders |
| `sampler.py` | 1 Hz per-app sampler: CPU, working set, private bytes, threads, context switches, GPU engine %, VRAM |
| `cpu_stress.py` | Game-like CPU load (N spinning workers) + 144 Hz frame-pacing probe |
| `tools/` (`gpu-stress`) | wgpu compute shader keeping the GPU ~98 % busy; reports batches/s |
| `tools/` (`mic-record`) | Opens an app's virtual mic like Discord would and records it to WAV; counts stream errors and callback gaps |
| `audio_check.py` | Dropout (mid-signal digital-zero gaps) and click detector for the recordings |
| `tine_pipe.py` | Talks to the running `tine.exe` over its control pipe (`status`, `open`, `quit`, `monitor on/off`) |
| `report.py` | Turns a run folder into `summary.json` + `summary.md` |
| `ab_game.ps1` | Repeated alternating on/off A/B of the frame-pacing probe; `-Mode routing` (an app records Tine's mic in both arms) or `-Mode parked` (nobody listens, Tine parks) |
| `startup_underruns.ps1` | Starts/stops Tine N times and reports router underruns after each start |
| `drift_model.py` | Offline model of the playback jitter-buffer controller (why D-035 drains the start-up prime explicitly) |
| `tools/` (`sessions`) | Lists who records each capture endpoint (process id, active) |

`tools/` is deliberately **not** a workspace member and never ships; it builds into
`target/bench-tools`.

## Reading the numbers

- **CPU % core**: percent of one logical core (100 = one core fully busy), summed over
  all of an app's processes. Windows accounts CPU time in 15.6 ms ticks, so a single
  1 s sample moves in 1.56 % steps; use the phase mean.
- **audiodg**: the Windows audio engine. Work done for an app's streams (e.g. the
  virtual cable) lands there, not in the app. Compare deltas between phases.
- **Working set** counts shared pages once per process, so multi-process suites are
  somewhat overstated; **private MB** is commit charge (may be reserved but untouched).
- **Dropouts** only count when audible signal (> -60 dBFS) sits on both sides of an
  exact-zero gap. In a quiet room both apps' gates output silence, so treat the
  stream error / callback-gap columns and Tine's router counters as the primary
  glitch signals.

## Known limits

- No known-signal injection: with only one virtual cable, both apps process the real
  microphone. Speech-dependent checks (clicks, dropouts) need someone talking or a
  second cable.
- The competitor is never stopped, so its `audiodg` share is estimated from the
  idle → active delta.
- Launching `SteelSeriesGG.exe` while GG is already running does not always open its
  window; check the `procs` column of `competitor_ui_open`.
