# 08 — Glossary

| Term | Meaning |
| :--- | :--- |
| **APO** | Audio Processing Object. A COM DLL loaded by the Windows audio engine (`audiodg.exe`). *(Tested in T02 and deferred post-v0.1 by D-018; v0.1 uses virtual-cable routing.)* |
| **Virtual cable** | A WASAPI virtual audio device (e.g. VB-Audio Virtual Cable: `CABLE Input` → `CABLE Output`) used by Tine v0.1 to deliver processed microphone audio to apps (D-018). |
| **SFX / MFX / EFX** | Stream / Mode / Endpoint effect slots for Windows APOs (unused in v0.1 under D-018). |
| **FxProperties** | Registry key under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{endpoint}` listing an endpoint's APO CLSIDs (unused in v0.1 under D-018). |
| **audiodg.exe** | Windows Audio Device Graph Isolation process; runs Windows APOs in Session 0 as LocalService (Tine v0.1 does not run inside `audiodg.exe`, D-018). |
| **Endpoint** | A Windows audio device as seen by apps (e.g. "Microphone (Shure MV7)"), identified by a GUID-based ID. |
| **RAW mode** | A capture stream option that asks Windows to skip APO processing. |
| **WASAPI** | Windows Audio Session API; what cpal uses on Windows. |
| **dBFS** | Decibels relative to digital full scale; 0 dBFS is the loudest possible sample. |
| **RMS / Peak** | Average energy vs. maximum instantaneous level. |
| **Crest factor** | Peak-to-RMS ratio (dB). Speech ≈ 12–20 dB. |
| **HPF / LPF** | High-pass / low-pass filter. |
| **Shelf / Peaking** | EQ shapes: shelf raises/lowers everything above/below a frequency; peaking acts on a bell around it. |
| **Q** | Bandwidth of a peaking filter (higher Q = narrower). |
| **Biquad** | 2nd-order IIR filter; building block of EQs. DF2T = Direct Form II Transposed. |
| **Expander / gate** | Reduces level when the signal is below a threshold (quiets room noise between words). Hysteresis prevents chattering. |
| **De-esser** | Compressor that acts only on sibilant frequencies (S, T, SH). |
| **Compressor** | Reduces dynamic range above a threshold by a ratio; *makeup gain* restores loudness; *knee* softens the transition. |
| **Limiter (lookahead)** | Compressor with ∞ ratio that sees the signal slightly ahead so it never lets peaks exceed the ceiling. |
| **GR** | Gain reduction, in dB. |
| **RNNoise / nnnoiseless** | Recurrent-neural-network noise suppressor; `nnnoiseless` is its pure-Rust port (480-sample, 48 kHz frames). |
| **Wet/Dry mix** | Blend between processed (wet) and unprocessed (dry) signal. |
| **One-pole smoother** | `y += (target − y) * (1 − e^(−1/(τ·fs)))`; removes zipper noise from parameter changes. |
| **Zipper noise** | Audible stepping when a parameter jumps between blocks. |
| **Denormals** | Tiny floats that are very slow on x86; avoided with FTZ/DAZ. |
| **SPSC ring** | Single-producer single-consumer lock-free queue. |
| **Triple buffer** | Lock-free scheme where a writer always has a free slot and a reader always gets the latest complete frame. |
| **Archetype** | Tine's mic category (Dynamic, Condenser, Headset, Laptop) that selects the base curve. |
| **Golden Line** | Tine's main view: one response line with 4 macro nodes. |
| **Macro** | One 0–1 control that drives several real parameters (Purity, Body, Presence, Leveler). |
| **Pin** | A parameter edited in Pro mode that macros no longer change. |
