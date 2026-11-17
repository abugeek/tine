# 01 — Vision: Why Tine Exists

## The problem

Most people talk through bad microphone chains every day: Discord, game voice chat, Zoom, OBS streams.
Their raw mic picks up keyboard clatter, fan noise, room echo, boomy low-mids, harsh "S" sounds and
wildly uneven levels (whisper → shout). Fixing that today means choosing between two bad options:

| Option | Why it fails ordinary users |
| :--- | :--- |
| **Vendor gaming suites** (SteelSeries GG Sonar, Razer Synapse, Logitech G HUB) | Heavy (hundreds of MB RAM, background services, Electron/CEF UIs), often tied to one brand's hardware, account sign-ins, telemetry. |
| **Studio-grade routing** (Voicemeeter Potato, Equalizer APO + Peace, OBS filter chains, VST hosts) | Powerful but assume audio-engineering knowledge: virtual patch bays, buses, gain staging, compressor ratios. Most users give up or make it worse. |
| **Built-in OS / app processing** (Discord Krisp, Windows Voice Focus) | One knob, opaque, per-app only, no tone shaping or leveling. |

## What Tine is

An **ultra-lightweight, native, open-source microphone conditioner** for Windows (first) that makes *any*
microphone sound broadcast-ready, for *every* app at once, with almost no setup and almost no resource cost.

## Who it is for

| Persona | Mic | What they want |
| :--- | :--- | :--- |
| **Competitive gamer** | Headset boom mic | Teammates hear them clearly, no keyboard noise, zero added lag, no CPU hit in-game. |
| **Streamer / content creator** | USB condenser (QuadCast, Yeti) or dynamic (SM7B, PodMic) | "Radio voice" in OBS and Discord without learning a DAW. |
| **Remote worker / student** | Laptop or webcam mic | Sound professional on calls in a noisy, echoey room. |
| **Audio-savvy power user** | Anything | Surgical control (exact EQ, compressor, limiter) without Voicemeeter's complexity. |

## How it fixes their problem

1. **Pick your mic type** (Dynamic / Condenser / Headset / Laptop) → a sensible starting curve for that hardware.
2. **Auto-Tune (8 s)** → 3 s of silence + 5 s of talking measures the room and voice and sets gate, noise
   removal, gain and compression automatically.
3. **Four macro knobs on one line** (Purity, Body, Presence, Leveler) for taste. A **Pro drawer** exposes
   every real parameter for experts. Both edit the same state, so nothing is hidden or duplicated.
4. **System-wide via virtual audio cable (D-018)**: Tine captures your physical microphone, runs the DSP
   pipeline, and outputs to a virtual audio cable (e.g. VB-Audio Virtual Cable `CABLE Input`). Discord, games,
   OBS and browsers pick `CABLE Output`.
5. **Invisible when closed**: closing the window exits the `tine.exe --ui` child process and frees all GPU
   memory; the lightweight tray parent remains (D-017).

## Product principles (in priority order)

1. **Never break the user's audio.** A crash, glitch or dropout in voice chat is worse than no processing.
   Real-time safety beats features.
2. **Sound good with zero knowledge.** Defaults + Auto-Tune must beat the raw mic for every archetype.
3. **Weightless.** Targets: < 12 MB working set (< 4 MB private bytes, hard 20 MB) and < 0.5 % CPU with the UI
   closed; 0 VRAM when the window is closed (D-017).
4. **Honest controls.** Every macro maps to real, inspectable parameters.
5. **Open and portable.** Open-source; Windows first, with DSP kept platform-neutral so macOS/Linux can follow.

## Non-goals (v0.1)

- Multi-track mixing, soundboards, music playback routing, voice changers/effects.
- Output-side (speaker/headphone) EQ.
- macOS / Linux packaging (DSP stays portable; platform integration comes later).
- Cloud accounts, telemetry, or any network access.

## Success criteria for v0.1

- A first-time user goes from install → better-sounding mic in Discord in **under 2 minutes**.
- 30-minute soak with zero dropouts; added DSP latency ≤ 21.6 ms at 48 kHz (1036 samples: RNNoise 480-sample FIFO + 480-sample OLA synthesis delay + 76-sample limiter lookahead/FIR, D-019).
- Blind A/B: listeners prefer Tine-processed audio for each of the 4 archetypes over raw.
