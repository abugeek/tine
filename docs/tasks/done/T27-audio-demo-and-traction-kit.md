# T27 — Reproducible Audio A/B Showcase Generator & Community Launch / SignPath Re-Application Playbook

| Field | Value |
| :--- | :--- |
| Status | done (`demo-showcase`) |
| Lane | Z — Integration & ship |
| Depends on | T23, T24, D-037..D-041 |
| Unblocks | v0.1.0 public launch & SignPath Foundation re-application |
| Owns | `scripts/demo/build-showcase.ps1`, `docs/tasks/LAUNCH-AND-SIGNPATH-PLAYBOOK.md`, `docs/tasks/done/T27-audio-demo-and-traction-kit.md` |
| Size | S |

## Goal

Provide a deterministic, one-command audio A/B showcase generator across Tine's built-in fixtures/presets/denoisers (`target/showcase/`) and a complete, copy-paste-ready launch and SignPath Foundation re-application playbook grounded in Tine's measured engineering numbers.

## Read first

[`AGENTS.md`](../../../AGENTS.md) · [`06-decisions.md`](../../project/06-decisions.md) (D-017, D-033, D-034, D-037–D-041) · [`T24-perf-competitor-followup.md`](../T24-perf-competitor-followup.md) · [`crates/tine-dsp/examples/process_wav.rs`](../../../crates/tine-dsp/examples/process_wav.rs)

## Steps

- [x] Create [`scripts/demo/build-showcase.ps1`](../../../scripts/demo/build-showcase.ps1) running `cargo run --release -p tine-dsp --features deepfilter --example process_wav` across all five showcase configurations, asserting `passthrough_latched: false`, computing SHA-256 hashes, and generating `target/showcase/SHOWCASE-REPORT.md`.
- [x] Execute `scripts/demo/build-showcase.ps1` and verify all 5 `.wav` outputs and `target/showcase/SHOWCASE-REPORT.md`.
- [x] Create [`docs/tasks/LAUNCH-AND-SIGNPATH-PLAYBOOK.md`](../LAUNCH-AND-SIGNPATH-PLAYBOOK.md) with all five sections: Measured Engineering Highlights Table, Real Showcase Metrics Table, Community Launch Copy (HN `Show HN`, `r/rust`, `r/obs` / `r/microphone` / `r/pcgaming`), 15s `assets/demo.gif` & 60s YouTube A/B Recording Script, and SignPath Foundation Re-Application Checklist & Email Template.
- [x] Record deliverables and measured showcase results in `docs/tasks/done/T27-audio-demo-and-traction-kit.md`.

## Acceptance criteria

- `scripts/demo/build-showcase.ps1` completes cleanly with exit code 0 and produces all 5 `.wav` files under `target/showcase/` plus `target/showcase/SHOWCASE-REPORT.md`.
- Every showcase run reports `passthrough_latched: false`.
- `docs/tasks/LAUNCH-AND-SIGNPATH-PLAYBOOK.md` contains all 5 sections with verified numbers matching `SHOWCASE-REPORT.md` and `docs/project/06-decisions.md` / `T24-perf-competitor-followup.md`.
- Only the 3 owned files are created/modified (zero `.rs` edits).

## Result

### Deliverables

1. [`scripts/demo/build-showcase.ps1`](../../../scripts/demo/build-showcase.ps1) — Reproducible showcase generator (PowerShell 5.1 & PowerShell 7 compatible). Runs `cargo run -q --release -p tine-dsp --features deepfilter --example process_wav`, parses `A/B:`, `Latency:`, `max_gate_gr_db`, `max_deesser_gr_db`, `max_comp_gr_db`, and `passthrough_latched`, enforces `passthrough_latched == false`, computes SHA-256 hashes, and writes `target/showcase/SHOWCASE-REPORT.md`.
2. [`docs/tasks/LAUNCH-AND-SIGNPATH-PLAYBOOK.md`](../LAUNCH-AND-SIGNPATH-PLAYBOOK.md) — Complete 5-section community launch and SignPath Foundation re-application playbook.
3. [`docs/tasks/done/T27-audio-demo-and-traction-kit.md`](T27-audio-demo-and-traction-kit.md) — This task record.

### Measured Verification Output

Executed `powershell -ExecutionPolicy Bypass -File scripts/demo/build-showcase.ps1` (`pwsh` is not installed on this host; Windows PowerShell 5.1 executed the script cleanly with exit code `0`):

| Showcase | Output WAV | Size (bytes) | SHA-256 | Latency | Raw -> Tine A/B | Gate / De-Esser / Comp GR | Pass-Through Latched |
| :--- | :--- | ---: | :--- | :--- | :--- | :--- | :--- |
| Noisy Quiet (`laptop` / `rnnoise`) | `target/showcase/noisy_quiet_rnnoise.wav` | 1,152,068 | `699da88cb54a43142ada96dc0e05767d9ce2ee6fa20546a1a0821973c2cff7ac` | 1036 samples (21.58 ms) | RMS `-32.75` -> `-26.83 dBFS` (`+5.92 dB`), peak `-14.96` -> `-9.58 dBFS` (`+5.39 dB`) | `-55.22 dB` / `0.00 dB` / `0.00 dB` | `false` |
| Noisy Quiet (`laptop` / `deepfilter`) | `target/showcase/noisy_quiet_studio_clean.wav` | 1,152,068 | `f00cabb4e8091244a7931b7b94e2be22e01f732ab5f3cde734214abfc230c6aa` | 1996 samples (41.58 ms) | RMS `-32.75` -> `-27.97 dBFS` (`+4.78 dB`), peak `-14.96` -> `-9.55 dBFS` (`+5.42 dB`) | `-60.00 dB` / `0.00 dB` / `0.00 dB` | `false` |
| Dynamic Speech (`dynamic` / `rnnoise`) | `target/showcase/dynamic_speech_rnnoise.wav` | 1,152,068 | `ffa4b2df4f04dbe203dbeb1d986e59071711b18b34acd8792fb3d31b39990ac4` | 1036 samples (21.58 ms) | RMS `-18.00` -> `-15.28 dBFS` (`+2.72 dB`), peak `-3.59` -> `-1.00 dBFS` (`+2.59 dB`) | `-55.81 dB` / `0.00 dB` / `4.32 dB` | `false` |
| Dynamic Speech (`dynamic` / `off`) | `target/showcase/dynamic_speech_low_latency.wav` | 1,152,068 | `a16eabfc54bd2394866837d79e8a38626532130c9c3bc1ad2079cc82a163e289` | 76 samples (1.58 ms) | RMS `-18.00` -> `-15.24 dBFS` (`+2.76 dB`), peak `-3.59` -> `-1.00 dBFS` (`+2.59 dB`) | `-53.19 dB` / `0.00 dB` / `4.34 dB` | `false` |
| Sibilant Condenser (`condenser` / `rnnoise`) | `target/showcase/sibilant_condenser.wav` | 768,068 | `587f1241d9b84449726fbd63a8d84bef04f23e161aeff87e040e29171237de7b` | 1036 samples (21.58 ms) | RMS `-19.81` -> `-16.44 dBFS` (`+3.37 dB`), peak `-4.14` -> `-2.16 dBFS` (`+1.98 dB`) | `-51.32 dB` / `3.00 dB` / `3.69 dB` | `false` |
