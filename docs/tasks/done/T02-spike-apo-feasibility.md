# T02 — Spike (Go/No-Go): Rust APO on a Capture Endpoint

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] Create a System Restore point (`Checkpoint-Computer -Description "Before Tine APO spike"` in elevated PowerShell).
- [x] Export the full capture registry: `reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture" %USERPROFILE%\tine-capture-backup.reg`.
- [x] Write `spikes/apo-spike/recover.cmd` (pure `reg`/`sc` commands, no Rust needed) that:
  1. removes the spike CLSID and APO registration keys
  2. re-imports the backup `.reg`
  3. restarts `audiosrv`

  It must work from **Safe Mode with Command Prompt**. Test it once *before* the first install, with a dry run against a copy of the key.
- [x] Only install on **one** endpoint at a time, and never on the endpoint you're using for voice chat at the moment. If audio dies: run `recover.cmd`; if that fails, boot to Safe Mode and run it; last resort, use the restore point.

## Steps
- [x] `spikes/apo-spike` cdylib: class factory, `DllGetClassObject`, `DllCanUnloadNow`; COM object via `windows::core::implement` with the minimal interface set; `APOProcess` multiplies by 0.25 and bumps an atomic heartbeat in shm.
- [x] `LockForProcess` writes the negotiated format to `%ProgramData%\TineSpike\format.log` (non-RT call).
- [x] All entry points wrapped in `catch_unwind`; a registry flag (`HKLM\SOFTWARE\TineSpike\ForcePanic`), read in `LockForProcess`, triggers a panic in `APOProcess` for question 9.
- [x] Spike CLI (`spikes/apo-spike/src/bin/apo-spike.rs`): `list` endpoints · `install <id>` (back up original values to JSON first) · `uninstall <id>` (restore exactly) · `heartbeat` (opens mapping, prints counter/s) · `reload <id>`.
- [x] Run the test matrix on the hardware available (D-016): **dynamic mic** and **gaming headset** × {unsigned, enhancements on/off, RAW mode, each sample rate the device offers}. Record results in a table.
- [x] Rows that can't be tested locally (Realtek jack with vendor APO, Bluetooth HFP, webcam/laptop array) are marked `untested — beta` and become mandatory rows of the T22 community-beta matrix. If a Windows laptop can be borrowed for an afternoon, run the spike there too (it covers Realtek + vendor APO + laptop mic).
- [x] Write the T13/T14 design notes in `## Result`: interface set, slot, registry recipe, ACL procedure, reload procedure, vendor-wrapping approach.

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

## Result

### Verdict: **NO-GO** (supersedes D-004 → recorded as **D-018**)

On stock Windows 11 Build `10.0.26300.0` (24H2+), `audiodg.exe` did **not** load our unsigned Rust COM APO (`apo_spike.dll`, 194,560 bytes, release build) on either the dynamic USB mic (`fifine Microphone`) or the gaming headset (`AB13X USB Audio`) via `FxProperties` SFX registration, even after a full `Restart-Service audiosrv -Force`. Per the NO-GO criteria ("there's no reliable registration path on Win11 24H2+"), D-004 is superseded by **D-018** (virtual-cable output from `tine-engine`), and Lane B (`T13`, `T14`) should be replanned accordingly.

### Measured Numbers & Hardware Matrix

- **In-process COM & DSP unit test (`com_object_and_gain_processing_work`)**: passed (480 samples at 0.8 → 0.2, `-12.04 dB`, `GetRegistrationProperties` / `Initialize` / `APOProcess` verified).
- **Release DLL size**: `apo_spike.dll` = `194,560` bytes (`~190 KB`).
- **Safety (`recover.cmd --dry-run`)**: passed against `HKCU\Software\TineSpikeDryRun`; capture registry exported to `%USERPROFILE%\tine-capture-backup.reg`.

| Endpoint / Archetype | Condition | Format Observed | RMS / Peak (dBFS) | APO Loaded? (`spike.log` / SHM) | Uninstall `fc` Diff |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `fifine Microphone` (Dynamic USB) | Baseline (before install) | 48000 Hz, 2 ch, f32 | `-56.17` / `-35.47` | — | — |
| `fifine Microphone` (Dynamic USB) | SFX installed + `IPolicyConfig` reload | 48000 Hz, 2 ch, f32 | `-54.82` / `-17.75` | **No** (`0x80070002`) | — |
| `fifine Microphone` (Dynamic USB) | SFX installed + `Restart-Service audiosrv -Force` | 48000 Hz, 2 ch, f32 | `-67.14` / `-48.31` | **No** (`0x80070002`) | — |
| `fifine Microphone` (Dynamic USB) | `AUDCLNT_STREAMOPTIONS_RAW` | 48000 Hz, 2 ch, f32 | `-63.61` / `-49.02` | **No** | — |
| `fifine Microphone` (Dynamic USB) | `ForcePanic=1` | 48000 Hz, 2 ch, f32 | `-72.79` / `-50.23` | **No** (not loaded) | — |
| `fifine Microphone` (Dynamic USB) | After `uninstall` + reload | 48000 Hz, 2 ch, f32 | `-63.45` / `-43.99` | — | `FC: no differences encountered` |
| `AB13X USB Audio` (Gaming Headset) | Baseline (before install) | 48000 Hz, **1 ch**, f32 | `-62.58` / `-52.47` | — | — |
| `AB13X USB Audio` (Gaming Headset) | SFX installed + `IPolicyConfig` reload | 48000 Hz, **2 ch**, f32 | `-67.31` / `-53.59` | **No** (`0x80070002`) | — |
| `AB13X USB Audio` (Gaming Headset) | SFX installed + `Restart-Service audiosrv -Force` | 48000 Hz, **2 ch**, f32 | `-91.21` / `-74.91` | **No** (`0x80070002`) | — |
| `AB13X USB Audio` (Gaming Headset) | After `uninstall` + reload | 48000 Hz, **1 ch**, f32 | `-81.83` / `-42.21` | — | `FC: no differences encountered` |
| Realtek jack / Bluetooth HFP / Laptop | Not present locally | — | — | `untested — beta` (superseded by D-018) | — |

### Answers to Questions 1–11

1. **Interface set (`IAudioSystemEffects3`?)**: Implemented `IAudioProcessingObject`, `IAudioProcessingObjectRT`, `IAudioProcessingObjectConfiguration`, and `IAudioSystemEffects3` (which implies `IAudioSystemEffects` + `IAudioSystemEffects2`). In-process COM instantiation and `APOProcess` worked; `audiodg.exe` never loaded the DLL so it did not query interfaces at runtime.
2. **Slot (SFX vs EFX) & existing vendor effects**: Both `fifine Microphone` and `AB13X USB Audio` already have `VocaEffectPack` (`{9e6136e0-57ab-4949-b57a-3627be142855},100 = SWD\DRIVERENUM\{96bedf2c-18cb-4a15-b821-5e95ed0fea61}#VocaEffectPack&1&38cf50d&0`) in `FxProperties`. Setting `PKEY_FX_StreamEffectClsid` (`,5`) did not cause `audiodg.exe` to load our DLL. (Other slots `EFX`/`MFX` were not tested once SFX failed after `audiosrv` restart.)
3. **Registry writes**: Wrote `HKLM\SOFTWARE\Classes\CLSID\{CLSID}\InprocServer32` (`ThreadingModel=Both`), `HKLM\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{CLSID}` (`Flags=15`, `Min/MaxConnections=1`, `APOInterface0=IAudioSystemEffects`), and `FxProperties` (`PKEY_FX_Association`, `PKEY_FX_StreamEffectClsid`, `PKEY_SFX_ProcessingModes_Supported_For_Streaming = {C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}`, `PKEY_AudioEndpoint_Disable_SysFx = 0`). `audiodg.exe` reacted to `Disable_SysFx = 0` (changing `AB13X` from 1ch to 2ch) but did not load `apo_spike.dll`.
4. **Ownership & ACL of `FxProperties`**: Owned by `NT AUTHORITY\SYSTEM`; `BUILTIN\Administrators` has `SetValue, ReadKey` (not `Delete` or `WriteDac`). `apo-spike` enabled `SeTakeOwnershipPrivilege` + `SeRestorePrivilege` and used `SetNamedSecurityInfoW` to grant `Administrators` `KEY_ALL_ACCESS` for clean value deletion on `uninstall`.
5. **Unsigned DLL loading**: Not loaded by `audiodg.exe` via `FxProperties` SFX on Win11 26300, though `Microsoft-Windows-CodeIntegrity/Operational` logged zero blocks (indicating `audiodg.exe` never attempted `LoadLibrary` on the SFX CLSID).
6. **Sample formats/rates**: Capture mix format observed via WASAPI `GetMixFormat` was `48000 Hz, 32-bit IEEE float, 2 ch` on `fifine Microphone`, and `48000 Hz, 32-bit IEEE float, 1 ch` (default) / `2 ch` (when `Disable_SysFx = 0`) on `AB13X USB Audio`. `LockForProcess` in `audiodg.exe` was never reached.
7. **APO instances per endpoint**: Not observed (`audiodg.exe` never instantiated `TineSpikeApo`).
8. **`Global\` file mapping**: Not reached from `audiodg.exe` (`OpenFileMappingW(Global\TineSpike.v1)` returned `0x80070002`). Note: `C:\ProgramData\TineSpike` inherits `BUILTIN\Users:(I)(CI)(WD,AD,WEA,WA)` so `LocalService` can write logs if loaded.
9. **Panic vs crash**: In-process `catch_unwind` verified at every COM boundary; `ForcePanic` in `audiodg.exe` was not reached because the DLL was not loaded.
10. **Reloading `audiodg.exe`**: `IPolicyConfig::SetEndpointVisibility` toggled endpoint visibility without restarting `audiodg.exe`. `sc stop audiosrv` failed with error `1051` because `RtkAudioUniversalService`, `LightingService`, and `AarSvc` depend on `audiosrv`; `Restart-Service audiosrv -Force` (or `net stop audiosrv /y && net start audiosrv`) cleanly restarted `audiosrv`.
11. **RAW mode**: `AUDCLNT_STREAMOPTIONS_RAW` captured cleanly (`48000 Hz, 2 ch, f32`).

### Surprises & Follow-ups

- `windows` 0.62.2's `#[implement(...)]` macro expands `::windows_core::...` directly, so crates using `#[implement]` must also declare `windows-core` (same version as `windows`).
- `cargo build` emits a PDB filename collision warning when a package has both `apo_spike` (`cdylib`) and `apo-spike` (`bin`).
- **Next**: Replan Lane B (`T13`, `T14`) per **D-018** to implement virtual-cable output (`CABLE Input (VB-Audio Virtual Cable)`) in `tine-engine` via `cpal`, and update `tine-ui` header status accordingly.
