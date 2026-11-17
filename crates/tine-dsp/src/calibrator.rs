//! 8-second Auto-Calibration state machine (`AutoCalibrator`) — Layer B of
//! `docs/project/04-mic-adaptation.md`.
//!
//! A pure, deterministic state machine that turns 9 seconds of *raw* mic audio
//! (the APO is bypassed during capture) into a [`Calibration`] plus a
//! parameter-level diff ([`CalibrationReport::changes`]) the wizard UI (T20)
//! shows the user. Its loudness targets are *proven*, not guessed: `finish()`
//! simulates the complete `DspPipeline` offline over the captured voice and
//! iterates the compressor makeup until the simulated output peaks land in
//! `-6..-3 dBFS`.
//!
//! # Phases
//!
//! [`CalPhase::Countdown`] 1 s (discarded) → [`CalPhase::Noise`] 3 s →
//! [`CalPhase::Voice`] 5 s → [`CalPhase::Analyzing`]. [`AutoCalibrator::push`]
//! is allocation-free and may be fed blocks of any size; the capture is
//! chunk-agnostic, so identical audio yields a bit-identical report.
//!
//! # Derivation (Layer B rules)
//!
//! * gate threshold = noise RMS + 6 dB;
//! * `denoise_mix` = 0.85 when noise > -48 dBFS else 0.50;
//! * input gain so the integrated *voiced* RMS hits -18 dBFS (failure when
//!   more than +36 dB would be needed);
//! * compressor threshold = voiced RMS after gain + 2 dB;
//! * compressor makeup verified by offline full-pipeline simulation so output
//!   peaks land in `-6..-3 dBFS` (max 4 iterations, best-effort otherwise).
//!
//! # Rejections
//!
//! [`CalFailure`] carries the wizard's retry reasons: clipping, a muted/dead
//! mic ([`CalFailure::TooQuiet`]), not enough detected speech, SNR below 10 dB,
//! and out-of-range gain. The spectral tilt is *reported* only (a Body-macro
//! suggestion for the UI, never auto-applied).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use tine_protocol::params::{DspParams, FilterType, ParamId};
use tine_protocol::presets::{Calibration, CalibrationOverrides, MacroPositions, PinSet};

use crate::macros::MacroMapper;
use crate::meter::StageMeters;
use crate::presets::PresetDef;
use crate::primitives::biquad::{Biquad, Coeffs};
use crate::primitives::db::{DB_FLOOR, db_to_lin, lin_to_db};
use crate::primitives::math::log10_f64;
use crate::rt::DenormalGuard;
use crate::stages::{CleanupStage, CompressorStage, DeesserStage, EqStage, InputStage, LimiterStage, Stage};

/// Countdown phase: seconds of audio discarded before measurement starts.
pub const COUNTDOWN_S: f32 = 1.0;
/// Room-noise measurement phase duration in seconds.
pub const NOISE_S: f32 = 3.0;
/// Voice measurement phase duration in seconds.
pub const VOICE_S: f32 = 5.0;
/// RMS window for the noise measurement (median of windows, click-robust).
pub const NOISE_WINDOW_MS: f32 = 100.0;
/// Voiced-detection window for the voice measurement.
pub const VOICE_WINDOW_MS: f32 = 20.0;
/// A voice window counts as voiced when its RMS exceeds noise RMS by this much.
pub const VOICED_ABOVE_NOISE_DB: f32 = 10.0;
/// Gate threshold offset above the measured noise RMS (dB).
pub const GATE_OFFSET_DB: f32 = 6.0;
/// Noise RMS above which the aggressive `RNNoise` mix is selected (dBFS).
pub const NOISY_RMS_DB: f32 = -48.0;
/// Speech-to-noise ratio below which the aggressive `RNNoise` mix is selected (dB).
pub const NOISY_SNR_DB: f32 = 26.0;
/// Post-gain noise RMS above which the aggressive `RNNoise` mix is selected (dBFS).
pub const NOISY_POST_GAIN_RMS_DB: f32 = -42.0;
/// `denoise_mix` selected for noisy rooms.
pub const NOISY_DENOISE_MIX: f32 = 0.85;
/// `denoise_mix` selected for quiet rooms.
pub const QUIET_DENOISE_MIX: f32 = 0.50;
/// Target integrated voiced-speech RMS after calibration (dBFS).
pub const TARGET_SPEECH_RMS_DB: f32 = -18.0;
/// Maximum input gain the calibrator may ask for (dB); above this it fails.
pub const MAX_INPUT_GAIN_DB: f32 = 36.0;
/// Compressor threshold offset above the gained voiced RMS (dB).
pub const COMP_THRESH_OFFSET_DB: f32 = 2.0;
/// Simulated output peaks must land at or above this (dBFS).
pub const PEAK_TARGET_LOW_DB: f32 = -6.0;
/// Simulated output peaks must land at or below this (dBFS).
pub const PEAK_TARGET_HIGH_DB: f32 = -3.0;
/// Maximum makeup-verification simulation passes in `finish()`.
pub const MAX_VERIFY_ITERATIONS: usize = 4;
/// Sample is "clipped" at/above this level (dBFS).
pub const CLIP_THRESHOLD_DB: f32 = -0.3;
/// Capture rejects when more than this percentage of samples is clipped.
pub const CLIP_MAX_PCT: f32 = 0.5;
/// Capture rejects with less than this much voiced audio (ms).
pub const MIN_VOICED_MS: u32 = 1_500;
/// Capture rejects below this speech-to-noise ratio over the voice segment (dB).
pub const MIN_SNR_DB: f32 = 10.0;
/// Capture peak below this (dBFS) means the mic is muted/dead → `TooQuiet`.
pub const QUIET_PEAK_DB: f32 = -80.0;
/// Spectral tilt: low-band edge of the tilt measurement (Hz).
pub const TILT_LOW_EDGE_HZ: f32 = 350.0;
/// Spectral tilt: mid-band lower edge the low band is compared against (Hz).
pub const TILT_MID_LOW_HZ: f32 = 1_000.0;
/// Spectral tilt: mid-band upper edge (Hz).
pub const TILT_MID_HIGH_HZ: f32 = 6_000.0;
/// Number of parameters the calibration layer replaces (D-021).
pub const CHANGED_PARAM_COUNT: usize = 5;

/// The five `ParamId`s the calibration layer replaces (D-021), in wizard order.
pub const CHANGED_PARAMS: [ParamId; CHANGED_PARAM_COUNT] = [
    ParamId::InputGainDb,
    ParamId::GateThresholdDb,
    ParamId::DenoiseMix,
    ParamId::CompThreshDb,
    ParamId::CompMakeupDb,
];

/// Cap for the noise-window median accumulator: 3 s / 100 ms = 30 windows at
/// any sample rate (window and region scale with fs), 64 covers rounding.
const MAX_NOISE_WINDOWS: usize = 64;

/// Lowest supported sample rate (buffer sizing guard).
const FS_MIN_HZ: f32 = 8_000.0;
/// Highest supported sample rate (buffer sizing guard, 192 kHz per standards).
const FS_MAX_HZ: f32 = 192_000.0;

/// Offline simulation block size: matches the pipeline's native frame (480
/// samples = 10 ms at 48 kHz), so per-iteration simulations are bit-comparable
/// to `DspPipeline::new(fs, 480)`.
const SIM_BLOCK: usize = 480;

/// Calibration state-machine phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalPhase {
    /// First second: audio is discarded (get ready / settle).
    Countdown,
    /// Room-noise measurement (3 s of "silence").
    Noise,
    /// Voice measurement (5 s of normal speech).
    Voice,
    /// 9 s captured; measurements + offline verification run in `finish()`.
    Analyzing,
    /// `finish()` produced a [`CalibrationReport`].
    Done,
    /// `finish()` returned a [`CalFailure`].
    Failed,
}

impl CalPhase {
    /// Returns `true` in the terminal phases ([`CalPhase::Done`]/[`CalPhase::Failed`]).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, CalPhase::Done | CalPhase::Failed)
    }
}

/// Live wizard progress reported by every [`AutoCalibrator::push`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalProgress {
    /// Current capture phase.
    pub phase: CalPhase,
    /// Fraction (`0.0..=1.0`) of the current phase already captured;
    /// `0.0` while [`CalPhase::Analyzing`] and `1.0` in terminal phases.
    pub phase_progress: f32,
    /// RMS level (dBFS) of the audio consumed by this `push` call
    /// (live level meter; sticky once the capture ends).
    pub live_level_db: f32,
    /// Total voiced audio detected so far in the voice phase (ms).
    pub voiced_ms: u32,
}

/// Why a calibration capture was rejected (shown in the wizard, offer retry).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CalFailure {
    /// The capture is (near-)digital silence: the mic is muted or dead.
    TooQuiet,
    /// Speech is drowned by noise over the whole voice segment.
    TooNoisy {
        /// Measured signal-to-noise ratio (voice-segment RMS − noise RMS, dB).
        snr_db: f32,
    },
    /// More than [`CLIP_MAX_PCT`] % of samples at/above [`CLIP_THRESHOLD_DB`]
    /// (tell the user to lower hardware gain).
    Clipping {
        /// Percentage of measured samples at/above the clip threshold.
        pct: f32,
    },
    /// Less than [`MIN_VOICED_MS`] of voiced audio in the voice phase.
    NotEnoughSpeech {
        /// Total voiced audio detected (ms).
        voiced_ms: u32,
    },
    /// Reaching the -18 dBFS speech target needs more than +36 dB of gain.
    GainOutOfRange {
        /// Uncapped gain that would be needed (dB).
        needed_db: f32,
    },
}

/// Final auto-calibration output: the calibration itself and its proven effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalibrationReport {
    /// The measured + derived calibration stored per endpoint.
    pub calibration: Calibration,
    /// Before/after per calibrated parameter (`(id, before, after)`), derived
    /// by running [`MacroMapper::apply`] with and without the calibration.
    /// Pinned parameters (D-007) show `before == after` (they stay un-driven).
    pub changes: [(ParamId, f32, f32); CHANGED_PARAM_COUNT],
    /// Peak level of the offline-verified pipeline simulation (dBFS).
    pub simulated_peak_db: f32,
    /// RMS level of the offline-verified pipeline simulation (dBFS).
    pub simulated_rms_db: f32,
}

/// Measures of one analysis pass over the captured voice.
struct Measurements {
    /// Total voiced audio (ms).
    voiced_ms: u32,
    /// Integrated voiced-speech RMS (dBFS).
    speech_rms_db: f32,
    /// RMS of the *whole* voice segment, gaps included (dBFS) — SNR basis.
    segment_rms_db: f32,
    /// Crest factor over the voiced windows (dB).
    crest_db: f32,
    /// Spectral tilt: `10·log10(E(<350 Hz) / E(1–6 kHz))` over voiced windows (dB).
    tilt_db: f32,
}

/// Pure, deterministic 9-second Auto-Calibration state machine.
///
/// All capture memory is allocated once in [`AutoCalibrator::new`];
/// [`AutoCalibrator::push`] is allocation-free (a non-RT capture thread may
/// feed it host blocks of any size). [`AutoCalibrator::finish`] is a one-shot
/// offline computation and may allocate.
pub struct AutoCalibrator {
    fs: f32,
    /// 9 s mono capture: `[0..countdown_len)` discarded,
    /// then the noise region, then the voice region.
    buf: Box<[f32]>,
    countdown_len: usize,
    noise_len: usize,
    voice_len: usize,
    noise_win_samples: usize,
    voice_win_samples: usize,
    voice_win_ms: u32,
    countdown_filled: usize,
    noise_filled: usize,
    voice_filled: usize,
    phase: CalPhase,
    clip_count: u64,
    measured_samples: u64,
    peak_lin: f32,
    noise_finalized: bool,
    noise_rms_db: f32,
    noise_peak_db: f32,
    win_sum_sq: f64,
    win_filled: usize,
    voiced_ms: u32,
    live_level_db: f32,
}

impl AutoCalibrator {
    /// Creates a calibrator for `fs` Hz, preallocating the 9 s mono capture
    /// buffer. Non-finite or out-of-range rates fall back to 48 kHz; the rate
    /// is clamped to `8 kHz..=192 kHz` so the buffer stays bounded.
    #[must_use]
    pub fn new(fs: f32) -> Self {
        let fs = if (FS_MIN_HZ..=FS_MAX_HZ).contains(&fs) {
            fs
        } else {
            48_000.0
        };
        let countdown_len = (f64::from(COUNTDOWN_S) * f64::from(fs)).round() as usize;
        let noise_len = (f64::from(NOISE_S) * f64::from(fs)).round() as usize;
        let voice_len = (f64::from(VOICE_S) * f64::from(fs)).round() as usize;
        let noise_win_samples = (f64::from(NOISE_WINDOW_MS) * 1e-3 * f64::from(fs)).round() as usize;
        let voice_win_samples = (f64::from(VOICE_WINDOW_MS) * 1e-3 * f64::from(fs)).round() as usize;
        let voice_win_ms = VOICE_WINDOW_MS.round() as u32;
        Self {
            fs,
            buf: vec![0.0; countdown_len + noise_len + voice_len].into_boxed_slice(),
            countdown_len,
            noise_len,
            voice_len,
            noise_win_samples: noise_win_samples.max(1),
            voice_win_samples: voice_win_samples.max(1),
            voice_win_ms: voice_win_ms.max(1),
            countdown_filled: 0,
            noise_filled: 0,
            voice_filled: 0,
            phase: CalPhase::Countdown,
            clip_count: 0,
            measured_samples: 0,
            peak_lin: 0.0,
            noise_finalized: false,
            noise_rms_db: DB_FLOOR,
            noise_peak_db: DB_FLOOR,
            win_sum_sq: 0.0,
            win_filled: 0,
            voiced_ms: 0,
            live_level_db: DB_FLOOR,
        }
    }

    /// Feeds one block of raw mic audio into the state machine.
    ///
    /// Allocation-free and chunk-agnostic; audio past the 9 s capture is
    /// ignored (the phase stays [`CalPhase::Analyzing`]). Returns the live
    /// [`CalProgress`] after every call.
    pub fn push(&mut self, samples: &[f32]) -> CalProgress {
        let mut consumed = 0usize;
        if self.capture_open() {
            let mut rest = samples;
            while !rest.is_empty() {
                let take = match self.phase {
                    CalPhase::Countdown => (self.countdown_len - self.countdown_filled).min(rest.len()),
                    CalPhase::Noise => (self.noise_len - self.noise_filled).min(rest.len()),
                    CalPhase::Voice => (self.voice_len - self.voice_filled).min(rest.len()),
                    _ => 0,
                };
                if take == 0 {
                    break;
                }
                let Some(chunk) = rest.get(..take) else { break };
                match self.phase {
                    CalPhase::Countdown => self.countdown_filled += take,
                    CalPhase::Noise => self.absorb_noise(chunk),
                    CalPhase::Voice => self.absorb_voice(chunk),
                    _ => {}
                }
                consumed += take;
                let Some(tail) = rest.get(take..) else { break };
                rest = tail;

                self.advance_phase();
                if self.phase == CalPhase::Analyzing {
                    break;
                }
            }
        }
        if consumed > 0
            && let Some(consumed_slice) = samples.get(..consumed)
        {
            self.live_level_db = rms_db(consumed_slice);
        }
        self.progress()
    }

    /// Finishes the calibration: analyzes the capture, derives the five
    /// calibrated parameters (D-021) and *proves* the result by simulating the
    /// complete pipeline offline over the captured voice, iterating the
    /// compressor makeup until simulated peaks land in `-6..-3 dBFS`.
    ///
    /// May be called at any time (an early call analyses whatever was
    /// captured). Deterministic: identical captured audio yields a
    /// bit-identical report. On success the phase latches to
    /// [`CalPhase::Done`], on rejection to [`CalPhase::Failed`]; re-running
    /// recomputes the same answer.
    pub fn finish(
        &mut self,
        preset: &PresetDef,
        macros: MacroPositions,
        pins: PinSet,
        current: &DspParams,
    ) -> Result<CalibrationReport, CalFailure> {
        // Finalize any partially captured noise segment (early-finish support).
        if !self.noise_finalized {
            let start = self.countdown_len;
            let end = start + self.noise_filled;
            if let Some(region) = self.buf.get(start..end) {
                let (rms_db, peak_db) = noise_stats(region, self.noise_win_samples);
                self.noise_rms_db = rms_db;
                self.noise_peak_db = peak_db;
            }
            self.noise_finalized = true;
        }

        let m = self.measure_voice();

        if let Some(failure) = self.check_failures(&m) {
            self.phase = CalPhase::Failed;
            return Err(failure);
        }

        let report = self.derive_and_verify(&m, preset, macros, pins, current);
        self.phase = CalPhase::Done;
        Ok(report)
    }

    /// Current capture phase still accepts audio (`Analyzing`/terminal do not).
    fn capture_open(&self) -> bool {
        !self.phase.is_terminal() && self.phase != CalPhase::Analyzing
    }

    /// Copies a noise-phase chunk into the capture buffer and folds counters.
    fn absorb_noise(&mut self, chunk: &[f32]) {
        let start = self.countdown_len + self.noise_filled;
        if let Some(dst) = self.buf.get_mut(start..start + chunk.len()) {
            for (d, &s) in dst.iter_mut().zip(chunk.iter()) {
                *d = s;
            }
        }
        self.fold_measured(chunk);
        self.noise_filled += chunk.len();
    }

    /// Copies a voice-phase chunk into the capture buffer, folds counters, and
    /// classifies completed 20 ms windows as voiced/not-voiced (live UI count).
    fn absorb_voice(&mut self, chunk: &[f32]) {
        let start = self.countdown_len + self.noise_len + self.voice_filled;
        if let Some(dst) = self.buf.get_mut(start..start + chunk.len()) {
            for (d, &s) in dst.iter_mut().zip(chunk.iter()) {
                *d = s;
            }
        }
        self.fold_measured(chunk);
        self.voice_filled += chunk.len();

        // Live voiced detection: same window math as the analysis pass.
        let thr_db = self.noise_rms_db + VOICED_ABOVE_NOISE_DB;
        for &s in chunk {
            self.win_sum_sq += f64::from(s) * f64::from(s);
            self.win_filled += 1;
            if self.win_filled >= self.voice_win_samples {
                let rms = ((self.win_sum_sq / (self.win_filled as f64)).sqrt()) as f32;
                if lin_to_db(rms) > thr_db {
                    self.voiced_ms += self.voice_win_ms;
                }
                self.win_sum_sq = 0.0;
                self.win_filled = 0;
            }
        }
    }

    /// Updates clipping and peak counters over the measured region
    /// (noise + voice; the countdown is discarded).
    fn fold_measured(&mut self, chunk: &[f32]) {
        let clip_lin = db_to_lin(CLIP_THRESHOLD_DB);
        for &s in chunk {
            let a = s.abs();
            if a >= clip_lin {
                self.clip_count += 1;
            }
            if a > self.peak_lin {
                self.peak_lin = a;
            }
            self.measured_samples += 1;
        }
    }

    /// Advances the phase once the current one is completely captured.
    fn advance_phase(&mut self) {
        match self.phase {
            CalPhase::Countdown if self.countdown_filled >= self.countdown_len => {
                self.phase = CalPhase::Noise;
            }
            CalPhase::Noise if self.noise_filled >= self.noise_len => {
                let start = self.countdown_len;
                let end = start + self.noise_len;
                if let Some(region) = self.buf.get(start..end) {
                    let (rms_db, peak_db) = noise_stats(region, self.noise_win_samples);
                    self.noise_rms_db = rms_db;
                    self.noise_peak_db = peak_db;
                }
                self.noise_finalized = true;
                self.phase = CalPhase::Voice;
            }
            CalPhase::Voice if self.voice_filled >= self.voice_len => {
                self.phase = CalPhase::Analyzing;
            }
            _ => {}
        }
    }

    /// Builds the current [`CalProgress`] snapshot.
    fn progress(&self) -> CalProgress {
        let (phase_progress, voiced) = match self.phase {
            CalPhase::Countdown => (frac(self.countdown_filled, self.countdown_len), 0),
            CalPhase::Noise => (frac(self.noise_filled, self.noise_len), 0),
            CalPhase::Voice => (frac(self.voice_filled, self.voice_len), self.voiced_ms),
            CalPhase::Analyzing => (0.0, self.voiced_ms),
            CalPhase::Done | CalPhase::Failed => (1.0, self.voiced_ms),
        };
        CalProgress {
            phase: self.phase,
            phase_progress,
            live_level_db: self.live_level_db,
            voiced_ms: voiced,
        }
    }

    // --------------------------------------------------------------- analyze

    /// Runs the failure checks in order and returns the first rejection.
    fn check_failures(&self, m: &Measurements) -> Option<CalFailure> {
        // 1. Clipping dominates: the user must lower hardware gain first.
        let pct = pct(self.clip_count, self.measured_samples);
        if pct > CLIP_MAX_PCT {
            return Some(CalFailure::Clipping { pct });
        }
        // 2. Muted / dead mic: the whole capture is (near-)digital silence.
        if lin_to_db(self.peak_lin) < QUIET_PEAK_DB {
            return Some(CalFailure::TooQuiet);
        }
        // 3. Not enough detected speech.
        if m.voiced_ms < MIN_VOICED_MS {
            return Some(CalFailure::NotEnoughSpeech { voiced_ms: m.voiced_ms });
        }
        // 4. Speech drowned in noise (whole voice segment vs. noise floor).
        let snr_db = m.segment_rms_db - self.noise_rms_db;
        if snr_db < MIN_SNR_DB {
            return Some(CalFailure::TooNoisy { snr_db });
        }
        // 5. Target unreachable within the +36 dB gain limit.
        let needed_db = TARGET_SPEECH_RMS_DB - m.speech_rms_db;
        if needed_db > MAX_INPUT_GAIN_DB {
            return Some(CalFailure::GainOutOfRange { needed_db });
        }
        None
    }

    /// Single pass over the captured voice: voiced detection, integrated
    /// voiced RMS, crest factor, and spectral tilt via two `primitives`
    /// biquads (low-pass < 350 Hz; 1–6 kHz band-pass, Q = center/bandwidth).
    fn measure_voice(&self) -> Measurements {
        let start = self.countdown_len + self.noise_len;
        let end = start + self.voice_filled;
        let Some(region) = self.buf.get(start..end) else {
            return Measurements {
                voiced_ms: 0,
                speech_rms_db: DB_FLOOR,
                segment_rms_db: DB_FLOOR,
                crest_db: 0.0,
                tilt_db: 0.0,
            };
        };

        let fs = f64::from(self.fs);
        let mut lp = Biquad::new(Coeffs::design(
            FilterType::LowPass,
            fs,
            f64::from(TILT_LOW_EDGE_HZ),
            core::f64::consts::FRAC_1_SQRT_2,
            0.0,
        ));
        let mid_center = f64::midpoint(f64::from(TILT_MID_LOW_HZ), f64::from(TILT_MID_HIGH_HZ));
        let mid_bw = f64::from(TILT_MID_HIGH_HZ) - f64::from(TILT_MID_LOW_HZ);
        let mut bp = Biquad::new(Coeffs::design(
            FilterType::BandPass,
            fs,
            mid_center,
            (mid_center / mid_bw).clamp(0.05, 20.0),
            0.0,
        ));

        let thr_db = self.noise_rms_db + VOICED_ABOVE_NOISE_DB;
        let win_samples = self.voice_win_samples;
        let mut voiced_ms = 0u32;
        let mut voiced_samples = 0u64;
        let mut voiced_sum_sq = 0.0f64;
        let mut voiced_peak = 0.0f32;
        let mut e_low = 0.0f64;
        let mut e_mid = 0.0f64;
        let mut segment_sum_sq = 0.0f64;

        for win in region.chunks_exact(win_samples) {
            let mut raw = 0.0f64;
            let mut low = 0.0f64;
            let mut mid = 0.0f64;
            let mut peak = 0.0f32;
            for &x in win {
                let xd = f64::from(x);
                raw += xd * xd;
                let a = x.abs();
                if a > peak {
                    peak = a;
                }
                let yl = f64::from(lp.process_sample(x));
                low += yl * yl;
                let ym = f64::from(bp.process_sample(x));
                mid += ym * ym;
            }
            segment_sum_sq += raw;
            let win_rms_db = lin_to_db(((raw / (win.len() as f64)).sqrt()) as f32);
            if win_rms_db > thr_db {
                voiced_ms += self.voice_win_ms;
                voiced_samples += win.len() as u64;
                voiced_sum_sq += raw;
                if peak > voiced_peak {
                    voiced_peak = peak;
                }
                e_low += low;
                e_mid += mid;
            }
        }

        let speech_rms_db = if voiced_samples > 0 {
            lin_to_db(((voiced_sum_sq / (voiced_samples as f64)).sqrt()) as f32)
        } else {
            DB_FLOOR
        };
        let crest_db = (lin_to_db(voiced_peak) - speech_rms_db).clamp(0.0, 40.0);
        let segment_rms_db = if region.is_empty() {
            DB_FLOOR
        } else {
            lin_to_db(((segment_sum_sq / (region.len() as f64)).sqrt()) as f32)
        };
        let tilt_db = if e_low > 0.0 && e_mid > 0.0 {
            ((10.0 * log10_f64(e_low / e_mid)) as f32).clamp(-40.0, 40.0)
        } else if e_mid > 0.0 {
            -40.0
        } else if e_low > 0.0 {
            40.0
        } else {
            0.0
        };

        Measurements {
            voiced_ms,
            speech_rms_db,
            segment_rms_db,
            crest_db,
            tilt_db,
        }
    }

    // ---------------------------------------------------------------- derive

    /// Derives the calibrated parameters and verifies them against the real
    /// pipeline simulated offline in 480-sample blocks.
    fn derive_and_verify(
        &self,
        m: &Measurements,
        preset: &PresetDef,
        macros: MacroPositions,
        pins: PinSet,
        current: &DspParams,
    ) -> CalibrationReport {
        let gain = clamp_param(ParamId::InputGainDb, TARGET_SPEECH_RMS_DB - m.speech_rms_db);
        let snr_db = m.speech_rms_db - self.noise_rms_db;
        let denoise = if snr_db < NOISY_SNR_DB || (self.noise_rms_db + gain) > NOISY_POST_GAIN_RMS_DB {
            NOISY_DENOISE_MIX
        } else {
            QUIET_DENOISE_MIX
        };
        // Stage 1 input gain runs before Stage 2 expander, so the expander sees
        // `noise_rms_db + gain`. Also account for RNNoise attenuation before the
        // expander so quiet speech syllables aren't chopped when AI denoising is active.
        let gate = clamp_param(
            ParamId::GateThresholdDb,
            self.noise_rms_db + gain + GATE_OFFSET_DB - 4.0 * denoise,
        );
        let thresh = clamp_param(ParamId::CompThreshDb, m.speech_rms_db + gain + COMP_THRESH_OFFSET_DB);

        let mut cal = Calibration {
            noise_rms_db: self.noise_rms_db,
            noise_peak_db: self.noise_peak_db,
            speech_rms_db: m.speech_rms_db,
            crest_db: m.crest_db,
            tilt_db: m.tilt_db,
            derived: CalibrationOverrides {
                input_gain_db: gain,
                gate_threshold_db: gate,
                denoise_mix: denoise,
                comp_thresh_db: thresh,
                comp_makeup_db: 0.0,
            },
        };

        // Offline verification over the captured voice. Stages 1-4 do not
        // depend on the compressor makeup, so they run once; only the
        // compressor + limiter re-run per makeup candidate — exactly the same
        // arithmetic a fresh `DspPipeline` would perform per candidate, at a
        // fraction of the cost (keeps `finish()` inside its 50 ms budget).
        let start = self.countdown_len + self.noise_len;
        let end = start + self.voice_filled;
        let mut pre = Vec::with_capacity(self.voice_filled);
        if let Some(voice) = self.buf.get(start..end) {
            pre.extend_from_slice(voice);
        }

        {
            let _guard = DenormalGuard::new();
            let params = MacroMapper::apply(preset, Some(&cal), macros, pins, current);
            let mut meters = StageMeters::default();
            let mut input = InputStage::new();
            let mut cleanup = CleanupStage::new();
            let mut eq = EqStage::new();
            let mut deesser = DeesserStage::new();
            input.prepare(self.fs, SIM_BLOCK);
            cleanup.prepare(self.fs, SIM_BLOCK);
            eq.prepare(self.fs, SIM_BLOCK);
            deesser.prepare(self.fs, SIM_BLOCK);
            for chunk in pre.chunks_mut(SIM_BLOCK) {
                input.process(chunk, &params, &mut meters);
                cleanup.process(chunk, &params, &mut meters);
                eq.process(chunk, &params, &mut meters);
                deesser.process(chunk, &params, &mut meters);
            }
        }

        // Makeup search: the compressor's gain reduction depends only on its
        // (unchanged) input level, so peak ≈ pre-makeup peak + makeup; one
        // correction step converges and the loop is the spec's safety net.
        // Each candidate runs on a fresh copy of the stages-1-4 output (the
        // tail stages process audio in place).
        let mut best_makeup = 0.0f32;
        let mut best_peak = DB_FLOOR;
        let mut best_rms = DB_FLOOR;
        let mut best_dist = f32::INFINITY;
        let mut makeup = 0.0f32;
        let mut work = Vec::with_capacity(pre.len());
        for _ in 0..MAX_VERIFY_ITERATIONS {
            cal.derived.comp_makeup_db = makeup;
            let params = MacroMapper::apply(preset, Some(&cal), macros, pins, current);
            work.clear();
            work.extend_from_slice(&pre);
            let (peak, rms) = sim_tail(&mut work, &params, self.fs);

            let dist = peak_target_distance(peak);
            if dist < best_dist {
                best_dist = dist;
                best_makeup = makeup;
                best_peak = peak;
                best_rms = rms;
            }
            if dist <= 0.0 {
                break;
            }
            // Makeup >= 0 can only raise the output peak, so an overshoot can
            // never be repaired: makeup 0 is the closest reachable value.
            if peak > PEAK_TARGET_HIGH_DB {
                break;
            }
            let next = clamp_param(
                ParamId::CompMakeupDb,
                makeup + f32::midpoint(PEAK_TARGET_LOW_DB, PEAK_TARGET_HIGH_DB) - peak,
            );
            if (next - makeup).abs() < 1e-3 {
                break;
            }
            makeup = next;
        }

        cal.derived.comp_makeup_db = best_makeup;

        // Parameter-level diff: apply without vs. with the calibration.
        let before = MacroMapper::apply(preset, None, macros, pins, current);
        let after = MacroMapper::apply(preset, Some(&cal), macros, pins, current);
        let mut changes = [(ParamId::Bypass, 0.0, 0.0); CHANGED_PARAM_COUNT];
        for (slot, &id) in changes.iter_mut().zip(CHANGED_PARAMS.iter()) {
            *slot = (id, before.get(id), after.get(id));
        }

        CalibrationReport {
            calibration: cal,
            changes,
            simulated_peak_db: best_peak,
            simulated_rms_db: best_rms,
        }
    }
}

/// Runs the makeup-dependent tail (compressor + limiter) of the offline
/// simulation over the pre-processed voice and returns its `(peak, rms)` in
/// dBFS. Stage state is fresh per call, so the result is bit-identical to a
/// fresh `DspPipeline` running the same parameters over the same audio.
fn sim_tail(pre: &mut [f32], params: &DspParams, fs: f32) -> (f32, f32) {
    let _guard = DenormalGuard::new();
    let mut meters = StageMeters::default();
    let mut comp = CompressorStage::new();
    let mut limiter = LimiterStage::new();
    comp.prepare(fs, SIM_BLOCK);
    limiter.prepare(fs, SIM_BLOCK);
    for chunk in pre.chunks_mut(SIM_BLOCK) {
        comp.process(chunk, params, &mut meters);
        limiter.process(chunk, params, &mut meters);
    }
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f32;
    for &s in pre.iter() {
        sum_sq += f64::from(s) * f64::from(s);
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    let rms = if pre.is_empty() {
        DB_FLOOR
    } else {
        lin_to_db(((sum_sq / (pre.len() as f64)).sqrt()) as f32)
    };
    (lin_to_db(peak), rms)
}

/// Distance (dB) of a simulated peak from the target window `-6..-3 dBFS`.
fn peak_target_distance(peak_db: f32) -> f32 {
    if peak_db < PEAK_TARGET_LOW_DB {
        PEAK_TARGET_LOW_DB - peak_db
    } else if peak_db > PEAK_TARGET_HIGH_DB {
        peak_db - PEAK_TARGET_HIGH_DB
    } else {
        0.0
    }
}

/// Noise statistics: median of 100 ms window RMS values (linear-power median,
/// click-robust) in dB plus the region peak in dB. Allocation-free: the window
/// accumulator is a fixed-size stack array kept sorted by insertion.
fn noise_stats(region: &[f32], win_samples: usize) -> (f32, f32) {
    let mut windows = [0.0f64; MAX_NOISE_WINDOWS];
    let mut count = 0usize;
    for win in region.chunks_exact(win_samples.max(1)) {
        if count >= MAX_NOISE_WINDOWS {
            break;
        }
        let mut sum_sq = 0.0f64;
        for &x in win {
            sum_sq += f64::from(x) * f64::from(x);
        }
        sorted_insert(&mut windows, count, sum_sq / (win.len() as f64));
        count += 1;
    }
    let median = if count == 0 {
        0.0
    } else if count % 2 == 1 {
        windows.get(count / 2).copied().unwrap_or(0.0)
    } else {
        let a = windows.get(count / 2 - 1).copied().unwrap_or(0.0);
        let b = windows.get(count / 2).copied().unwrap_or(0.0);
        f64::midpoint(a, b)
    };
    let mut peak = 0.0f32;
    for &x in region {
        let a = x.abs();
        if a > peak {
            peak = a;
        }
    }
    (lin_to_db((median.sqrt()) as f32), lin_to_db(peak))
}

/// Inserts `v` into the sorted prefix `0..count` of `windows`, shifting the
/// tail right (fixed capacity, allocation-free, no panics).
fn sorted_insert(windows: &mut [f64; MAX_NOISE_WINDOWS], count: usize, v: f64) {
    let mut pos = count;
    for i in 0..count {
        if let Some(&w) = windows.get(i)
            && w > v
        {
            pos = i;
            break;
        }
    }
    let mut i = count;
    while i > pos {
        let src = windows.get(i - 1).copied().unwrap_or(0.0);
        if let Some(dst) = windows.get_mut(i) {
            *dst = src;
        }
        i -= 1;
    }
    if let Some(slot) = windows.get_mut(pos) {
        *slot = v;
    }
}

/// Clamps `v` into the `tine-protocol` SPECS range of `id` (non-finite → default).
fn clamp_param(id: ParamId, v: f32) -> f32 {
    let spec = id.spec();
    if v.is_finite() {
        v.clamp(spec.min, spec.max)
    } else {
        spec.default
    }
}

/// Fraction `filled / total` guarded against division by zero.
fn frac(filled: usize, total: usize) -> f32 {
    if total == 0 {
        1.0
    } else {
        (filled as f64 / total as f64) as f32
    }
}

/// Percentage of `count` in `total` (`0.0` when `total == 0`).
fn pct(count: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (count as f64 / total as f64 * 100.0) as f32
    }
}

/// RMS in dBFS of a slice (f64 accumulation).
fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return DB_FLOOR;
    }
    let sum_sq: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    lin_to_db(((sum_sq / (samples.len() as f64)).sqrt()) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_window_rms_is_robust_to_a_single_click() {
        // 3 s of low-level noise with one window 40 dB louder: the median of
        // 30 windows must not move, but the region peak catches the click.
        let fs = 48_000.0f32;
        let n = (NOISE_S * fs) as usize;
        let win = (NOISE_WINDOW_MS * 1e-3 * fs) as usize;
        let mut region = vec![0.0f32; n];
        let level = db_to_lin(-50.0);
        for (i, s) in region.iter_mut().enumerate() {
            *s = level * ((i % 97) as f32 - 48.0) * 0.01;
        }
        let (rms_clean, _) = noise_stats(&region, win);
        if let Some(w) = region.get_mut(win * 5..win * 6) {
            for s in w.iter_mut() {
                *s *= db_to_lin(40.0);
            }
        }
        let (rms_click, peak) = noise_stats(&region, win);
        assert!(
            (rms_clean - rms_click).abs() < 0.05,
            "median moved: {rms_clean} -> {rms_click}"
        );
        assert!(peak > -20.0, "click peak not observed: {peak}");
    }

    #[test]
    fn peak_target_distance_bands() {
        assert_eq!(peak_target_distance(-6.0), 0.0);
        assert_eq!(peak_target_distance(-4.5), 0.0);
        assert_eq!(peak_target_distance(-3.0), 0.0);
        assert_eq!(peak_target_distance(-9.0), 3.0);
        assert_eq!(peak_target_distance(0.0), 3.0);
    }

    #[test]
    fn helpers_guard_division_by_zero() {
        assert_eq!(frac(5, 0), 1.0);
        assert_eq!(frac(0, 4), 0.0);
        assert_eq!(pct(10, 100), 10.0);
        assert_eq!(pct(10, 0), 0.0);
        assert_eq!(rms_db(&[]), DB_FLOOR);
    }
}
