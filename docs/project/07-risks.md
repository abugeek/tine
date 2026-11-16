# 07 — Risks and Open Questions

## Risks

| ID | Risk | Impact | Likelihood | Mitigation / owner task |
| :--- | :--- | :--- | :--- | :--- |
| R-01 | Windows refuses to load a third-party, non-driver-packaged APO on a capture endpoint (protected audiodg, policy, "audio enhancements" off, Win11 driver-bundled APO model). | Blocks D-004 | Medium | **T02 spike first.** Equalizer APO proves the FxProperties route works on many systems; verify on Win11 24H2+ with USB, Realtek and Bluetooth mics. Fallback: VB-Cable route. |
| R-02 | Code signing: unsigned DLL/installer triggers SmartScreen; may be required for audiodg loading. | Install friction / blocker | Medium | Spike tests unsigned load. Investigate SignPath.io (free OSS signing) or Azure Trusted Signing. |
| R-03 | A bug in `tine_apo.dll` crashes `audiodg.exe` → **all system audio stops**. | Severe UX + reputation | Medium | `catch_unwind` at every COM boundary, no-panic lints, NaN latch, fuzz tests on `process()`, long soak tests, pass-through on any fault. |
| R-04 | Rune's `release_gpu_when_closed` leaves residual GPU/driver memory, or the windowless app + tray idle above budget. | Breaks "zero idle VRAM" | Low (Rune ships this lifecycle, D-011) | T03 measures it. Fallback: spawn the UI as `tine.exe --ui` child process (new decision superseding D-002). |
| R-05 | Vendor SFX/MFX APOs (Realtek, Nahimic, Dolby, NVIDIA Broadcast) already on the endpoint. | Lost vendor effects or double processing | High | Wrap the original APO CLSID (Equalizer APO pattern); store originals for clean uninstall. |
| R-06 | Windows Update / driver update resets FxProperties. | Tine silently stops | High | Startup + device-change health check; one-click repair. |
| R-07 | Endpoint shared-mode format is not 48 kHz / not mono. | Quality/latency | High | D-006 rate-agnostic DSP; downmix. |
| R-08 | Apps open the mic in **RAW mode** or exclusive mode (some games/DAWs) and bypass APOs. | Tine not heard in that app | Medium | Document; detect via the tap heartbeat not moving while the mic is in use? (best effort). |
| R-09 | `< 8 MB RSS` with the Rune runtime kept alive (state, text, layout caches) while the window is closed may be unrealistic. | Missed target | Medium | T03 measures real numbers; hard limit 20 MB; lazy-init everything not needed for the tray. |
| R-10 | Rune is pre-release, accepts no PRs, and requires consumers to mirror its `[patch.crates-io]` (patched winit/wgpu-hal/parley/accesskit). | Breaking changes, build friction | Medium | Pin rev (D-012); bump Rune only in dedicated PRs that re-run headless snapshots and T03 checks; keep view state in plain structs; report bugs as Rune issues. |
| R-11 | RNNoise quality on music/singing or non-speech sounds; artifacts at high mix. | Quality complaints | Low | Mix control, Purity defaults ≤ 0.85. |
| R-12 | Shared memory DACL too permissive (any local process can change mic processing). | Local tampering | Low | Grant only interactive users + LocalService; validate/clamp every param in the APO. |
| R-13 | Limited test hardware (D-016): no condenser, laptop/webcam, Bluetooth, Realtek-jack or vendor-APO endpoint, and no spare machine/VM for the APO spike. | Bugs reach users on untested paths; a broken APO spike could silence the main PC | High | Restore point + registry export + Safe-Mode recovery script before T02; proxy fixtures; `tine diag`; community beta gate in T22 with a minimum coverage matrix. Borrowing a Windows laptop for one afternoon would cover laptop mic + Realtek + vendor APOs. |

## Open questions (answer in chat, then record as decisions)

| ID | Question | Recommendation |
| :--- | :--- | :--- |
| ~~Q-01~~ | ~~License?~~ | **Resolved → D-015: `MIT OR Apache-2.0`.** |
| Q-02 | Project name/branding final? Any logo/tray icon assets? | Placeholder monochrome tuning-fork icon. |
| Q-03 | Minimum Windows version? | Windows 10 22H2 and Windows 11; test matrix focuses on Win11 24H2+. |
| Q-04 | Installer tech? | WiX v5 MSI (per-machine, needed for APO + ProgramData), or Inno Setup if simpler. |
| Q-05 | Should Tine process multiple mics simultaneously? | v0.1: one active endpoint; APO may be installed on several but only one is "managed". |
| Q-06 | Auto-update? | v0.1: "new version available" check against GitHub Releases, opt-in, no auto-install. |
| ~~Q-07~~ | ~~Which real microphones are available for testing?~~ | **Resolved → D-016:** dynamic USB/XLR + gaming headset; proxies and community beta for the rest. |
