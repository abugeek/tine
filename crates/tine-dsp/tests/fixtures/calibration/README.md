# Calibration fixtures (T11 — `AutoCalibrator`)

This directory is owned by task **T11 (AutoCalibrator)**. It is currently
**empty of media by design**: per the parent
[`README.md`](../README.md#dsp-test-fixtures) policy, synthetic signals that
tests need in specific shapes (silence, bursts, clipping tones, gain-stress
levels, the 9 s Auto-Tune capture layout) are **generated at test time** in
`crates/tine-dsp/tests/calibrator.rs` — deterministic generators from
`tests/common/mod.rs` plus the shared `tests/fixtures/noisy_quiet.flac`.

That means:

- No redundant FLAC capture lives here; the acceptance fixture
  (`noisy_quiet`, per T11's acceptance criteria) is read from
  `tests/fixtures/` and laid out into the 1 s countdown + 3 s noise + 5 s
  voice capture at test time.
- The failure-variant inputs (clipping, too quiet, not enough speech, too
  noisy, gain out of range) are all synthesized in-test so their exact
  measured levels (SNR, clip percentage, needed gain) can be asserted by the
  *same* math the calibrator uses.

Should a real-mic calibration capture be added later (see the D-16 follow-up
in the parent README), it belongs here, must follow the parent README's FLAC/
provenance/licence rules, and must keep every measured level noted.
