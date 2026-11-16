//! Named headless verification scenes (pure functions building [`AppState`]).

use tine_protocol::params::ParamId;
use tine_protocol::presets::{Archetype, MacroPositions};

use crate::fake::scene_live_frame;
use crate::model::{
    ApoBadge, AutoTuneStep, AutoTuneUi, CalFailureUi, CalibrationParamDiff, CalibrationUi, EndpointRow, Macro,
    MonitorUi, UiModel,
};
use crate::state::AppState;

fn base_model() -> UiModel {
    UiModel {
        endpoints: vec![
            EndpointRow {
                id: String::from("{0.0.1.00000000}.{shure-sm7b}"),
                name: String::from("Shure SM7B (USB Audio)"),
            },
            EndpointRow {
                id: String::from("{0.0.1.00000000}.{headset-boom}"),
                name: String::from("HyperX Cloud Mic"),
            },
        ],
        selected: Some(0),
        apo: ApoBadge::Active,
        archetype: Archetype::Dynamic,
        macros: MacroPositions {
            purity: 0.55,
            body: 0.62,
            presence: 0.58,
            leveler: 0.50,
        },
        monitor: MonitorUi {
            enabled: false,
            device_name: Some(String::from("Headphones (Realtek Audio)")),
            available_devices: vec![
                String::from("Headphones (Realtek Audio)"),
                String::from("Speakers (USB DAC)"),
            ],
            estimated_latency_ms: 14.2,
        },
        ..UiModel::default()
    }
}

/// Default Golden Line view with active APO and live speech-like metering.
#[must_use]
pub fn make_golden_line() -> AppState {
    let mut s = AppState::from_model(base_model());
    s.live = scene_live_frame(1.4, false);
    s
}

/// Golden Line with the Surgical Pro Rack drawer expanded at the bottom.
#[must_use]
pub fn make_pro_drawer_open() -> AppState {
    let mut s = make_golden_line();
    s.ui.pro_drawer_open = true;
    s.model.pins.pin(ParamId::CompThreshDb);
    s
}

/// Auto-Tune wizard: Prepare step.
#[must_use]
pub fn make_auto_tune_prepare() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Prepare { remaining_secs: 0.7 },
        progress: 0.0,
        voiced_ms: 0,
        level_db: -48.0,
        prompt_text: String::from("Find a quiet spot, then press Start to measure room noise."),
    };
    s
}

/// Auto-Tune wizard: Silence step (3 s room noise measurement).
#[must_use]
pub fn make_auto_tune_silence() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Silence { remaining_secs: 1.8 },
        progress: 0.15,
        voiced_ms: 0,
        level_db: -51.4,
        prompt_text: String::from("Stay quiet for a moment while Tine measures your room noise floor."),
    };
    s
}

/// Auto-Tune wizard: Speak step (5 s vocal reference).
#[must_use]
pub fn make_auto_tune_speak() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Speak { remaining_secs: 3.2 },
        progress: 0.60,
        voiced_ms: 2400,
        level_db: -21.5,
        prompt_text: String::from("The quick brown fox jumps over the lazy dog."),
    };
    s
}

/// Auto-Tune wizard: Analyzing step.
#[must_use]
pub fn make_auto_tune_analyzing() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Analyzing,
        progress: 0.92,
        voiced_ms: 5400,
        level_db: -24.0,
        prompt_text: String::from("Analyzing vocal crest factor and spectral tilt..."),
    };
    s
}

/// Auto-Tune wizard: Results step with before/after parameter diffs.
#[must_use]
pub fn make_auto_tune_results() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Results,
        progress: 1.0,
        voiced_ms: 5400,
        level_db: -18.0,
        prompt_text: String::from("Calibration complete."),
    };
    s.model.calibration = Some(CalibrationUi {
        noise_rms_db: -51.4,
        speech_rms_db: -21.5,
        simulated_peak_db: -4.2,
        simulated_rms_db: -16.8,
        changes: vec![
            CalibrationParamDiff {
                param: ParamId::InputGainDb,
                label: "Pre-Gain",
                before: 0.0,
                after: 3.5,
                unit: "dB",
            },
            CalibrationParamDiff {
                param: ParamId::GateThresholdDb,
                label: "Gate Threshold",
                before: -48.0,
                after: -45.4,
                unit: "dBFS",
            },
            CalibrationParamDiff {
                param: ParamId::CompThreshDb,
                label: "Compressor Threshold",
                before: -24.0,
                after: -19.5,
                unit: "dBFS",
            },
        ],
    });
    s
}

/// Auto-Tune wizard: Failed step (not enough speech, per the T11 `CalFailure` contract).
#[must_use]
pub fn make_auto_tune_failed() -> AppState {
    let mut s = make_golden_line();
    s.model.autotune = AutoTuneUi {
        step: AutoTuneStep::Failed(CalFailureUi::NotEnoughSpeech { voiced_ms: 800 }),
        progress: 0.0,
        voiced_ms: 800,
        level_db: -58.0,
        prompt_text: String::new(),
    };
    s
}

/// Golden Line with a macro node hovered (spring tooltip shown).
#[must_use]
pub fn make_golden_line_tooltip() -> AppState {
    let mut s = make_golden_line();
    s.ui.hovered_macro = Some(Macro::Body);
    s
}

/// Endpoint selected, but Windows APO is not yet installed.
#[must_use]
pub fn make_apo_not_installed() -> AppState {
    let mut s = make_golden_line();
    s.model.apo = ApoBadge::NotInstalled;
    s
}

/// First-run onboarding overlay, step 1 (pick the microphone).
#[must_use]
pub fn make_onboarding() -> AppState {
    let mut s = make_golden_line();
    s.model.first_run = true;
    s.model.apo = ApoBadge::NotInstalled;
    s.ui.onboarding_step = 1;
    s
}

/// Onboarding step 2: APO install succeeded.
#[must_use]
pub fn make_onboarding_install() -> AppState {
    let mut s = make_onboarding();
    s.ui.onboarding_step = 2;
    s.model.apo = ApoBadge::Active;
    s
}

/// Onboarding step 2: APO install failed with a typed error (remedies shown).
#[must_use]
pub fn make_onboarding_install_error() -> AppState {
    let mut s = make_onboarding();
    s.ui.onboarding_step = 2;
    s.model.apo = ApoBadge::Error(String::from("denied"));
    s
}

/// Onboarding step 2: install in progress (UAC prompt pending).
#[must_use]
pub fn make_onboarding_installing() -> AppState {
    let mut s = make_onboarding();
    s.ui.onboarding_step = 2;
    s.ui.apo_installing = true;
    s
}

/// Onboarding step 3: Auto-Tune (skippable).
#[must_use]
pub fn make_onboarding_autotune() -> AppState {
    let mut s = make_onboarding();
    s.ui.onboarding_step = 3;
    s
}

/// Onboarding step 4: finish ("open Discord/OBS").
#[must_use]
pub fn make_onboarding_finish() -> AppState {
    let mut s = make_onboarding();
    s.ui.onboarding_step = 4;
    s
}

fn apply_golden_line(s: &mut AppState) {
    *s = make_golden_line();
}

fn apply_pro_drawer_open(s: &mut AppState) {
    *s = make_pro_drawer_open();
}

fn apply_auto_tune_wizard(s: &mut AppState) {
    *s = make_auto_tune_speak();
}

fn apply_auto_tune_prepare(s: &mut AppState) {
    *s = make_auto_tune_prepare();
}

fn apply_auto_tune_silence(s: &mut AppState) {
    *s = make_auto_tune_silence();
}

fn apply_auto_tune_speak(s: &mut AppState) {
    *s = make_auto_tune_speak();
}

fn apply_auto_tune_analyzing(s: &mut AppState) {
    *s = make_auto_tune_analyzing();
}

fn apply_auto_tune_results(s: &mut AppState) {
    *s = make_auto_tune_results();
}

fn apply_auto_tune_failed(s: &mut AppState) {
    *s = make_auto_tune_failed();
}

fn apply_golden_line_tooltip(s: &mut AppState) {
    *s = make_golden_line_tooltip();
}

fn apply_apo_not_installed(s: &mut AppState) {
    *s = make_apo_not_installed();
}

fn apply_onboarding(s: &mut AppState) {
    *s = make_onboarding();
}

fn apply_onboarding_install(s: &mut AppState) {
    *s = make_onboarding_install();
}

fn apply_onboarding_installing(s: &mut AppState) {
    *s = make_onboarding_installing();
}

fn apply_onboarding_install_error(s: &mut AppState) {
    *s = make_onboarding_install_error();
}

fn apply_onboarding_autotune(s: &mut AppState) {
    *s = make_onboarding_autotune();
}

fn apply_onboarding_finish(s: &mut AppState) {
    *s = make_onboarding_finish();
}

/// Named scene callback type matching [`rune::AppSpec::scenes`].
pub type SceneEntry = (&'static str, fn(&mut AppState));

/// Scene registry passed to [`rune::AppSpec::scenes`].
pub const SCENES: &[SceneEntry] = &[
    ("golden_line", apply_golden_line),
    ("golden_line_tooltip", apply_golden_line_tooltip),
    ("pro_drawer_open", apply_pro_drawer_open),
    ("auto_tune_wizard", apply_auto_tune_wizard),
    ("auto_tune_prepare", apply_auto_tune_prepare),
    ("auto_tune_silence", apply_auto_tune_silence),
    ("auto_tune_speak", apply_auto_tune_speak),
    ("auto_tune_analyzing", apply_auto_tune_analyzing),
    ("auto_tune_results", apply_auto_tune_results),
    ("auto_tune_failed", apply_auto_tune_failed),
    ("apo_not_installed", apply_apo_not_installed),
    ("onboarding", apply_onboarding),
    ("onboarding_install", apply_onboarding_install),
    ("onboarding_installing", apply_onboarding_installing),
    ("onboarding_install_error", apply_onboarding_install_error),
    ("onboarding_autotune", apply_onboarding_autotune),
    ("onboarding_finish", apply_onboarding_finish),
];
