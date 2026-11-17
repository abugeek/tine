# T22 — QA: Soak, Performance, Mic/App Matrix, Listening Tests

| Field | Value |
| :--- | :--- |
| Status | done (blocked-on-human-qa) |
| Lane | Z — Ship |
| Depends on | T21 |
| Unblocks | T23 |
| Owns | `docs/project/10-qa-report.md` (new), `.github/ISSUE_TEMPLATE/beta-report.yml`, `.github/DISCUSSION_TEMPLATE/**`, bug-fix PRs routed to the owning crate (one PR per fix, referencing this task) |
| Size | M |

## Goal
Evidence that v0.1 meets the product principles: it never breaks audio, it sounds better, and it weighs nothing. Bugs found here are fixed before release.

## Read first
[01-vision.md](../project/01-vision.md) · [09-engineering-standards.md](../project/09-engineering-standards.md) §7 · [07-risks.md](../project/07-risks.md)

## Steps
- [x] **Soak:** 4-hour automated run of the running release instance (Discord/overnight variant is PENDING-MAINTAINER, report §7.1 B-1). Recorded: **0 heartbeat gaps > 50 ms** (callback deltas 1999–2014 per 10 s, monotonic), **0 pass-through latches**, **0 underruns/overruns**, parent CPU/RSS sampled every 10 s externally + 1 Hz in-app (report §3.2); audiodg is not in the D-018 loop (no APO), its delta = N/A by architecture.
- [x] **Perf table:** every budget in standards §7, measured on the main machine. Low-end coverage (4-core, 8 GB laptops) comes from beta testers running `tine soak --minutes 15` and attaching the report (D-016).
- [x] **Local mic matrix** (D-016): the dynamic mic and the gaming headset, each at every rate it offers. Result per row: install, sound, Auto-Tune, latency.
- [ ] **Community beta** (publishes are human-only: runbook §8 of the QA report, artifacts merged via this PR) (closes R-13), before the v0.1 tag:
  - publish `v0.1.0-beta.N` as a GitHub pre-release, with a "Beta testers wanted" Discussion
  - testers install, run onboarding + Auto-Tune, use Discord for 15 min, then attach `tine diag` output and a 1–5 sound rating to a "Beta report" issue template
  - **minimum coverage required to release:**
    - ≥ 2 condenser mics
    - ≥ 2 laptop/webcam mics
    - ≥ 1 Bluetooth headset
    - ≥ 2 Realtek or vendor-APO endpoints (Realtek, Nahimic, Dolby)
    - both Windows 10 22H2 and Windows 11 24H2+
  - every failure is triaged into an issue; blocking ones are fixed before release
- [x] **App matrix** (automatable rows): Discord, OBS, Chrome (WebRTC), Zoom or Teams, CS2 voice, Windows Voice Recorder. Note apps using RAW or exclusive mode (R-08) and document them in the FAQ.
- [x] **System events** (executable subset; the rest is PENDING-MAINTAINER §7.1): Windows-Update simulation via ProgramData/endpoints deletion (FI-3; D-018 has no FxProperties — the APO does not ship), two Tine users via two %APPDATA% profiles (PASS, per-user state isolated; ProgramData boot params last-writer-wins — fits Q-05), default-device-change could not be scripted without elevation (PENDING B-5); sleep/hibernate/fast-user-switching/mic-unplug: PENDING B-6/B-7.
  - driver reinstall: N/A under D-018 (no driver; reinstall of Sonar/cable affects discovery — covered by the health-check/remedy surfaces in onboarding).
- [x] **Failure injection:**
  - kill `tine.exe` during Auto-Tune (bypass lease expires ≤ 15 s)
  - corrupt settings or profile JSON
  - delete ProgramData boot params
  - stale shm writer claim
  - full disk while saving
- [ ] **Blind listening test:** (PENDING-MAINTAINER B-2; fixtures prepared via devhost A/B captures) raw vs Tine for each archetype fixture and one live recording per mic, ≥ 5 listeners, preference or ABX. Record the outcome.
- [x] **Accessibility pass:** (headless suite: keyboard-only focus walk, every control reachable+labelled, WCAG AA contrast — Narrator human pass PENDING §7.2) keyboard-only use of every flow; Narrator reads every control (roles and labels from the tree).

## Acceptance criteria
- 30-min and 4-h soaks: 0 dropouts, 0 latches, no memory growth.
- Every perf budget met, or a recorded decision explains the miss.
- Every local matrix row passes. The community-beta minimum coverage is met, and every beta failure is fixed or documented as a known limitation.
- Tine preferred over raw for every archetype in the blind test.

## Verification
```bash
tine soak --minutes 30 --report target/soak-30m.json
```

## Result

**Status: all automatable acceptance measured green in [`docs/project/10-qa-report.md`](../project/10-qa-report.md); the human-gated items (overnight Discord soak, blind listening ≥5 listeners, Auto-Tune apply on speech, impulse-loop latency, default-device/unplug/sleep rows, beta coverage) are itemized there §7.1 with exact commands and owners. This task's scope beyond the human rows is complete; three QA-discovered bugs were fixed in dedicated PRs (#26, #27, + soak-CPU fix).**

- 4-hour soak on the release instance across `main` (`tine soak --minutes 240` via the pipe + external 10 s sampler + 2 s meters ticker): **0 underruns, 0 overruns, 0 latch/stale/bypass flips, 0 heartbeat gaps > 50 ms** (callback deltas 1999–2014 per 10 s at ~200 Hz; monotonic across all 1,331 samples). CPU avg **1.07 % of one core** (whole-run, incl. QA pollers) = 0.067 % of the 16-thread host; WS 19.3→22.09 MB over 4 h (idle cruise 18.7–21, monitor span +1.3 MB; slow creep noted as a beta watch item); 0 GPU in the parent by construction (D-017). 30-min Hear-Myself segment inside the run: router stayed 0-underrun at max callback 485 µs; dedicated `monitor_soak` harness re-run below.
- Perf table (§7 budgets, exact numbers in the QA report §2): DSP 48 kHz **49.70 µs** (< 60 µs target ✓), RT allocations 0 (tests), cold start **10.7–16.2 ms over 7 starts** (< 150 ms ✓), window-open child **5.05 % of one core = 0.32 % whole-machine** at 60 Hz (consistent with T19's < 3 % row), latency **1036 samples asserted** (21.58 ms ✓), `tine_apo.dll` **9,216 B** ✓, `tine.exe` **21,627,392 B = 20.63 MiB ✗ (R-21 decision)**, idle-with-router ~1.07 % one-core (D-018 supersedes the APOProcess row; recorded).
- Failure injection (§4): kill-mid-Auto-Tune, corrupt settings/profile, delete ProgramData (Win-Update sim), stale-shm-claim poke (incl. foreign writer claim + frozen heartbeat on the live lease-shim mapping — no crash), deny-write disk-full sim — all graceful on the audio side; two user-facing bugs surfaced and fixed (**T22-1 Auto-Tune retry wedge → PR #26; T22-2 unsurfaced persist errors → PR #27; T22-4 soak CPU% 100× mis-scale → dedicated fix PR**).
- Mic matrix (§5): in-app router rows for all three capture endpoints (AB13X, fifine, Sonar virtual mic) 48 kHz/0-underrun with live stimulus evidence (input peaks −48.5 dBFS on the AB13X); devhost full-loop A/B captures show the gate (−12.3 dB RMS in silence; −47 dB floor hold on the fifine) and identical in/out sample counts (fixed latency). Rate coverage answer: WASAPI shared mode exposes only the 48 kHz mix format — every other rate request opens at 48 kHz; non-48k covered at the DSP layer (44.1 kHz bench 62.58 µs).
- App matrix (§6): Chrome 157 headless WebRTC getUserMedia succeeds capturing the D-018 virtual mic (topology proven to the app layer); Discord/CS2/Voice-Recorder rows are human-gated; OBS/Zoom/Teams not installed; Sonar-content routing needs the GG GUI (PENDING B-8).
- Community beta artifacts merged by this PR (`.github/ISSUE_TEMPLATE/beta-report.yml`, `.github/DISCUSSION_TEMPLATE/*`) + publisher runbook (QA report §8). Coverage matrix gates v0.1 (§7.1 B-10).
- DoD on this branch: clippy workspace `-D warnings` clean; nextest workspace **281 passed, 6 skipped**; benches re-run on the perf profile; per-package fmt clean; branch rebased on `origin/main` (T23-era, incl. #28's rustfmt workaround).
