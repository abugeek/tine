//! T11 — `AutoCalibrator` integration tests.
//!
//! Covers: phase state machine + progress, allocation-free `push`,
//! the `noisy_quiet` fixture acceptance (gate = noise+6, denoise 0.85,
//! simulated peaks in `-6..-3 dBFS`), bit-exact equality of the offline
//! verification against the real `DspPipeline`, determinism (chunk-agnostic,
//! identical input → identical report), every failure variant with its
//! payload, non-48 kHz capture, and the `finish() < 50 ms` perf budget
//! (ignored; run under a release/perf profile).

#![allow(clippy::missing_docs_in_private_items)]

mod common;

use std::path::PathBuf;
use std::time::Instant;

use assert_no_alloc::AllocDisabler;
use common::{peak_db, pink_noise, read_flac_mono, rms_db, sine, white_noise};
use tine_dsp::calibrator::{
    AutoCalibrator, CHANGED_PARAM_COUNT, CHANGED_PARAMS, CalFailure, CalPhase, CalibrationReport, MAX_INPUT_GAIN_DB,
    MIN_VOICED_MS, PEAK_TARGET_HIGH_DB, PEAK_TARGET_LOW_DB, TARGET_SPEECH_RMS_DB,
};
use tine_dsp::macros::MacroMapper;
use tine_dsp::presets::{PresetDef, builtin};
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::PinSet;

/// Asserting allocation-freedom selectively: the global allocator denies
/// allocations only inside `assert_no_alloc(...)` closures, so fixture
/// loading etc. outside those closures still works.
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const FS: usize = 48_000;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The dynamic preset base gate the diff `before` values anchor on.
const DYNAMIC_BASE_GATE_DB: f32 = -50.0;

/// Ratio threshold for sine clipping tests (`10^(-0.3/20)`).
fn clip_threshold_lin() -> f32 {
    10f32.powf(-0.3 / 20.0)
}

/// Pushes `stream` in `block`-sized chunks (plus one empty-push edge case),
/// returning the final progress.
fn capture(cal: &mut AutoCalibrator, stream: &[f32], block: usize) -> tine_dsp::calibrator::CalProgress {
    assert!(block > 0);
    let mut last = cal.push(&[]);
    for chunk in stream.chunks(block) {
        last = cal.push(chunk);
    }
    last
}

/// The 9 s Auto-Tune capture built from the `noisy_quiet` fixture:
/// 1 s discard (fixture noise), 3 s room noise (pink, -45 dBFS),
/// 5 s quiet speech (fixture speech at -30 dBFS, last 2 s repeated).
fn noisy_quiet_stream() -> Vec<f32> {
    let (file, fs) = read_flac_mono(&fixture_path("noisy_quiet.flac"));
    assert_eq!(fs as usize, FS, "fixture must be 48 kHz");
    assert!(file.len() >= 6 * FS, "fixture too short");
    let speech_at = 3 * FS;
    let mut stream = file.get(..4 * FS).expect("countdown+noise").to_vec();
    let speech = file.get(speech_at..).expect("speech tail");
    stream.extend_from_slice(speech);
    let repeat = file.get(speech_at..speech_at + 2 * FS).expect("speech head");
    stream.extend_from_slice(repeat);
    assert_eq!(stream.len(), 9 * FS);
    stream
}

/// The voice segment (last 5 s) of a 9 s calibration stream.
fn voice_of(stream: &[f32]) -> &[f32] {
    stream.get(4 * FS..).expect("voice region")
}

/// Reference measurement: median over `win`-sample window RMS (linear-power
/// median, like the implementation) in dB.
fn median_window_rms_db(x: &[f32], win: usize) -> f32 {
    let mut powers: Vec<f64> = x
        .chunks_exact(win)
        .map(|w| {
            let sum: f64 = w.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
            sum / (w.len() as f64)
        })
        .collect();
    powers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = powers.len() / 2;
    let median = if powers.len() % 2 == 1 {
        powers.get(mid).copied().unwrap_or(0.0)
    } else {
        f64::midpoint(
            powers.get(mid - 1).copied().unwrap_or(0.0),
            powers.get(mid).copied().unwrap_or(0.0),
        )
    };
    tine_dsp::primitives::db::lin_to_db((median.sqrt()) as f32)
}

/// Runs a full capture + finish against the dynamic preset.
fn calibrate(stream: &[f32], block: usize) -> Result<CalibrationReport, CalFailure> {
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let mut cal = AutoCalibrator::new(FS as f32);
    capture(&mut cal, stream, block);
    cal.finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
}

// -------------------------------------------------------------- state machine

#[test]
fn phases_advance_with_progress() {
    let mut cal = AutoCalibrator::new(FS as f32);
    let zeros = vec![0.0f32; 4 * FS];

    // 0.5 s in: half of the countdown phase.
    let p = cal.push(zeros.get(..FS / 2).expect("0.5 s"));
    assert_eq!(p.phase, CalPhase::Countdown);
    assert!((p.phase_progress - 0.5).abs() < 1e-3);

    // Countdown complete -> exactly into noise.
    let p = cal.push(zeros.get(FS / 2..FS).expect("0.5 s"));
    assert_eq!(p.phase, CalPhase::Noise);
    assert_eq!(p.phase_progress, 0.0);

    // Mid-noise.
    let p = cal.push(zeros.get(FS..FS * 5 / 2).expect("1.5 s"));
    assert_eq!(p.phase, CalPhase::Noise);
    assert!((p.phase_progress - 0.5).abs() < 0.02);

    // Noise complete -> exactly into voice.
    let p = cal.push(zeros.get(FS * 5 / 2..FS * 4).expect("1.5 s"));
    assert_eq!(p.phase, CalPhase::Voice);
    assert_eq!(p.phase_progress, 0.0);

    // Mid-voice: silence is never voiced.
    let p = cal.push(zeros.get(..FS * 5 / 2).expect("2.5 s"));
    assert_eq!(p.phase, CalPhase::Voice);
    assert!((p.phase_progress - 0.5).abs() < 0.02);
    assert_eq!(p.voiced_ms, 0, "silence is never voiced");

    // One oversized push crosses the remaining voice phase -> Analyzing, and
    // further pushes are inert while capture is complete.
    let p = cal.push(&vec![0.0f32; 9 * FS]);
    assert_eq!(p.phase, CalPhase::Analyzing);
    assert_eq!(p.phase_progress, 0.0);
    let p = cal.push(&[0.0; 480]);
    assert_eq!(p.phase, CalPhase::Analyzing);

    // Terminal phases report progress 1.0 and absorb nothing: the capture
    // was all silence, so finish() rejects it as TooQuiet.
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let failure = cal.finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT);
    assert!(matches!(failure, Err(CalFailure::TooQuiet)), "silence: {failure:?}");
    let p = cal.push(&[0.9; 480]);
    assert_eq!(p.phase, CalPhase::Failed);
    assert_eq!(p.phase_progress, 1.0);
}

#[test]
fn live_voiced_ms_counts_during_voice_phase() {
    let stream = noisy_quiet_stream();
    let mut cal = AutoCalibrator::new(FS as f32);
    let mut last = cal.push(&[]);
    for chunk in stream.chunks(480) {
        last = cal.push(chunk);
    }
    assert_eq!(last.phase, CalPhase::Analyzing);
    assert!(
        last.voiced_ms >= MIN_VOICED_MS,
        "fixture speech must be detected as >= {MIN_VOICED_MS} ms voiced (got {})",
        last.voiced_ms
    );
}

#[test]
fn push_is_allocation_free() {
    let stream = noisy_quiet_stream();
    let mut cal = AutoCalibrator::new(FS as f32);
    // 123-sample blocks: never frame-aligned, crossing window and phase
    // boundaries at odd offsets; includes the zero-length edge case.
    assert_no_alloc::assert_no_alloc(|| cal.push(&[]));
    for chunk in stream.chunks(123) {
        assert_no_alloc::assert_no_alloc(|| cal.push(chunk));
    }
    assert!(matches!(cal.push(&[0.0; 480]).phase, CalPhase::Analyzing));
}

// ------------------------------------------------------- noisy_quiet fixture

#[test]
fn noisy_quiet_fixture_meets_acceptance() {
    let stream = noisy_quiet_stream();
    let report = calibrate(&stream, 480).expect("fixture calibration must succeed");

    let cal = &report.calibration;
    let noise_region = stream.get(FS..4 * FS).expect("noise region");

    // Gate threshold = measured noise RMS + input gain + 6 dB - 4 * denoise_mix
    // (clamped to SPECS, ± 0.5 dB), with noise_rms independently measured as
    // the median of 100 ms window RMS values.
    let noise_rms = median_window_rms_db(noise_region, FS / 10);
    assert!(
        (noise_rms - (-45.0)).abs() < 3.0,
        "fixture noise level sanity: {noise_rms:.2} dBFS"
    );
    let gate_spec = ParamId::GateThresholdDb.spec();
    let expected_gate = (noise_rms + cal.derived.input_gain_db + 6.0 - 4.0 * cal.derived.denoise_mix)
        .clamp(gate_spec.min, gate_spec.max);
    assert!(
        (cal.derived.gate_threshold_db - expected_gate).abs() <= 0.5,
        "gate {} dB vs expected {expected_gate:.2} dB",
        cal.derived.gate_threshold_db
    );

    // Noisy room (noise > -48 dBFS) selects the aggressive denoiser mix.
    assert_eq!(cal.derived.denoise_mix, 0.85);

    // Offline-verified simulation lands its peaks inside -6..-3 dBFS.
    assert!(
        report.simulated_peak_db >= PEAK_TARGET_LOW_DB && report.simulated_peak_db <= PEAK_TARGET_HIGH_DB,
        "simulated peak {} dB outside target window",
        report.simulated_peak_db
    );
    assert!(report.simulated_rms_db.is_finite());

    // Speech level: input gain puts the voiced RMS at -18 dBFS, compressor
    // threshold 2 dB above the gained voiced RMS (within SPECS clamping).
    assert_eq!(cal.speech_rms_db + cal.derived.input_gain_db, TARGET_SPEECH_RMS_DB);
    assert_eq!(cal.derived.comp_thresh_db, TARGET_SPEECH_RMS_DB + 2.0);

    // Measured fields are populated and sane.
    assert!(cal.noise_peak_db > cal.noise_rms_db);
    assert!(cal.crest_db > 0.0 && cal.crest_db <= 40.0);
    assert!(cal.tilt_db >= -40.0 && cal.tilt_db <= 40.0);
    assert_eq!(
        cal.noise_rms_db, noise_rms,
        "noise measurement must match the reference"
    );

    println!(
        "noisy_quiet: noise_rms={:.2} gate={:.2} (-50 base {DYNAMIC_BASE_GATE_DB}) mix={} gain={} comp_thresh={} makeup={:.2} crest={:.2} tilt={:.2} speech_rms={:.2} sim_peak={:.2} sim_rms={:.2}",
        cal.noise_rms_db,
        cal.derived.gate_threshold_db,
        cal.derived.denoise_mix,
        cal.derived.input_gain_db,
        cal.derived.comp_thresh_db,
        cal.derived.comp_makeup_db,
        cal.crest_db,
        cal.tilt_db,
        cal.speech_rms_db,
        report.simulated_peak_db,
        report.simulated_rms_db
    );

    // The parameter diff covers exactly the five D-021 overrides.
    for (row, &id) in report.changes.iter().zip(CHANGED_PARAMS.iter()) {
        assert_eq!(row.0, id);
        assert_eq!(row.2, report_after(&report, id), "diff must use MacroMapper::apply");
    }
    // `before` diff row = apply(cal = None) with the same macros: the
    // dynamic preset's purity 0.4 moves the -50 dB preset base to -53 dB.
    let (_, gate_before, gate_after) = report.changes[1];
    assert!((gate_before - -53.0).abs() < 0.01, "macro-shifted base gate is -53 dB");
    // Both diff rows run through the same MacroMapper (dynamic purity 0.4),
    // so `after - before` must equal calibration value minus preset base.
    let expected_after = gate_before + (cal.derived.gate_threshold_db - DYNAMIC_BASE_GATE_DB);
    assert!(
        (gate_after - expected_after).abs() < 1e-4,
        "gate after {gate_after} vs expected {expected_after}"
    );
}

/// `MacroMapper::apply` value of a calibrated parameter (post-calibration).
fn report_after(report: &CalibrationReport, id: tine_protocol::params::ParamId) -> f32 {
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    MacroMapper::apply(
        preset,
        Some(&report.calibration),
        preset.macro_defaults,
        PinSet::EMPTY,
        &DspParams::DEFAULT,
    )
    .get(id)
}

#[test]
fn offline_verification_matches_full_pipeline_bit_exactly() {
    let stream = noisy_quiet_stream();
    let report = calibrate(&stream, 480).expect("fixture calibration must succeed");

    // Re-run the exact scenario on the real pipeline: same parameters (the
    // calibration with its verified makeup, applied through the MacroMapper),
    // same audio, same 480-sample framing.
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let params = MacroMapper::apply(
        preset,
        Some(&report.calibration),
        preset.macro_defaults,
        PinSet::EMPTY,
        &DspParams::DEFAULT,
    );
    let mut buf = voice_of(&stream).to_vec();
    let mut pipe = DspPipeline::new(FS as f32, 480);
    pipe.process(&mut buf, &params, &mut NoMeters, &mut NoTap);

    let peak = peak_db(&buf);
    let rms = rms_db(&buf);
    assert_eq!(
        peak, report.simulated_peak_db,
        "offline peak must be bit-identical to the real pipeline"
    );
    assert_eq!(
        rms, report.simulated_rms_db,
        "offline RMS must be bit-identical to the real pipeline"
    );
    assert!(!pipe.is_passthrough_latched());
    assert!(buf.iter().all(|s| s.is_finite()));
}

#[test]
fn makeup_search_converges_within_iteration_budget() {
    // The fixture noise floor is known to require makeup > 0, and the loop
    // must still converge (peak in window) with the spec's 4-iteration cap.
    let stream = noisy_quiet_stream();
    let report = calibrate(&stream, 480).expect("calibration must succeed");
    let (_, makeup_before, makeup_after) = report.changes[4];
    assert_eq!(makeup_before, 0.0, "preset base makeup is 0 dB");
    assert!(
        makeup_after > 0.0,
        "quiet speech needs positive verified makeup (got {makeup_after})"
    );
    assert!(report.simulated_peak_db >= PEAK_TARGET_LOW_DB && report.simulated_peak_db <= PEAK_TARGET_HIGH_DB);
}

// ------------------------------------------------------------- determinism

#[test]
fn report_is_deterministic_and_chunk_agnostic() {
    let stream = noisy_quiet_stream();
    let a = calibrate(&stream, 480).expect("first run");
    // Radical different chunking (and one odd-sized tail) must not move a bit.
    let b = calibrate(&stream, 4_801).expect("second run");
    assert_eq!(a, b, "identical input must produce a bit-identical report");
}

// ---------------------------------------------------------------- failures

#[test]
fn digital_silence_is_too_quiet() {
    let silence = vec![0.0f32; 9 * FS];
    assert_eq!(calibrate(&silence, 480), Err(CalFailure::TooQuiet));
}

#[test]
fn finish_without_capture_is_too_quiet() {
    let mut cal = AutoCalibrator::new(FS as f32);
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let r = cal.finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT);
    assert_eq!(r, Err(CalFailure::TooQuiet));
    // Latched phase: further pushes are inert.
    assert_eq!(cal.push(&[0.5; 480]).phase, CalPhase::Failed);
}

#[test]
fn full_scale_voice_rejects_clipping() {
    // 1 s discard + 3 s moderate room noise + 5 s of full-scale sine:
    // ~16.6% of voice samples sit at/above -0.3 dBFS → far above 0.5%.
    let mut stream = vec![0.0f32; FS];
    stream.extend(pink_noise(3 * FS, 10f32.powf(-40.0 / 20.0), 0x1101));
    let tone = sine(220.0, FS as f32, 5 * FS, 0.99);
    stream.extend(tone);

    // The calibrator counts clips over the *measured* region (noise + voice;
    // the countdown is discarded).
    let expected_pct = {
        let clip = clip_threshold_lin();
        let measured = stream.get(FS..).expect("measured region");
        let hits = measured.iter().filter(|&&x| x.abs() >= clip).count();
        (hits as f64 / (measured.len() as f64)) * 100.0
    };
    assert!(
        expected_pct > 5.0,
        "sine should clip substantially ({expected_pct:.1}%)"
    );

    match calibrate(&stream, 480) {
        Err(CalFailure::Clipping { pct }) => {
            assert!(
                (pct - expected_pct as f32).abs() < 0.5,
                "clip percentage {pct} vs expected {expected_pct:.2}"
            );
        }
        other => panic!("expected Clipping, got {other:?}"),
    }
}

#[test]
fn sparse_speech_rejects_not_enough_speech() {
    // 1 s discard + 3 s quiet room + only 0.8 s of speech in the 5 s voice
    // window: 800 ms voiced < 1500 ms minimum.
    let mut stream = vec![0.0f32; FS];
    stream.extend(white_noise(3 * FS, 10f32.powf(-55.0 / 20.0), 0x1102));
    stream.extend(sine(300.0, FS as f32, FS * 4 / 5, 0.1)); // 0.8 s of speech
    let voiced_len = stream.len() - 4 * FS;
    stream.resize(9 * FS, 0.0);
    assert_eq!(voiced_len, FS * 4 / 5);

    match calibrate(&stream, 480) {
        Err(CalFailure::NotEnoughSpeech { voiced_ms }) => {
            assert!(
                (600..MIN_VOICED_MS).contains(&voiced_ms),
                "voiced {voiced_ms} ms out of expected 600..{MIN_VOICED_MS}"
            );
        }
        other => panic!("expected NotEnoughSpeech, got {other:?}"),
    }
}

#[test]
fn speech_at_noise_floor_rejects_too_noisy() {
    // Room noise at -35 dBFS RMS; voice bursts only +11 dB above the noise
    // (above the +10 dB voiced cut, so speech is detected) at 50% duty:
    // whole-segment RMS lands ~-27 dBFS → SNR ~8 dB < 10 dB.
    let noise_amp = 10f32.powf(-35.0 / 20.0) * 3f32.sqrt();
    let burst_amp = 10f32.powf(-24.0 / 20.0) * 3f32.sqrt();
    let mut stream = vec![0.0f32; FS];
    stream.extend(white_noise(3 * FS, noise_amp, 0x1103));
    for cycle in 0..5 {
        let on = white_noise(FS / 2, burst_amp, 0x1104 + cycle as u64);
        stream.extend_from_slice(&on);
        stream.resize(stream.len() + FS / 2, 0.0);
    }
    assert_eq!(stream.len(), 9 * FS);

    match calibrate(&stream, 480) {
        Err(CalFailure::TooNoisy { snr_db }) => {
            assert!(
                snr_db > 0.0 && snr_db < 10.0,
                "SNR {snr_db:.2} dB out of expected 0..10"
            );
        }
        other => panic!("expected TooNoisy, got {other:?}"),
    }
}

#[test]
fn inaudible_mic_rejects_gain_out_of_range() {
    // Mic far too quiet: -90 dBFS noise floor, -72 dBFS speech → the -18 dBFS
    // target would need ~+54 dB of gain, above the +36 dB cap.
    let noise_amp = 10f32.powf(-90.0 / 20.0) * 3f32.sqrt();
    let speech_amp = 10f32.powf(-72.0 / 20.0) * 3f32.sqrt();
    let mut stream = vec![0.0f32; FS];
    stream.extend(white_noise(3 * FS, noise_amp, 0x1105));
    stream.extend(white_noise(5 * FS, speech_amp, 0x1106));

    let expected_needed = TARGET_SPEECH_RMS_DB - rms_db(stream.get(4 * FS..).expect("voice"));
    match calibrate(&stream, 480) {
        Err(CalFailure::GainOutOfRange { needed_db }) => {
            assert!(
                (needed_db - expected_needed).abs() < 1.0,
                "needed {needed_db:.2} dB vs expected {expected_needed:.2} dB"
            );
            assert!(needed_db > MAX_INPUT_GAIN_DB);
        }
        other => panic!("expected GainOutOfRange, got {other:?}"),
    }
}

// ------------------------------------------------------------------- misc

#[test]
fn capture_is_rate_agnostic_44_1khz() {
    let fs = 44_100.0f32;
    let n = |secs: usize| (secs as f64 * f64::from(fs)) as usize;
    // Quiet room (below -48 dBFS → gentle denoise mix) + comfortable speech.
    let mut stream = vec![0.0f32; n(1)];
    stream.extend(white_noise(n(3), 10f32.powf(-62.0 / 20.0), 0x1107));
    stream.extend(sine(220.0, fs, n(5), 0.1)); // ~-23 dBFS RMS

    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let mut cal = AutoCalibrator::new(fs);
    capture(&mut cal, &stream, 1_000);
    let report = cal
        .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
        .expect("44.1 kHz capture must calibrate");
    let d = &report.calibration.derived;
    assert_eq!(d.denoise_mix, 0.50, "quiet room selects the gentle mix");
    assert_eq!(d.comp_thresh_db, TARGET_SPEECH_RMS_DB + 2.0);
    assert!(
        report.simulated_peak_db >= PEAK_TARGET_LOW_DB && report.simulated_peak_db <= PEAK_TARGET_HIGH_DB,
        "simulated peak {} dB outside target window at 44.1 kHz",
        report.simulated_peak_db
    );
    assert_eq!(cal.push(&[0.0; 480]).phase, CalPhase::Done);
}

#[test]
fn hot_preamp_quiet_room_uses_gentle_denoise_and_gain_shifted_gate() {
    // Hot preamp: raw noise is -46 dBFS (above -48 dBFS), but speech is -14 dBFS
    // (SNR = 32 dB >= 26 dB, gain = -4 dB, post-gain noise = -50 dBFS <= -42 dBFS).
    // Must NOT be misclassified as noisy (D4), and gate must include gain (D3).
    let noise_amp = 10f32.powf(-46.0 / 20.0) * 3f32.sqrt();
    let speech_amp = 10f32.powf(-14.0 / 20.0) * 2f32.sqrt();
    let mut stream = vec![0.0f32; FS];
    stream.extend(white_noise(3 * FS, noise_amp, 0x2201));
    stream.extend(sine(220.0, FS as f32, 5 * FS, speech_amp));

    let report = calibrate(&stream, 480).expect("hot preamp calibration must succeed");
    let cal = &report.calibration;
    assert_eq!(
        cal.derived.denoise_mix, 0.50,
        "hot preamp with high SNR must select gentle denoise_mix"
    );
    assert!(
        cal.derived.input_gain_db < 0.0,
        "hot preamp needs negative gain, got {}",
        cal.derived.input_gain_db
    );
    let gate_spec = ParamId::GateThresholdDb.spec();
    let expected_gate = (cal.noise_rms_db + cal.derived.input_gain_db + 6.0 - 4.0 * cal.derived.denoise_mix)
        .clamp(gate_spec.min, gate_spec.max);
    assert!(
        (cal.derived.gate_threshold_db - expected_gate).abs() <= 0.1,
        "gate {} dB vs expected {expected_gate:.2} dB",
        cal.derived.gate_threshold_db
    );
}

#[test]
fn finish_is_reproducible_on_second_call() {
    let stream = noisy_quiet_stream();
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let mut cal = AutoCalibrator::new(FS as f32);
    capture(&mut cal, &stream, 480);
    let first = cal
        .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
        .expect("first finish");
    let second = cal
        .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
        .expect("second finish");
    assert_eq!(first, second, "re-running finish must be deterministic");
    assert_eq!(CHANGED_PARAM_COUNT, 5);
    assert!(!matches!(
        *CHANGED_PARAMS.last().expect("non-empty"),
        tine_protocol::params::ParamId::Bypass
    ));
}

// -------------------------------------------------------------------- perf

#[test]
#[ignore = "perf budget: run with `cargo test -p tine-dsp --profile perf --test calibrator -- --ignored --nocapture`"]
fn finish_meets_50ms_budget() {
    let stream = noisy_quiet_stream();
    let preset: &PresetDef = builtin(tine_protocol::presets::Archetype::Dynamic);
    let mut cal = AutoCalibrator::new(FS as f32);
    capture(&mut cal, &stream, 480);

    // Warm-up pass (caches, branch predictors, CPU frequency).
    let _ = cal
        .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
        .expect("warm-up finish");

    let t0 = Instant::now();
    let report = cal
        .finish(preset, preset.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT)
        .expect("measured finish");
    let elapsed = t0.elapsed();
    let ms = elapsed.as_secs_f64() * 1e3;
    println!(
        "finish(): {ms:.2} ms for 5 s @ 48 kHz (report peak {:.2} dB)",
        report.simulated_peak_db
    );
    assert!(ms < 50.0, "finish() took {ms:.2} ms (budget 50 ms)");
}
