# T02 — Spike (Go/No-Go): Rust APO on a Capture Endpoint

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | B — Windows / APO |
| Depends on | T01 |
| Unblocks | T13, T14 |
| Owns | `spikes/apo-spike/**` (throwaway crate, excluded from the workspace default members) |
| Size | L |

## Goal
Prove or disprove [D-004](../project/06-decisions.md) with evidence. The proof is a minimal APO written in Rust (`windows` crate COM) that:
- `audiodg.exe` loads on a physical microphone endpoint of stock, fully updated Windows 11, without test-signing mode
- audibly processes audio (−12 dB gain)
- exposes a heartbeat to a user process
- survives a panic
- installs and uninstalls cleanly

Write down everything T13/T14 need to know.

## Read first
- [02-architecture.md](../project/02-architecture.md) §1, §4, §8 · [07-risks.md](../project/07-risks.md) R-01–R-08 · [09-engineering-standards.md](../project/09-engineering-standards.md) §5
- Microsoft Learn: "Audio Processing Object Architecture", "Implementing Audio Processing Objects", "Windows 11 APIs for Audio Processing Objects", `IAudioProcessingObject*` and `IAudioSystemEffects2/3` reference, `PKEY_FX_*` property keys.
- Read for background only (don't copy code): the Equalizer APO source (its registration, FxProperties ACL handling, and child-APO wrapping) · `dechamps/APO` notes · Microsoft `SwapAPO` sample (Windows-driver-samples / sysvad).

## Questions the spike must answer (write each answer in `## Result`)
1. Which interface set does audiodg require for a capture SFX/EFX on Win11 24H2+? Is `IAudioSystemEffects3` needed?
2. Which slot works for capture: SFX (stream) or EFX (endpoint)? What about endpoints that already have vendor effects?
3. Exact registry writes needed: CLSID, `AudioEngine\AudioProcessingObjects\{CLSID}` values, `FxProperties` PKEYs and their value format. Which processing modes (`PKEY_SFX_ProcessingModes_Supported_For_Streaming`)?
4. Who owns the `MMDevices\...\FxProperties` key and how do we get write access (take ownership, `SeRestorePrivilege`, TrustedInstaller ACL)?
5. Does audiodg load an **unsigned** DLL? If not, what signature is needed (Authenticode? specific EKU?)?
6. What sample formats/rates arrive in `LockForProcess` for USB, Realtek, and Bluetooth HFP mics?
7. How many APO instances are created per endpoint, and when (per stream, per mode)?
8. Can a `Global\` file mapping created in `Initialize` be opened read/write by a normal user process with our DACL?
9. What happens on a panic caught by `catch_unwind`, and what happens on a genuine crash (to know the blast radius)?
10. How do we force audiodg to reload the APO (endpoint disable/enable through `IPolicyConfig`? restart `audiosrv`?) and what does each do to running apps?
11. RAW-mode capture (e.g. some games/Discord settings): does it bypass us, and can we detect it?

## Safety first (no VM or spare PC is available, D-016) — do this before any install
- [ ] Create a System Restore point (`Checkpoint-Computer -Description "Before Tine APO spike"` in elevated PowerShell).
- [ ] Export the full capture registry: `reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture" %USERPROFILE%\tine-capture-backup.reg`.
- [ ] Write `spikes/apo-spike/recover.cmd` (pure `reg`/`sc` commands, no Rust needed) that:
  1. removes the spike CLSID and APO registration keys
  2. re-imports the backup `.reg`
  3. restarts `audiosrv`

  It must work from **Safe Mode with Command Prompt**. Test it once *before* the first install, with a dry run against a copy of the key.
- [ ] Only install on **one** endpoint at a time, and never on the endpoint you're using for voice chat at the moment. If audio dies: run `recover.cmd`; if that fails, boot to Safe Mode and run it; last resort, use the restore point.

## Steps
- [ ] `spikes/apo-spike` cdylib: class factory, `DllGetClassObject`, `DllCanUnloadNow`; COM object via `windows::core::implement` with the minimal interface set; `APOProcess` multiplies by 0.25 and bumps an atomic heartbeat in shm.
- [ ] `LockForProcess` writes the negotiated format to `%ProgramData%\TineSpike\format.log` (non-RT call).
- [ ] All entry points wrapped in `catch_unwind`; a registry flag (`HKLM\SOFTWARE\TineSpike\ForcePanic`), read in `LockForProcess`, triggers a panic in `APOProcess` for question 9.
- [ ] Spike CLI (`spikes/apo-spike/src/bin/apo-spike.rs`): `list` endpoints · `install <id>` (back up original values to JSON first) · `uninstall <id>` (restore exactly) · `heartbeat` (opens mapping, prints counter/s) · `reload <id>`.
- [ ] Run the test matrix on the hardware available (D-016): **dynamic mic** and **gaming headset** × {unsigned, enhancements on/off, RAW mode, each sample rate the device offers}. Record results in a table.
- [ ] Rows that can't be tested locally (Realtek jack with vendor APO, Bluetooth HFP, webcam/laptop array) are marked `untested — beta` and become mandatory rows of the T22 community-beta matrix. If a Windows laptop can be borrowed for an afternoon, run the spike there too (it covers Realtek + vendor APO + laptop mic).
- [ ] Write the T13/T14 design notes in `## Result`: interface set, slot, registry recipe, ACL procedure, reload procedure, vendor-wrapping approach.

## Acceptance criteria (GO)
- On the dynamic mic **and** the gaming headset on stock Win11 (no test signing): heartbeat increments, −12 dB audible in a recorder and in Discord.
- `recover.cmd` was proven to work before the first install.
- Uninstall restores the original registry state byte-for-byte (`reg export` diff), and audio still works.
- A forced panic leaves audiodg alive and audio passing through.
- All 11 questions answered; R-01/R-02 updated; new decisions recorded (slot, signing approach).

## NO-GO
Any of these means NO-GO:
- loading requires driver-package signing we can't get
- loading works only in test-signing mode
- there's no reliable registration path on Win11 24H2+

Then record a decision superseding D-004 and replan lane B (virtual-cable output from `tine-engine`).

## Verification
```bash
cargo build -p apo-spike --release
apo-spike list
apo-spike install "{0.0.1.00000000}.{guid}"
apo-spike heartbeat
apo-spike uninstall "{0.0.1.00000000}.{guid}"
```

## Out of scope
Real DSP, installer UX, production code quality (this crate is deleted after T13).

## Pitfalls
- A broken APO can silence all audio on your only machine, so follow the Safety section exactly. Keep a phone handy to read this file if the PC has no sound.
- audiodg runs in Session 0 as LocalService, so the mapping must use the `Global\` namespace. The APO creates it; user processes open it.
- The DLL must sit where LocalService can read it (e.g. `C:\Program Files\TineSpike\`), not under the user profile.
- Build the spike with the workspace release profile so the results apply to production.
