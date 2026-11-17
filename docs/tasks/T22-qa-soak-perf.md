# T22 — QA: Soak, Performance, Mic/App Matrix, Listening Tests

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: tine-worker) |
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
- [ ] **Soak:** `tine soak --minutes 30` and a 4-hour overnight run with Hear Myself on and Discord connected. Record:
  - heartbeat gaps > 50 ms
  - pass-through latches
  - underruns
  - `tine.exe` CPU, RSS and GPU memory every 10 s
  - audiodg CPU delta vs Tine uninstalled
- [ ] **Perf table:** every budget in standards §7, measured on the main machine. Low-end coverage (4-core, 8 GB laptops) comes from beta testers running `tine soak --minutes 15` and attaching the report (D-016).
- [ ] **Local mic matrix** (D-016): the dynamic mic and the gaming headset, each at every rate it offers. Result per row: install, sound, Auto-Tune, latency.
- [ ] **Community beta** (closes R-13), before the v0.1 tag:
  - publish `v0.1.0-beta.N` as a GitHub pre-release, with a "Beta testers wanted" Discussion
  - testers install, run onboarding + Auto-Tune, use Discord for 15 min, then attach `tine diag` output and a 1–5 sound rating to a "Beta report" issue template
  - **minimum coverage required to release:**
    - ≥ 2 condenser mics
    - ≥ 2 laptop/webcam mics
    - ≥ 1 Bluetooth headset
    - ≥ 2 Realtek or vendor-APO endpoints (Realtek, Nahimic, Dolby)
    - both Windows 10 22H2 and Windows 11 24H2+
  - every failure is triaged into an issue; blocking ones are fixed before release
- [ ] **App matrix:** Discord, OBS, Chrome (WebRTC), Zoom or Teams, CS2 voice, Windows Voice Recorder. Note apps using RAW or exclusive mode (R-08) and document them in the FAQ.
- [ ] **System events:**
  - sleep/resume, hibernate
  - mic unplug/replug, default device change
  - fast user switching
  - Windows Update simulation (delete FxProperties value)
  - driver reinstall
  - two Tine users on one machine
- [ ] **Failure injection:**
  - kill `tine.exe` during Auto-Tune (bypass lease expires ≤ 15 s)
  - corrupt settings or profile JSON
  - delete ProgramData boot params
  - stale shm writer claim
  - full disk while saving
- [ ] **Blind listening test:** raw vs Tine for each archetype fixture and one live recording per mic, ≥ 5 listeners, preference or ABX. Record the outcome.
- [ ] **Accessibility pass:** keyboard-only use of every flow; Narrator reads every control (roles and labels from the tree).

## Acceptance criteria
- 30-min and 4-h soaks: 0 dropouts, 0 latches, no memory growth.
- Every perf budget met, or a recorded decision explains the miss.
- Every local matrix row passes. The community-beta minimum coverage is met, and every beta failure is fixed or documented as a known limitation.
- Tine preferred over raw for every archetype in the blind test.

## Verification
```bash
tine soak --minutes 30 --report target/soak-30m.json
```
