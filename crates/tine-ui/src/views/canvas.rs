//! Center "Tine Deck 2.3" DJ / Broadcast Mixer Console:
//! - Left strip (`USB / MIC`): Pre-Gain (`LEVEL`) & High-Pass (`LOW CUT`) rotary dials + quick `VOICE FX` presets.
//! - Four Macro Channel Strips (`PURITY`, `BODY`, `PRESENCE`, `LEVELER`): tactile `CUE` stage switches,
//!   72 pt porcelain-white rotary dials with radial LED tick collars & spring-rotated caps, live-linked
//!   parameter sub-knobs, and per-stage gain reduction LEDs.
//! - Right Master / FX Bay (`TINE-750MK2`): embedded OLED 32-band spectrum & memoized EQ response scope,
//!   twin peak-hold VU meters, chunky `THEME` & `MONITOR` slide switches, and boxed `VOICE PROFILE` stepper.

use std::cell::RefCell;

use rune::{
    Align, Color, Cx, Drag, El, Key, KeyEvent, LineCap, Phase, PopupMenu, Shape, Side, Transition, Tx, canvas, col, el,
    memo, popup_menu_supported, row, text,
};
use tine_dsp::curve::response_curve;
use tine_dsp::macros::{ExplainBuf, ExplainRow, Macro as DspMacro, MacroMapper};
use tine_dsp::presets;
use tine_protocol::params::{Band, DenoiseEngine, DspParams, EQ_BAND_COUNT, ParamId, ParamSpec, Scale, Unit};
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides};
use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::intent::Intent;
use crate::model::{CalibrationUi, Macro, UiModel};
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};
use crate::views::kit;

/// Lowest frequency drawn on the log axis.
pub const F_MIN_HZ: f32 = 80.0;
/// Highest frequency drawn on the log axis.
pub const F_MAX_HZ: f32 = 20_000.0;
/// Number of response-curve evaluation points along the axis.
pub const CURVE_POINTS: usize = 128;
/// Native sample rate the UI evaluates the response curve at (D-006).
const SAMPLE_RATE_HZ: f32 = 48_000.0;
/// Vertical display range of the response curve, in dB.
const DB_MAX: f32 = 18.0;

/// Macro units per pixel of knob drag (`1 px = 0.004`).
const DRAG_PER_PX: f32 = 0.004;
/// Sub-knob drag travel in pixels for full `0.0..=1.0` sweep.
const SUB_KNOB_DRAG_PX: f32 = 150.0;

/// Converts pointer drag `(dx, dy)` into effective clockwise knob travel in pixels.
///
/// - Sliding right (`+dx`) turns clockwise (increases); sliding left (`-dx`) turns counter-clockwise (decreases).
/// - Sliding up (`-dy`) turns clockwise (increases); sliding down (`+dy`) turns counter-clockwise (decreases).
/// - Small vertical drift during a horizontal swipe (or horizontal drift during a vertical swipe)
///   is ignored within a 35% cone, while larger diagonal motion blends smoothly without any
///   sudden sign-flip jump.
#[must_use]
pub fn knob_drag_px(dx: f32, dy: f32) -> f32 {
    let v = -dy;
    if (dx >= 0.0 && v >= 0.0) || (dx <= 0.0 && v <= 0.0) {
        dx + v
    } else if v.abs() <= dx.abs() * 0.35 {
        dx
    } else if dx.abs() <= v.abs() * 0.35 {
        v
    } else {
        dx + v
    }
}
/// Peak-hold window in telemetry frames (1.5 s at 60 Hz).
const HOLD_FRAMES: u64 = 90;
/// dBFS at or above which the clip LED lights.
const CLIP_DBFS: f32 = -0.3;
/// Meter display range bottom.
const METER_MIN_DB: f32 = -60.0;

/// Inset of the Leveler node from the right plot edge (kept for layout helper tests).
const LEVELER_INSET: f32 = 25.0;

/// Color runs the response curve is split into for its amber → coral gradient.
const CURVE_RUNS: usize = 16;

/// Fallback OLED scope size before the first layout settles.
const PLOT_FALLBACK: [f32; 2] = [200.0, 120.0];

/// Archetypes in stepper cycle order.
const ARCHETYPE_ORDER: [Archetype; 4] = [
    Archetype::Dynamic,
    Archetype::Condenser,
    Archetype::Headset,
    Archetype::Laptop,
];

/// Voice FX quick macro scenes (Decky-Deck `SOUND FX` style).
const VOICE_FX_PRESETS: [(&str, [f32; 4]); 3] = [
    ("Broadcast", [0.64, 0.72, 0.68, 0.70]),
    ("Natural", [0.40, 0.46, 0.44, 0.36]),
    ("Crisp", [0.70, 0.20, 0.84, 0.66]),
];

/// 0..1 x position of `hz` on the logarithmic frequency axis.
#[must_use]
pub fn freq_x(hz: f32) -> f32 {
    ((hz / F_MIN_HZ).log10() / (F_MAX_HZ / F_MIN_HZ).log10()).clamp(0.0, 1.0)
}

/// Fixed x position of a macro node, in plot points.
///
/// Purity sits in the ~150 Hz zone, Body at ~250 Hz, Presence at ~4 kHz, and the
/// Leveler rides the right edge as a level node.
#[must_use]
pub fn macro_x(m: Macro, plot_w: f32) -> f32 {
    match m {
        Macro::Purity => freq_x(150.0) * plot_w,
        Macro::Body => freq_x(250.0) * plot_w,
        Macro::Presence => freq_x(4000.0) * plot_w,
        Macro::Leveler => (plot_w - LEVELER_INSET).max(freq_x(4000.0) * plot_w + 80.0),
    }
}

/// Explanation rows for a macro tooltip (e.g. `"+2.9 dB @ 110 Hz"`).
///
/// Rows come straight from [`MacroMapper::explain`] for the current archetype
/// preset: the parameters the macro drives, the effective base (`before`), the
/// value the macro maps to (`after`), and a `pinned (Pro)` marker when the
/// parameter is pinned (D-007).
#[must_use]
pub fn macro_explanation_rows(m: Macro, model: &UiModel) -> Vec<String> {
    let preset = presets::builtin(model.archetype);
    let cal = model
        .active_calibration
        .or(model.pending_calibration)
        .or_else(|| model.calibration.as_ref().map(|c| calibration_from_ui(c, &preset.base)));
    let mut buf = ExplainBuf::new();
    MacroMapper::explain(preset, cal.as_ref(), model.macros, model.pins, dsp_macro(m), &mut buf);
    buf.rows().iter().map(|row| explain_row_string(row, model)).collect()
}

pub(crate) fn calibration_from_ui(cal: &CalibrationUi, base: &DspParams) -> Calibration {
    let find = |target: ParamId, fallback: f32| {
        cal.changes
            .iter()
            .find(|d| d.param == target)
            .map_or(fallback, |d| d.after)
    };
    Calibration {
        noise_rms_db: cal.noise_rms_db,
        noise_peak_db: cal.noise_rms_db,
        speech_rms_db: cal.speech_rms_db,
        crest_db: 12.0,
        tilt_db: 0.0,
        derived: CalibrationOverrides {
            input_gain_db: find(ParamId::InputGainDb, base.input_gain_db),
            gate_threshold_db: find(ParamId::GateThresholdDb, base.gate_threshold_db),
            denoise_mix: find(ParamId::DenoiseMix, base.denoise_mix),
            comp_thresh_db: find(ParamId::CompThreshDb, base.comp_thresh_db),
            comp_makeup_db: find(ParamId::CompMakeupDb, base.comp_makeup_db),
        },
    }
}

/// Maps `tine-ui`'s macro node onto the DSP macro enum of the same node.
const fn dsp_macro(m: Macro) -> DspMacro {
    match m {
        Macro::Purity => DspMacro::Purity,
        Macro::Body => DspMacro::Body,
        Macro::Presence => DspMacro::Presence,
        Macro::Leveler => DspMacro::Leveler,
    }
}

/// Formats one [`ExplainRow`] the way `docs/project/05-ui-ux.md` shows tooltips:
/// EQ gains as `"+2.9 dB @ 110 Hz"`, toggles as `"on" / "off"`, ratios as
/// `":1"`, everything else as `"before → after"` with its unit.
fn explain_row_string(row: &ExplainRow, model: &UiModel) -> String {
    let spec = row.id.spec();
    let body = match row.id {
        ParamId::EqGainDb(band) => {
            let hz = model.params.eq_freq_hz.get(band.index()).map_or(1000.0, |f| *f);
            format!("{:+.1} dB @ {hz:.0} Hz", row.after)
        }
        ParamId::DeesserEnabled | ParamId::CompAutoMakeup => {
            format!("{}: {}", spec.label, if row.after >= 0.5 { "on" } else { "off" })
        }
        ParamId::CompRatio => format!("{}: {:.1}:1", spec.label, row.after),
        ParamId::DenoiseMix => format!("{}: {:.0}%", spec.label, row.after * 100.0),
        _ => {
            let unit = match spec.unit {
                Unit::Db => " dB",
                Unit::Dbfs => " dBFS",
                Unit::Hz => " Hz",
                Unit::Ms => " ms",
                Unit::Ratio | Unit::None => "",
            };
            format!("{}: {:.1} \u{2192} {:.1}{unit}", spec.label, row.before, row.after)
        }
    };
    if row.pinned {
        format!("{body} \u{00b7} pinned (Pro)")
    } else {
        body
    }
}

/// FNV-1a hash of the `DspParams` fields the response curve depends on.
#[must_use]
pub fn curve_params_hash(p: &DspParams) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(u64::from(p.hpf_enabled));
    mix(u64::from(p.hpf_freq_hz.to_bits()));
    for b in 0..EQ_BAND_COUNT {
        mix(u64::from(p.eq_type[b] as u32));
        mix(u64::from(p.eq_freq_hz[b].to_bits()));
        mix(u64::from(p.eq_gain_db[b].to_bits()));
        mix(u64::from(p.eq_q[b].to_bits()));
    }
    mix(u64::from(p.bypass));
    h
}

struct CurveCache {
    hash: u64,
    db: [f32; CURVE_POINTS],
    freqs: [f32; CURVE_POINTS],
    hits: u64,
    evals: u64,
}

thread_local! {
    static CURVE: RefCell<CurveCache> = RefCell::new(CurveCache {
        hash: u64::MAX,
        db: [0.0; CURVE_POINTS],
        freqs: curve_freqs(),
        hits: 0,
        evals: 0,
    });
}

fn curve_freqs() -> [f32; CURVE_POINTS] {
    let mut out = [0.0_f32; CURVE_POINTS];
    for (i, slot) in out.iter_mut().enumerate() {
        let t = CURVE_POINTS as f32;
        *slot = F_MIN_HZ * (F_MAX_HZ / F_MIN_HZ).powf(i as f32 / (t - 1.0));
    }
    out
}

/// Response curve in dB over 128 log-spaced points, memoized on the params hash:
/// the DSP evaluation only runs when the curve-relevant parameters changed.
#[must_use]
pub fn curve_db(params: &DspParams) -> [f32; CURVE_POINTS] {
    CURVE.with(|c| {
        let mut c = c.borrow_mut();
        let hash = curve_params_hash(params);
        if c.hash == hash {
            c.hits += 1;
        } else {
            if params.bypass {
                c.db = [0.0; CURVE_POINTS];
            } else {
                let (freqs, mut db) = (c.freqs, c.db);
                response_curve(params, SAMPLE_RATE_HZ, &freqs, &mut db);
                c.db = db;
            }
            c.hash = hash;
            c.evals += 1;
        }
        c.db
    })
}

/// Cache statistics for tests and perf evidence: `(hits, evals)`.
#[must_use]
pub fn curve_cache_stats() -> (u64, u64) {
    CURVE.with(|c| {
        let c = c.borrow();
        (c.hits, c.evals)
    })
}

/// Per-meter peak-hold state.
#[derive(Clone, Copy)]
struct PeakHold {
    hold_db: f32,
    hold_until_seq: u64,
}

impl PeakHold {
    const fn new() -> Self {
        Self {
            hold_db: METER_MIN_DB,
            hold_until_seq: 0,
        }
    }

    /// Observes one telemetry frame and returns the held peak in dBFS.
    fn observe(&mut self, peak_db: f32, seq: u64) -> f32 {
        if seq >= self.hold_until_seq || peak_db >= self.hold_db {
            self.hold_db = peak_db;
            self.hold_until_seq = seq + HOLD_FRAMES;
        }
        self.hold_db
    }
}

/// Smoothed meter ballistics + ~5 Hz numeric readout throttle.
#[derive(Clone, Copy)]
struct MeterBallistics {
    last_seq: u64,
    smooth_in: f32,
    smooth_out: f32,
    text_seq: u64,
    text_in: f32,
    text_out: f32,
}

impl MeterBallistics {
    const fn new() -> Self {
        Self {
            last_seq: u64::MAX,
            smooth_in: METER_MIN_DB,
            smooth_out: METER_MIN_DB,
            text_seq: u64::MAX,
            text_in: METER_MIN_DB,
            text_out: METER_MIN_DB,
        }
    }

    fn observe(&mut self, tel: &AudioTelemetryFrame) -> (f32, f32, f32, f32) {
        if self.last_seq == u64::MAX || tel.sequence < self.last_seq {
            self.smooth_in = tel.input_rms_db;
            self.smooth_out = tel.output_rms_db;
            self.text_in = tel.input_rms_db;
            self.text_out = tel.output_rms_db;
            self.last_seq = tel.sequence;
            self.text_seq = tel.sequence;
        } else if tel.sequence != self.last_seq {
            // Fast attack (~22 ms), smooth decay (~320 ms) at 60 Hz.
            const ALPHA_ATTACK: f32 = 0.55;
            const ALPHA_DECAY: f32 = 0.05;
            let step = |cur: f32, target: f32| {
                let a = if target > cur { ALPHA_ATTACK } else { ALPHA_DECAY };
                cur + a * (target - cur)
            };
            self.smooth_in = step(self.smooth_in, tel.input_rms_db);
            self.smooth_out = step(self.smooth_out, tel.output_rms_db);
            self.last_seq = tel.sequence;

            // Update numeric text at ~5 Hz (every 12 frames) so digits don't blur.
            if tel.sequence.saturating_sub(self.text_seq) >= 12 {
                self.text_in = self.smooth_in;
                self.text_out = self.smooth_out;
                self.text_seq = tel.sequence;
            }
        }
        (self.smooth_in, self.smooth_out, self.text_in, self.text_out)
    }
}

thread_local! {
    static PEAKS: RefCell<[PeakHold; 2]> = const { RefCell::new([PeakHold::new(); 2]) };
    static PEAK_OBSERVED_SEQ: RefCell<u64> = const { RefCell::new(u64::MAX) };
    static METERS_SMOOTH: RefCell<MeterBallistics> = const { RefCell::new(MeterBallistics::new()) };
}

fn smoothed_meters(tel: &AudioTelemetryFrame) -> (f32, f32, f32, f32) {
    METERS_SMOOTH.with_borrow_mut(|m| m.observe(tel))
}

/// Peak-held levels `(input, output)` in dBFS for the current telemetry frame.
fn held_peaks(tel: &AudioTelemetryFrame) -> (f32, f32) {
    PEAKS.with_borrow_mut(|p| {
        let repeat = PEAK_OBSERVED_SEQ.with_borrow(|seen| *seen == tel.sequence);
        if repeat {
            return (p[0].hold_db, p[1].hold_db);
        }
        PEAK_OBSERVED_SEQ.with_borrow_mut(|seen| *seen = tel.sequence);
        let a = p[0].observe(tel.input_peak_db, tel.sequence);
        let b = p[1].observe(tel.output_peak_db, tel.sequence);
        (a, b)
    })
}

fn meter_color(db: f32, t: TineTheme) -> Color {
    if db > -1.5 {
        t.meter_red
    } else if db > -12.0 {
        t.meter_amber
    } else {
        t.meter_green
    }
}

/// dBFS → 0..1 meter fill.
const fn db_norm(db: f32) -> f32 {
    ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0)
}

/// Renders the 6-column Tine Deck 2.3 Mixer Console.
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = s.theme();
    let is_light = s.ui.theme_mode.is_light();
    let compact = s.ui.pro_drawer_open;

    let silent_fallback = AudioTelemetryFrame {
        input_rms_db: METER_MIN_DB,
        input_peak_db: METER_MIN_DB,
        output_rms_db: METER_MIN_DB,
        output_peak_db: METER_MIN_DB,
        ..AudioTelemetryFrame::default()
    };
    let tel = if s.live.telemetry.sequence == 0
        && s.live.telemetry.input_rms_db == 0.0
        && s.live.telemetry.output_rms_db == 0.0
    {
        &silent_fallback
    } else {
        &s.live.telemetry
    };
    let held = held_peaks(tel);
    let (smooth_in, smooth_out, text_in, text_out) = smoothed_meters(tel);

    // Settled OLED scope size from the previous layout pass (re-runs the view on resize).
    let [plot_w, plot_h] = cx
        .layout_with("canvas/plot", |r| [r[2] as i32, r[3] as i32])
        .map_or(PLOT_FALLBACK, |[w, h]| [w.max(120) as f32, h.max(80) as f32]);

    let curve_hash = curve_params_hash(&s.model.params);
    let curve = curve_db(&s.model.params);

    let deck = row()
        .key("canvas/deck")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(0.0)
        .child(col_input_mic(s, compact, t))
        .child(col_divider("canvas/div-1", t))
        .child(col_purity(s, tel, compact, t))
        .child(col_divider("canvas/div-2", t))
        .child(col_body(s, compact, t))
        .child(col_divider("canvas/div-3", t))
        .child(col_presence(s, tel, compact, t))
        .child(col_divider("canvas/div-4", t))
        .child(col_leveler(s, tel, compact, t))
        .child(col_divider("canvas/div-5", t))
        .child(col_master_bay(
            s,
            tel,
            (smooth_in, smooth_out),
            (text_in, text_out),
            held,
            &curve,
            curve_hash,
            plot_w,
            plot_h,
            compact,
            is_light,
            t,
        ));

    let mut card = kit::card("canvas", t)
        .px(spacing::SM + 2.0)
        .py(spacing::SM)
        .gap(if compact { 0.0 } else { spacing::XS })
        .child(deck);

    if !compact {
        card = card.child(coach_bar(s, t));
    }

    card = if compact {
        card.h(196.0)
    } else {
        card.grow(1.0).min_h(200.0)
    };

    if let Some(m) = s.ui.hovered_macro {
        card = card.child(tooltip_for(m, &s.model, t));
    }

    card
}

/// Full-height hairline column divider between mixer channel strips.
fn col_divider(key: &'static str, t: TineTheme) -> El {
    el().key(key).w(1.0).h_full().fill(t.border)
}

// ---------------------------------------------------------------------------
// Column 1: USB / MIC Input & Voice FX Strip
// ---------------------------------------------------------------------------

fn col_input_mic(s: &AppState, compact: bool, t: TineTheme) -> El {
    let mut c = col()
        .key("deck/col-input")
        .w(118.0)
        .h_full()
        .px(spacing::SM)
        .py(spacing::XS)
        .gap(4.0)
        .center();

    c = c
        .child(
            row()
                .key("deck/usb-tag")
                .gap(4.0)
                .center()
                .child(text("▲ USB · MIC").font(typography::EYEBROW).color(t.text_dim)),
        )
        .child(text("INPUT").font(typography::LABEL).color(t.text))
        .child(param_knob(
            "deck/knob/input-gain",
            "LEVEL",
            ParamId::InputGainDb,
            s.model.params.input_gain_db,
            false,
            if compact { 56.0 } else { 58.0 },
            "-24",
            "+36",
            format!("{:+.1} dB", s.model.params.input_gain_db),
            s.model.pins.is_pinned(ParamId::InputGainDb),
            s.ui.hovered_param == Some(ParamId::InputGainDb),
            t,
        ));

    if compact {
        c = c.child(el().key("deck/input-spacer-compact").grow(1.0));
    } else {
        c = c
            .child(param_knob(
                "deck/knob/hpf",
                "LOW CUT",
                ParamId::HpfFreqHz,
                s.model.params.hpf_freq_hz,
                false,
                48.0,
                "20",
                "200",
                format!("{:.0} Hz", s.model.params.hpf_freq_hz),
                s.model.pins.is_pinned(ParamId::HpfFreqHz),
                s.ui.hovered_param == Some(ParamId::HpfFreqHz),
                t,
            ))
            .child(el().key("deck/input-spacer").grow(1.0))
            .child(sound_fx_stack(s, t));
    }

    c
}

/// Decky-Deck `SOUND FX` stack: quick voice character macro presets + Reset.
fn sound_fx_stack(s: &AppState, t: TineTheme) -> El {
    let cur = s.model.macros;
    let near = |a: [f32; 4]| {
        (cur.purity - a[0]).abs() < 0.03
            && (cur.body - a[1]).abs() < 0.03
            && (cur.presence - a[2]).abs() < 0.03
            && (cur.leveler - a[3]).abs() < 0.03
    };

    let mut stack = col()
        .key("deck/sound-fx")
        .w_full()
        .gap(4.0)
        .center()
        .child(text("VOICE FX").font(typography::EYEBROW).color(t.text_dim));

    for (name, vals) in VOICE_FX_PRESETS {
        let active = near(vals);
        let key = format!("deck/fx-{}", name.to_ascii_lowercase());
        stack = stack.child(
            kit::deck_pill(
                key,
                "button",
                format!("Voice FX preset: {name}"),
                name,
                active,
                t,
                move |s| {
                    s.set_macro_optimistic(Macro::Purity, vals[0]);
                    s.set_macro_optimistic(Macro::Body, vals[1]);
                    s.set_macro_optimistic(Macro::Presence, vals[2]);
                    s.set_macro_optimistic(Macro::Leveler, vals[3]);
                },
            )
            .w_full(),
        );
    }

    let defaults = presets::builtin(s.model.archetype).macro_defaults;
    let is_default = near([defaults.purity, defaults.body, defaults.presence, defaults.leveler]);
    stack.child(
        kit::deck_pill(
            "deck/fx-reset",
            "button",
            "Reset macros and unpin all controls to archetype default",
            "Neutral",
            is_default && s.model.pins.is_empty() && !VOICE_FX_PRESETS.iter().any(|(_, v)| near(*v)),
            t,
            |s| {
                s.reset_deck_to_neutral();
            },
        )
        .w_full(),
    )
}

// ---------------------------------------------------------------------------
// Columns 2–5: Channel Strips (PURITY, BODY, PRESENCE, LEVELER)
// ---------------------------------------------------------------------------

fn col_purity(s: &AppState, tel: &AudioTelemetryFrame, compact: bool, t: TineTheme) -> El {
    let gate_on = s.model.params.gate_enabled;
    let engine_label = match s.model.denoise_engine {
        DenoiseEngine::Rnnoise => "RNNoise ▾",
        DenoiseEngine::DeepFilterNet3 => "Studio AI ▾",
        DenoiseEngine::Off => "Low-Lat ▾",
    };

    let mut c = col()
        .key("deck/col-purity")
        .grow(1.0)
        .min_w(104.0)
        .h_full()
        .px(spacing::SM)
        .py(spacing::XS)
        .gap(4.0)
        .center()
        .child(kit::cue_pad(
            "deck/cue-gate",
            "Noise gate cue",
            if gate_on { "CUE · GATE" } else { "GATE OFF" },
            gate_on,
            t,
            |s| {
                let next = if s.model.params.gate_enabled { 0.0 } else { 1.0 };
                s.set_param_optimistic(ParamId::GateEnabled, next);
            },
        ));

    if !compact {
        c = c.child(denoise_mode_pill(s, engine_label, t));
    }

    c = c.child(macro_knob(
        Macro::Purity,
        "PURITY",
        s.model.macros.purity,
        s.ui.hovered_macro == Some(Macro::Purity),
        if compact { 64.0 } else { 68.0 },
        "0%",
        "100%",
        format!("{:.0}%", s.model.macros.purity * 100.0),
        t,
    ));

    if compact {
        c = c
            .child(el().key("deck/purity-spacer-compact").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-gate",
                "GATE",
                tel.gate_attenuation_db,
                t.deck_blue,
                t,
            ));
    } else {
        c = c
            .child(param_knob(
                "deck/knob/denoise",
                "DENOISE",
                ParamId::DenoiseMix,
                s.model.params.denoise_mix,
                false,
                48.0,
                "0%",
                "100%",
                format!("{:.0}%", s.model.params.denoise_mix * 100.0),
                s.model.pins.is_pinned(ParamId::DenoiseMix),
                s.ui.hovered_param == Some(ParamId::DenoiseMix),
                t,
            ))
            .child(param_knob(
                "deck/knob/gate-thresh",
                "GATE",
                ParamId::GateThresholdDb,
                s.model.params.gate_threshold_db,
                false,
                48.0,
                "-80",
                "-20",
                format!("{:.0} dB", s.model.params.gate_threshold_db),
                s.model.pins.is_pinned(ParamId::GateThresholdDb),
                s.ui.hovered_param == Some(ParamId::GateThresholdDb),
                t,
            ))
            .child(el().key("deck/purity-spacer").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-gate",
                "GATE GR",
                tel.gate_attenuation_db,
                t.deck_blue,
                t,
            ));
    }

    c
}

fn denoise_mode_pill(s: &AppState, label: &str, t: TineTheme) -> El {
    let current = s.model.denoise_engine;
    row()
        .key("deck/denoise-mode")
        .role("combobox")
        .label(format!("Denoiser backend: {}", current.display_name()))
        .focusable(false)
        .w_full()
        .h(22.0)
        .px(spacing::SM)
        .center()
        .radius(radii::SM + 2.0)
        .fill(t.surface_raised)
        .border(1.0, t.border)
        .hover(t.surface_hover)
        .press_scale(0.96)
        .cursor(rune::Cursor::Pointer)
        .on_press_tx(|s: &mut AppState, tx: &mut Tx| {
            if !popup_menu_supported() {
                let next = match s.model.denoise_engine {
                    DenoiseEngine::Rnnoise => DenoiseEngine::DeepFilterNet3,
                    DenoiseEngine::DeepFilterNet3 => DenoiseEngine::Off,
                    DenoiseEngine::Off => DenoiseEngine::Rnnoise,
                };
                s.model.denoise_engine = next;
                s.emit(Intent::SetDenoiseEngine(next));
                return;
            }
            let choices = DenoiseEngine::ALL
                .iter()
                .map(|e| (e.as_str().to_owned(), e.display_name().to_owned()));
            let active = s.model.denoise_engine.as_str().to_owned();
            let menu = PopupMenu::picker(choices, &active);
            tx.popup_menu(
                menu,
                "deck/denoise-mode",
                |s: &mut AppState, picked: Option<&str>, _tx| {
                    if let Some(id) = picked
                        && let Some(engine) = DenoiseEngine::from_name(id)
                    {
                        s.model.denoise_engine = engine;
                        s.emit(Intent::SetDenoiseEngine(engine));
                    }
                },
            );
        })
        .child(text(label.to_owned()).font(typography::CAPTION).color(t.text))
}

fn col_body(s: &AppState, compact: bool, t: TineTheme) -> El {
    let hpf_on = s.model.params.hpf_enabled;
    let low_gain = s.model.params.eq_gain_db[Band::B0.index()];
    let mud_gain = s.model.params.eq_gain_db[Band::B1.index()];
    let low_freq = s.model.params.eq_freq_hz[Band::B0.index()];
    let mud_freq = s.model.params.eq_freq_hz[Band::B1.index()];

    let mut c = col()
        .key("deck/col-body")
        .grow(1.0)
        .min_w(104.0)
        .h_full()
        .px(spacing::SM)
        .py(spacing::XS)
        .gap(4.0)
        .center()
        .child(kit::cue_pad(
            "deck/cue-hpf",
            "High-pass filter cue",
            if hpf_on { "CUE · HPF" } else { "HPF OFF" },
            hpf_on,
            t,
            |s| {
                let next = if s.model.params.hpf_enabled { 0.0 } else { 1.0 };
                s.set_param_optimistic(ParamId::HpfEnabled, next);
            },
        ));

    if !compact {
        c = c.child(
            row()
                .key("deck/body-tag")
                .w_full()
                .h(22.0)
                .center()
                .radius(radii::SM + 2.0)
                .fill(t.well)
                .border(1.0, t.border)
                .child(
                    text(format!("{low_freq:.0} / {mud_freq:.0} Hz"))
                        .font(typography::CAPTION)
                        .color(t.text_dim),
                ),
        );
    }

    c = c.child(macro_knob(
        Macro::Body,
        "BODY",
        s.model.macros.body,
        s.ui.hovered_macro == Some(Macro::Body),
        if compact { 64.0 } else { 68.0 },
        "Thin",
        "Warm",
        format!("{:.0}%", s.model.macros.body * 100.0),
        t,
    ));

    if compact {
        c = c.child(el().key("deck/body-spacer-compact").grow(1.0)).child(
            text(format!("LOW {low_gain:+.1} dB"))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );
    } else {
        c = c
            .child(param_knob(
                "deck/knob/eq-low",
                "LOW",
                ParamId::EqGainDb(Band::B0),
                low_gain,
                true,
                48.0,
                "-18",
                "+18",
                format!("{low_gain:+.1} dB"),
                s.model.pins.is_pinned(ParamId::EqGainDb(Band::B0)),
                s.ui.hovered_param == Some(ParamId::EqGainDb(Band::B0)),
                t,
            ))
            .child(param_knob(
                "deck/knob/eq-mud",
                "MUD CUT",
                ParamId::EqGainDb(Band::B1),
                mud_gain,
                true,
                48.0,
                "-18",
                "+18",
                format!("{mud_gain:+.1} dB"),
                s.model.pins.is_pinned(ParamId::EqGainDb(Band::B1)),
                s.ui.hovered_param == Some(ParamId::EqGainDb(Band::B1)),
                t,
            ))
            .child(el().key("deck/body-spacer").grow(1.0))
            .child(
                row().key("deck/body-footer").w_full().h(16.0).center().child(
                    text(if hpf_on {
                        format!("HPF {:.0} Hz", s.model.params.hpf_freq_hz)
                    } else {
                        String::from("HPF Off")
                    })
                    .font(typography::CAPTION)
                    .color(t.text_dim),
                ),
            );
    }

    c
}

fn col_presence(s: &AppState, tel: &AudioTelemetryFrame, compact: bool, t: TineTheme) -> El {
    let deess_on = s.model.params.deesser_enabled;
    let hi_gain = s.model.params.eq_gain_db[Band::B4.index()];
    let pres_gain = s.model.params.eq_gain_db[Band::B3.index()];

    let mut c = col()
        .key("deck/col-presence")
        .grow(1.0)
        .min_w(104.0)
        .h_full()
        .px(spacing::SM)
        .py(spacing::XS)
        .gap(4.0)
        .center()
        .child(kit::cue_pad(
            "deck/cue-deess",
            "De-Esser cue",
            if deess_on { "CUE · DE-ESS" } else { "DE-ESS OFF" },
            deess_on,
            t,
            |s| {
                let next = if s.model.params.deesser_enabled { 0.0 } else { 1.0 };
                s.set_param_optimistic(ParamId::DeesserEnabled, next);
            },
        ));

    if !compact {
        c = c.child(
            row()
                .key("deck/presence-tag")
                .w_full()
                .h(22.0)
                .center()
                .radius(radii::SM + 2.0)
                .fill(t.well)
                .border(1.0, t.border)
                .child(
                    text(format!("S-Band {:.1}k", s.model.params.deesser_freq_hz / 1000.0))
                        .font(typography::CAPTION)
                        .color(t.text_dim),
                ),
        );
    }

    c = c.child(macro_knob(
        Macro::Presence,
        "PRESENCE",
        s.model.macros.presence,
        s.ui.hovered_macro == Some(Macro::Presence),
        if compact { 64.0 } else { 68.0 },
        "Soft",
        "Crisp",
        format!("{:.0}%", s.model.macros.presence * 100.0),
        t,
    ));

    if compact {
        c = c
            .child(el().key("deck/presence-spacer-compact").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-deess",
                "DE-ESS",
                tel.deesser_reduction_db,
                t.deck_blue,
                t,
            ));
    } else {
        c = c
            .child(param_knob(
                "deck/knob/eq-air",
                "HI / AIR",
                ParamId::EqGainDb(Band::B4),
                hi_gain,
                true,
                48.0,
                "-18",
                "+18",
                format!("{hi_gain:+.1} dB"),
                s.model.pins.is_pinned(ParamId::EqGainDb(Band::B4)),
                s.ui.hovered_param == Some(ParamId::EqGainDb(Band::B4)),
                t,
            ))
            .child(param_knob(
                "deck/knob/eq-clarity",
                "CLARITY",
                ParamId::EqGainDb(Band::B3),
                pres_gain,
                true,
                48.0,
                "-18",
                "+18",
                format!("{pres_gain:+.1} dB"),
                s.model.pins.is_pinned(ParamId::EqGainDb(Band::B3)),
                s.ui.hovered_param == Some(ParamId::EqGainDb(Band::B3)),
                t,
            ))
            .child(el().key("deck/presence-spacer").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-deess",
                "DE-ESS GR",
                tel.deesser_reduction_db,
                t.deck_blue,
                t,
            ));
    }

    c
}

fn col_leveler(s: &AppState, tel: &AudioTelemetryFrame, compact: bool, t: TineTheme) -> El {
    let comp_on = s.model.params.comp_enabled;

    let mut c = col()
        .key("deck/col-leveler")
        .grow(1.0)
        .min_w(104.0)
        .h_full()
        .px(spacing::SM)
        .py(spacing::XS)
        .gap(4.0)
        .center()
        .child(kit::cue_pad(
            "deck/cue-comp",
            "Compressor cue",
            if comp_on { "CUE · COMP" } else { "COMP OFF" },
            comp_on,
            t,
            |s| {
                let next = if s.model.params.comp_enabled { 0.0 } else { 1.0 };
                s.set_param_optimistic(ParamId::CompEnabled, next);
            },
        ));

    if !compact {
        c = c.child(
            row()
                .key("deck/leveler-tag")
                .w_full()
                .h(22.0)
                .center()
                .radius(radii::SM + 2.0)
                .fill(t.well)
                .border(1.0, t.border)
                .child(
                    text(format!(
                        "{:.1}:1 · Lim {:.1}",
                        s.model.params.comp_ratio, s.model.params.limiter_ceiling_db
                    ))
                    .font(typography::CAPTION)
                    .color(t.text_dim),
                ),
        );
    }

    c = c.child(macro_knob(
        Macro::Leveler,
        "LEVELER",
        s.model.macros.leveler,
        s.ui.hovered_macro == Some(Macro::Leveler),
        if compact { 64.0 } else { 68.0 },
        "Open",
        "Radio",
        format!("{:.0}%", s.model.macros.leveler * 100.0),
        t,
    ));

    if compact {
        c = c
            .child(el().key("deck/leveler-spacer-compact").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-comp",
                "COMP",
                tel.comp_gain_reduction_db,
                t.accent,
                t,
            ));
    } else {
        c = c
            .child(param_knob(
                "deck/knob/comp-thresh",
                "THRESH",
                ParamId::CompThreshDb,
                s.model.params.comp_thresh_db,
                false,
                48.0,
                "-60",
                "0",
                format!("{:.0} dB", s.model.params.comp_thresh_db),
                s.model.pins.is_pinned(ParamId::CompThreshDb),
                s.ui.hovered_param == Some(ParamId::CompThreshDb),
                t,
            ))
            .child(param_knob(
                "deck/knob/comp-ratio",
                "RATIO",
                ParamId::CompRatio,
                s.model.params.comp_ratio,
                false,
                48.0,
                "1:1",
                "20:1",
                format!("{:.1}:1", s.model.params.comp_ratio),
                s.model.pins.is_pinned(ParamId::CompRatio),
                s.ui.hovered_param == Some(ParamId::CompRatio),
                t,
            ))
            .child(el().key("deck/leveler-spacer").grow(1.0))
            .child(mini_gr_meter(
                "deck/gr-comp",
                "COMP GR",
                tel.comp_gain_reduction_db,
                t.accent,
                t,
            ));
    }

    c
}

// ---------------------------------------------------------------------------
// Column 6: Master / FX Bay (OLED Spectrum & EQ Scope + VU + Switches + Stepper)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn col_master_bay(
    s: &AppState,
    tel: &AudioTelemetryFrame,
    smooth_rms: (f32, f32),
    text_rms: (f32, f32),
    held: (f32, f32),
    curve: &[f32; CURVE_POINTS],
    curve_hash: u64,
    plot_w: f32,
    plot_h: f32,
    compact: bool,
    is_light: bool,
    t: TineTheme,
) -> El {
    let mut header_row = row()
        .key("canvas/title-row")
        .w_full()
        .gap(spacing::XS)
        .center()
        .child(text("TINE-750MK2").font(typography::EYEBROW).color(t.text_dim));

    if s.model.active_calibration.is_some() {
        header_row = header_row.child(
            row()
                .key("canvas/calibrated-badge")
                .role("status")
                .label("Auto-Tune calibrated")
                .h(18.0)
                .px(6.0)
                .center()
                .radius(radii::PILL)
                .fill(t.accent_soft)
                .border(1.0, t.accent_glow)
                .child(text("✦ Cal").font(typography::CAPTION).color(t.accent_text)),
        );
    }

    header_row = header_row
        .child(el().key("canvas/title-spacer").grow(1.0))
        .child(readout("canvas/readout-in", "IN", text_rms.0, t))
        .child(readout("canvas/readout-out", "OUT", text_rms.1, t));

    let plot = col()
        .key("canvas/plot")
        .grow(1.0)
        .h_full()
        .min_h(if compact { 120.0 } else { 130.0 })
        .radius(radii::MD)
        .fill(t.well)
        .gradient(t.well, t.bg)
        .border(1.0, t.border)
        .clip_rounded(radii::MD)
        .child(grid_layer(is_light, t))
        .child(curve_layer(curve, curve_hash, plot_w, plot_h, is_light, t))
        .child(
            canvas(spectrum_shape(&s.live.spectrum, plot_w, plot_h, t))
                .key("canvas/spectrum")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
        )
        .child(db_labels(plot_h, t))
        .child(freq_labels(plot_w, plot_h, t));

    let meters = meters_panel(tel, smooth_rms, held, compact, t);

    let scope_strip = row()
        .key("canvas/plot-strip")
        .w_full()
        .grow(1.0)
        .min_h(if compact { 120.0 } else { 130.0 })
        .gap(spacing::SM)
        .child(plot)
        .child(meters);

    let mut master = col()
        .key("deck/col-master")
        .grow(1.85)
        .min_w(240.0)
        .h_full()
        .pl(spacing::SM + 2.0)
        .pr(spacing::XS)
        .py(spacing::XS)
        .gap(if compact { spacing::SM } else { 5.0 })
        .child(header_row)
        .child(scope_strip);

    if !compact {
        let switches_row = row()
            .key("deck/master-switches")
            .w_full()
            .gap(spacing::SM)
            .center()
            .child(
                row()
                    .key("deck/theme-switch-slot")
                    .grow(1.0)
                    .child(kit::deck_slide_switch(
                        "deck/theme-switch",
                        "Console theme mode: Dark or Light",
                        "CONSOLE",
                        ("DARK", "LIGHT"),
                        is_light,
                        t,
                        |s| {
                            s.toggle_theme();
                        },
                    )),
            )
            .child(
                row()
                    .key("deck/monitor-switch-slot")
                    .grow(1.0)
                    .child(kit::deck_slide_switch(
                        "deck/monitor-switch",
                        "Monitor return cue",
                        "RETURN",
                        ("MUTE", "HEAR"),
                        s.model.monitor.enabled,
                        t,
                        |s| {
                            s.model.monitor.enabled = !s.model.monitor.enabled;
                            s.emit(Intent::ToggleMonitor);
                        },
                    )),
            );

        master = master.child(switches_row).child(voice_profile_box(s, t));
    }

    master
}

/// Boxed `VOICE PROFILE` stepper module (modeled after `BEAT FX < 7 >` in Decky Deck 2.3).
fn voice_profile_box(s: &AppState, t: TineTheme) -> El {
    let arch_name = match s.model.archetype {
        Archetype::Dynamic => "Dynamic",
        Archetype::Condenser => "Condenser",
        Archetype::Headset => "Headset",
        Archetype::Laptop => "Laptop",
    };
    let idx = ARCHETYPE_ORDER
        .iter()
        .position(|&a| a == s.model.archetype)
        .unwrap_or(0);
    let prev_arch = ARCHETYPE_ORDER[(idx + ARCHETYPE_ORDER.len() - 1) % ARCHETYPE_ORDER.len()];
    let next_arch = ARCHETYPE_ORDER[(idx + 1) % ARCHETYPE_ORDER.len()];
    let keep_warm = !s.model.park_when_unused;

    col()
        .key("deck/voice-profile-box")
        .w_full()
        .px(spacing::SM)
        .py(spacing::XS + 2.0)
        .gap(spacing::XS + 2.0)
        .center()
        .radius(radii::MD)
        .fill(t.surface)
        .border(1.0, t.border_strong)
        .child(text("VOICE PROFILE").font(typography::EYEBROW).color(t.text_dim))
        .child(
            row()
                .key("deck/archetype-stepper")
                .w_full()
                .gap(4.0)
                .center()
                .child(
                    kit::deck_pill(
                        "deck/arch-prev",
                        "button",
                        "Previous microphone archetype",
                        "◀",
                        false,
                        t,
                        move |s| {
                            s.set_archetype_optimistic(prev_arch);
                        },
                    )
                    .w(30.0)
                    .px(0.0),
                )
                .child(
                    row()
                        .key("deck/arch-readout")
                        .grow(1.0)
                        .h(24.0)
                        .center()
                        .radius(radii::SM + 2.0)
                        .fill(t.well)
                        .border(1.0, t.border)
                        .child(text(arch_name).font(typography::LABEL).color(t.text)),
                )
                .child(
                    kit::deck_pill(
                        "deck/arch-next",
                        "button",
                        "Next microphone archetype",
                        "▶",
                        false,
                        t,
                        move |s| {
                            s.set_archetype_optimistic(next_arch);
                        },
                    )
                    .w(30.0)
                    .px(0.0),
                ),
        )
        .child(kit::deck_slide_switch(
            "deck/ptt-switch",
            "Idle power mode: Auto park or Keep warm for PTT",
            "STANDBY",
            ("AUTO", "WARM"),
            keep_warm,
            t,
            |s| {
                let next_park = !s.model.park_when_unused;
                s.model.park_when_unused = next_park;
                s.emit(Intent::SetParkWhenUnused(next_park));
            },
        ))
}

// ---------------------------------------------------------------------------
// Rotary Knobs & Mini LED Bars
// ---------------------------------------------------------------------------

/// Primary Macro Rotary Knob (`focusable(true)`, keeps exact `m.key()`, `role="slider"`,
/// `label="{} {value:.2}"`, `DRAG_PER_PX = 0.004`, arrows/Space/double-click).
#[allow(clippy::too_many_arguments)]
fn macro_knob(
    m: Macro,
    title: &'static str,
    value: f32,
    hovered: bool,
    size: f32,
    min_lbl: &'static str,
    max_lbl: &'static str,
    readout_str: String,
    t: TineTheme,
) -> El {
    let norm = value.clamp(0.0, 1.0);

    let dial = el()
        .key(m.key())
        .role("slider")
        .label(format!("{} {value:.2}", m.label()))
        .focusable(true)
        .size(size, size)
        .cursor(rune::Cursor::Grab)
        .child(
            canvas(kit::rotary_dial_backdrop(size, norm, false, true, t))
                .key(format!("{}/scale", m.key()))
                .absolute(0.0, 0.0)
                .size(size, size),
        )
        .child(kit::rotary_cap(
            format!("{}/cap", m.key()),
            size,
            norm,
            true,
            hovered,
            t,
        ))
        .on_key(move |s: &mut AppState, ev: &KeyEvent, _tx: &mut Tx| match &ev.key {
            Key::Up | Key::Down | Key::Left | Key::Right => {
                let delta = if ev.mods.shift { 0.1 } else { 0.01 };
                let signed = if matches!(ev.key, Key::Up | Key::Right) {
                    delta
                } else {
                    -delta
                };
                let next = m.get(s.model.macros) + signed;
                s.set_macro_optimistic(m, next);
                true
            }
            Key::Text(t) if t == " " => {
                s.toggle_bypass_optimistic();
                true
            }
            _ => false,
        })
        .on_hover_change(move |s: &mut AppState, inside: bool, tx: &mut Tx| {
            if inside {
                s.ui.hovered_macro = Some(m);
            } else if s.ui.hovered_macro == Some(m) {
                s.ui.hovered_macro = None;
            }
            tx.animated();
        })
        .on_double_press(move |s: &mut AppState| {
            let defaults = presets::builtin(s.model.archetype).macro_defaults;
            s.set_macro_optimistic(m, m.get(defaults));
        })
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| match d.phase {
            Phase::Down => {
                d.data = m.get(s.model.macros);
                s.ui.hovered_macro = Some(m);
                tx.animated();
            }
            Phase::Move => {
                s.set_macro_optimistic(m, d.data + knob_drag_px(d.dx, d.dy) * DRAG_PER_PX);
            }
            Phase::Up => {
                if d.cancelled() {
                    s.set_macro_optimistic(m, d.data);
                }
            }
        });

    col()
        .key(format!("{}/wrap", m.key()))
        .w_full()
        .gap(1.0)
        .center()
        .child(text(title).font(typography::EYEBROW).color(t.text))
        .child(dial)
        .child(
            row()
                .key(format!("{}/range", m.key()))
                .w(96.0)
                .center()
                .child(text(min_lbl).font(typography::CAPTION).color(t.text_dim))
                .child(el().key(format!("{}/range-l", m.key())).grow(1.0))
                .child(text(readout_str).font(typography::CAPTION).color(t.deck_blue_text))
                .child(el().key(format!("{}/range-r", m.key())).grow(1.0))
                .child(text(max_lbl).font(typography::CAPTION).color(t.text_dim)),
        )
}

/// Interactive secondary rotary sub-knob bound directly to a `ParamId` (`focusable(false)`).
/// Updates live whenever its parent macro turns, and pins `id` when dragged directly (D-007).
#[allow(clippy::too_many_arguments)]
fn param_knob(
    key: &'static str,
    title: &'static str,
    id: ParamId,
    value: f32,
    bipolar: bool,
    size: f32,
    min_lbl: &'static str,
    max_lbl: &'static str,
    readout_str: String,
    pinned: bool,
    hovered: bool,
    t: TineTheme,
) -> El {
    let spec = id.spec();
    let norm = to_pos(value, spec);

    let dial = el()
        .key(key)
        .role("slider")
        .label(format!("{} {readout_str}", spec.label))
        .focusable(false)
        .size(size, size)
        .radius(size * 0.5)
        .hover(rune::color::oklcha(0.54, 0.22, 262.0, 0.10))
        .cursor(rune::Cursor::Grab)
        .child(
            canvas(kit::rotary_dial_backdrop(size, norm, bipolar, false, t))
                .key(format!("{key}/scale"))
                .absolute(0.0, 0.0)
                .size(size, size),
        )
        .child(kit::rotary_cap(format!("{key}/cap"), size, norm, pinned, hovered, t))
        .on_hover_change(move |s: &mut AppState, inside: bool, tx: &mut Tx| {
            if inside {
                s.ui.hovered_param = Some(id);
            } else if s.ui.hovered_param == Some(id) {
                s.ui.hovered_param = None;
            }
            tx.animated();
        })
        .on_double_press(move |s: &mut AppState| {
            s.reset_param_or_unpin(id);
        })
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            let spec = id.spec();
            match d.phase {
                Phase::Down => {
                    d.data = to_pos(s.model.params.get(id), spec);
                    s.ui.hovered_param = Some(id);
                    tx.animated();
                }
                Phase::Move => {
                    let next_pos = (d.data + knob_drag_px(d.dx, d.dy) / SUB_KNOB_DRAG_PX).clamp(0.0, 1.0);
                    let next_val = from_pos(next_pos, spec);
                    if id == ParamId::DenoiseMix && next_val > 0.01 && s.model.denoise_engine == DenoiseEngine::Off {
                        s.model.denoise_engine = DenoiseEngine::Rnnoise;
                        s.emit(Intent::SetDenoiseEngine(DenoiseEngine::Rnnoise));
                    }
                    s.set_param_optimistic(id, next_val);
                }
                Phase::Up => {
                    if d.cancelled() {
                        s.set_param_optimistic(id, from_pos(d.data, spec));
                    }
                }
            }
        });

    col()
        .key(format!("{key}/wrap"))
        .w_full()
        .gap(1.0)
        .center()
        .child(
            row()
                .key(format!("{key}/head"))
                .gap(4.0)
                .center()
                .child(
                    text(title)
                        .font(typography::EYEBROW)
                        .color(if pinned { t.accent_text } else { t.text_dim }),
                )
                .child(text(readout_str).font(typography::CAPTION).color(t.text)),
        )
        .child(dial)
        .child(
            row()
                .key(format!("{key}/bounds"))
                .w(68.0)
                .center()
                .child(text(min_lbl).font(typography::CAPTION).color(t.text_dim))
                .child(el().key(format!("{key}/bounds-pad")).grow(1.0))
                .child(text(max_lbl).font(typography::CAPTION).color(t.text_dim)),
        )
}

fn to_pos(v: f32, spec: &ParamSpec) -> f32 {
    match spec.scale {
        Scale::Log => {
            let ratio = (spec.max / spec.min).ln();
            (((v.clamp(spec.min, spec.max) / spec.min).ln()) / ratio).clamp(0.0, 1.0)
        }
        _ => ((v - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0),
    }
}

fn from_pos(p: f32, spec: &ParamSpec) -> f32 {
    let p = p.clamp(0.0, 1.0);
    match spec.scale {
        Scale::Log => (spec.min * (spec.max / spec.min).powf(p)).clamp(spec.min, spec.max),
        _ => (spec.min + p * (spec.max - spec.min)).clamp(spec.min, spec.max),
    }
}

/// Compact per-channel Gain Reduction / activity LED bar.
fn mini_gr_meter(key: &'static str, label: &'static str, db: f32, fill_color: Color, t: TineTheme) -> El {
    let norm = (-db).clamp(0.0, 18.0) / 18.0;
    row()
        .key(key)
        .role("meter")
        .label(format!("{label} {:.1} dB", -db))
        .w_full()
        .gap(4.0)
        .center()
        .child(text(label).font(typography::EYEBROW).color(t.text_dim))
        .child(
            el().key(format!("{key}/track"))
                .grow(1.0)
                .h(6.0)
                .radius(3.0)
                .fill(t.well)
                .border(1.0, t.border)
                .clip_rounded(3.0)
                .child(
                    el().key(format!("{key}/fill"))
                        .grow(norm.max(0.001))
                        .h_full()
                        .fill(if norm > 0.01 { fill_color } else { t.well }),
                )
                .child(el().key(format!("{key}/rest")).grow((1.0 - norm).max(0.001))),
        )
        .child(text(format!("{:.1}", -db)).font(typography::CAPTION).color(t.text_dim))
}

// ---------------------------------------------------------------------------
// OLED Scope Layers, Meters & Spring Tooltip
// ---------------------------------------------------------------------------

/// Static grid layer: 0 dB reference line, ±6/±12 dB guides and 100/1k/10k gridlines.
fn grid_layer(is_light: bool, t: TineTheme) -> El {
    memo("canvas/grid", is_light, move || {
        let mut s = Shape::new();
        let vline = |s: Shape, x: f32| s.line([x, 0.0], [x, 1.0], 1.0, LineCap::Butt, t.border);
        let hline = |s: Shape, db: f32, color| {
            let y = 0.5 - db / (2.0 * DB_MAX);
            s.line([0.0, y], [1.0, y], 1.0, LineCap::Butt, color)
        };
        s = vline(s, freq_x(100.0));
        s = vline(s, freq_x(1000.0));
        s = vline(s, freq_x(10_000.0));
        s = hline(s, 12.0, t.border);
        s = hline(s, 6.0, t.border);
        // 0 dB reference line at mid height.
        s = hline(s, 0.0, t.accent_glow);
        s = hline(s, -6.0, t.border);
        s = hline(s, -12.0, t.border);
        canvas(s)
            .key("canvas/grid")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .cache()
    })
}

/// Response curve layer: wide translucent glow stroke under a solid spline.
fn curve_layer(db: &[f32; CURVE_POINTS], hash: u64, w: f32, h: f32, is_light: bool, t: TineTheme) -> El {
    memo("canvas/curve", (hash, w as i32, h as i32, is_light), move || {
        let pts: Vec<[f32; 2]> = curve_points(db, w, h);
        let halo = Color { a: 0.12, ..t.accent };
        let mut s = Shape::new()
            .stroke(pts.clone(), 12.0, halo)
            .stroke(pts.clone(), 5.0, t.accent_glow);
        // Main stroke: amber (lows) → coral (highs), drawn as overlapping runs.
        let per = pts.len().div_ceil(CURVE_RUNS);
        for run in 0..CURVE_RUNS {
            let start = run * per;
            let end = ((run + 1) * per + 1).min(pts.len());
            if start + 1 >= end {
                break;
            }
            let frac = run as f32 / (CURVE_RUNS - 1) as f32;
            s = s.stroke(pts[start..end].to_vec(), 2.5, mix(t.accent_hi, t.accent_lo, frac));
        }
        canvas(s)
            .key("canvas/curve")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
    })
}

/// Maps the cached dB curve into plot points.
fn curve_points(db: &[f32; CURVE_POINTS], w: f32, h: f32) -> Vec<[f32; 2]> {
    db.iter()
        .enumerate()
        .map(|(i, &d)| {
            let x = i as f32 / (CURVE_POINTS - 1) as f32 * w;
            let y = (0.5 - d / (2.0 * DB_MAX)) * h;
            [x, y.clamp(2.0, h - 2.0)]
        })
        .collect()
}

/// 32 smoothed spectrum bars (live layer).
#[must_use]
pub fn spectrum_shape(spectrum: &[f32; 32], width: f32, height: f32, t: TineTheme) -> Shape {
    let mut shape = Shape::new();
    let step = width / 32.0;
    let bar_w = (step * 0.55).max(2.0);
    for (i, &mag) in spectrum.iter().enumerate() {
        let x = i as f32 * step + (step - bar_w) / 2.0;
        let bar_h = (mag.clamp(0.0, 1.0) * (height * 0.85)).max(2.0);
        let top = height - bar_h;
        shape = shape.fill(
            vec![[x, height], [x + bar_w, height], [x + bar_w, top], [x, top]],
            t.spectrum,
        );
    }
    shape
}

fn db_labels(plot_h: f32, t: TineTheme) -> El {
    let mut col_el = col().key("canvas/db-labels").w(34.0).h(plot_h).absolute(6.0, 0.0);
    for (label, db) in [("+12", 12.0), ("0 dB", 0.0), ("-12", -12.0)] {
        let offset = if db == 0.0 { 15.0 } else { 7.0 };
        let y = ((0.5 - db / (2.0 * DB_MAX)) * plot_h - offset).clamp(6.0, (plot_h - 28.0).max(6.0));
        col_el = col_el.child(
            row()
                .key(format!("canvas/db-{label}"))
                .h(14.0)
                .absolute(0.0, y)
                .child(text(label).font(typography::CAPTION).color(t.text_dim)),
        );
    }
    col_el
}

// The frequency labels are drawn inside the plot at its bottom edge.
const FREQ_LABELS: [(&str, f32); 3] = [("100", 100.0), ("1k", 1000.0), ("10k", 10_000.0)];

fn freq_labels(plot_w: f32, plot_h: f32, t: TineTheme) -> El {
    let mut strip = row()
        .key("canvas/freq-labels")
        .role("label")
        .label("Frequency grid 100 Hz · 1 kHz · 10 kHz")
        .w(plot_w)
        .h(16.0)
        .absolute(0.0, (plot_h - 16.0).max(0.0));
    for (name, hz) in FREQ_LABELS {
        let x = freq_x(hz) * plot_w;
        strip = strip.child(
            row()
                .key(format!("canvas/freq-label-{name}"))
                .w(32.0)
                .h(14.0)
                .center()
                .absolute((x - 16.0).max(2.0).min((plot_w - 34.0).max(2.0)), 0.0)
                .child(text(name).font(typography::CAPTION).color(t.text_dim)),
        );
    }
    strip
}

/// A compact "IN -21 dBFS" level readout chip.
fn readout(key: &'static str, name: &'static str, db: f32, t: TineTheme) -> El {
    row()
        .key(key)
        .h(20.0)
        .px(6.0)
        .gap(4.0)
        .center()
        .radius(radii::PILL)
        .fill(t.well)
        .border(1.0, t.border)
        .child(text(name).font(typography::EYEBROW).color(t.text_dim))
        .child(text(format!("{db:.0}")).font(typography::CAPTION).color(t.text))
}

/// Linear blend of two colors (`f` = 0 → `a`, 1 → `b`).
fn mix(a: Color, b: Color, f: f32) -> Color {
    let f = f.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * f,
        g: a.g + (b.g - a.g) * f,
        b: a.b + (b.b - a.b) * f,
        a: a.a + (b.a - a.a) * f,
    }
}

fn macro_summary(m: Macro) -> &'static str {
    match m {
        Macro::Purity => "Cuts background noise & silences room hum",
        Macro::Body => "Adds vocal warmth & reduces boxy mud",
        Macro::Presence => "Softens harsh highs or boosts vocal clarity & de-essing",
        Macro::Leveler => "Evens out quiet & loud speech",
    }
}

fn macro_coach_line(m: Macro) -> &'static str {
    match m {
        Macro::Purity => "Cuts background noise & room hum. Drag up/right to clean; double-click to reset.",
        Macro::Body => "Balances chest warmth (110 Hz) & mud cut (320 Hz). Drag up/right for fuller voice.",
        Macro::Presence => "Softens harsh highs (<50%) or boosts clarity & S-taming (>50%). Double-click to reset.",
        Macro::Leveler => "Controls compression ratio & threshold to keep voice volume steady. Double-click to reset.",
    }
}

/// Live beginner coach / hint bar at the bottom of the mixer deck.
fn coach_bar(s: &AppState, t: TineTheme) -> El {
    let (badge, msg, active_highlight) = if let Some(m) = s.ui.hovered_macro {
        (m.label(), macro_coach_line(m).to_owned(), true)
    } else if let Some(id) = s.ui.hovered_param {
        let suffix = if s.model.pins.is_pinned(id) {
            "Custom pinned · double-click to relink to main knob"
        } else {
            "Drag up/right to adjust · double-click to reset"
        };
        (
            id.spec().label,
            format!("{} ({suffix})", crate::views::pro_rack::param_hint(id)),
            true,
        )
    } else {
        (
            "QUICK GUIDE",
            String::from(
                "Drag the 4 big dials to shape your voice, try Voice FX presets, or click Auto-Tune (8s). Hover any knob to see what it does.",
            ),
            false,
        )
    };

    row()
        .key("canvas/coach-bar")
        .role("status")
        .label(format!("{badge}: {msg}"))
        .w_full()
        .h(26.0)
        .px(spacing::SM)
        .gap(spacing::SM)
        .center()
        .radius(radii::SM)
        .fill(t.surface_raised)
        .border(1.0, t.border)
        .child(
            row()
                .key("canvas/coach-badge")
                .h(18.0)
                .px(6.0)
                .center()
                .radius(radii::SM)
                .fill(if active_highlight {
                    t.deck_blue_soft
                } else {
                    t.accent_soft
                })
                .child(text(badge).font(typography::EYEBROW).color(if active_highlight {
                    t.deck_blue_text
                } else {
                    t.accent_text
                })),
        )
        .child(
            row().key("canvas/coach-text-wrap").grow(1.0).child(
                text(msg)
                    .key("canvas/coach-text")
                    .font(typography::CAPTION)
                    .color(t.text),
            ),
        )
}

/// Spring-animated tooltip card shown while a macro knob is hovered or dragged.
/// Anchored above the macro knob (`Side::Top`) so the live sub-knobs below stay unobstructed.
fn tooltip_for(m: Macro, model: &UiModel, t: TineTheme) -> El {
    let value = m.get(model.macros);
    let rows = macro_explanation_rows(m, model);
    let title = format!("{} {value:.2}", m.label());

    let mut card = col()
        .key("canvas/tooltip")
        .role("tooltip")
        .label(format!("{title}; {}", rows.join("; ")))
        .px(spacing::MD)
        .py(spacing::SM)
        .gap(2.0)
        .radius(radii::MD)
        .fill(t.surface_raised)
        .border(1.0, t.border_strong)
        .shadow(8.0, 24.0, kit::oklcha_black(0.35))
        .paint_order(20)
        .child(text(title.clone()).font(typography::LABEL).color(t.accent_text))
        .child(text(macro_summary(m)).font(typography::CAPTION).color(t.text))
        .transition(Transition::offset(0.0, 6.0).combined(Transition::opacity()));

    for r in rows {
        card = card.child(text(r).font(typography::CAPTION).color(t.text_dim));
    }

    card.anchored(m.key(), Side::Top, Align::Center, 8.0)
}

/// In/Out meters with clip LEDs and the compressor GR mini-bar.
fn meters_panel(
    tel: &AudioTelemetryFrame,
    smooth_rms: (f32, f32),
    held: (f32, f32),
    compact: bool,
    t: TineTheme,
) -> El {
    const METER_W: f32 = 12.0;
    let meter_h: f32 = if compact { 82.0 } else { 112.0 };

    let led = |key: &'static str, clipped: bool| {
        el().key(key)
            .size(7.0, 7.0)
            .radius(3.5)
            .fill(if clipped { t.meter_red } else { t.well })
            .border(1.0, t.border)
    };

    let in_group = col()
        .key("canvas/meter-in-total")
        .gap(2.0)
        .center()
        .child(text("In").font(typography::CAPTION).color(t.text_dim))
        .child(led("canvas/clip-led-in", tel.input_peak_db >= CLIP_DBFS))
        .child(
            canvas(meter_shape(
                METER_W,
                meter_h,
                smooth_rms.0,
                tel.input_peak_db,
                held.0,
                t,
            ))
            .key("canvas/meter-in")
            .size(METER_W, meter_h),
        )
        .child(
            text(format!("{:.0}", held.0))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );

    let out_group = col()
        .key("canvas/meter-out-total")
        .gap(2.0)
        .center()
        .child(text("Out").font(typography::CAPTION).color(t.text_dim))
        .child(led("canvas/clip-led-out", tel.output_peak_db >= CLIP_DBFS))
        .child(
            canvas(meter_shape(
                METER_W,
                meter_h,
                smooth_rms.1,
                tel.output_peak_db,
                held.1,
                t,
            ))
            .key("canvas/meter-out")
            .size(METER_W, meter_h),
        )
        .child(
            text(format!("{:.0}", held.1))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );

    let gr = (-tel.comp_gain_reduction_db).clamp(0.0, 12.0) / 12.0;
    let gr_group = col()
        .key("canvas/gr-bar")
        .role("meter")
        .label(format!(
            "Compressor gain reduction {:.1} dB",
            tel.comp_gain_reduction_db
        ))
        .gap(2.0)
        .w_full()
        .center()
        .child(text("GR").font(typography::CAPTION).color(t.text_dim))
        .child(canvas(gr_shape(52.0, gr, t)).key("canvas/gr-canvas").size(52.0, 5.0));

    col()
        .key("canvas/meters")
        .role("group")
        .label("Input and output meters")
        .w(64.0)
        .center()
        .gap(4.0)
        .child(
            row()
                .key("canvas/meters-pair")
                .gap(6.0)
                .child(in_group)
                .child(out_group),
        )
        .child(gr_group)
}

/// Vertical meter: track, RMS fill from the bottom, peak tick and 1.5 s peak-hold tick.
fn meter_shape(w: f32, h: f32, rms_db: f32, peak_db: f32, held_db: f32, t: TineTheme) -> Shape {
    let fill_h = db_norm(rms_db) * h;
    let y_of = |db: f32| h - db_norm(db) * h;
    Shape::new()
        // Track.
        .fill(vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]], t.well)
        // RMS fill.
        .fill(
            vec![[0.0, h], [w, h], [w, h - fill_h], [0.0, h - fill_h]],
            meter_color(rms_db, t),
        )
        // 1.5 s peak hold tick.
        .line([0.0, y_of(held_db)], [w, y_of(held_db)], 2.0, LineCap::Butt, t.text)
        // Instantaneous peak tick.
        .line(
            [0.0, y_of(peak_db)],
            [w, y_of(peak_db)],
            2.0,
            LineCap::Butt,
            meter_color(peak_db, t),
        )
}

/// Horizontal compressor gain-reduction mini-bar.
fn gr_shape(w: f32, amount: f32, t: TineTheme) -> Shape {
    let fw = (amount.clamp(0.0, 1.0) * w).max(1.0);
    Shape::new()
        .fill(vec![[0.0, 0.0], [w, 0.0], [w, 5.0], [0.0, 5.0]], t.well)
        .fill(vec![[0.0, 0.0], [fw, 0.0], [fw, 5.0], [0.0, 5.0]], t.accent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tine_protocol::params::Band;

    #[test]
    fn freq_axis_places_macro_zones() {
        assert!((freq_x(150.0) - 0.114).abs() < 0.01, "Purity ~150 Hz zone");
        assert!((freq_x(250.0) - 0.206).abs() < 0.01, "Body ~250 Hz zone");
        assert!((freq_x(4000.0) - 0.709).abs() < 0.01, "Presence ~4 kHz zone");
        assert_eq!(freq_x(F_MIN_HZ), 0.0);
        assert_eq!(freq_x(F_MAX_HZ), 1.0);
    }

    #[test]
    fn leveler_stays_at_right_edge() {
        assert_eq!(macro_x(Macro::Leveler, 640.0), 640.0 - LEVELER_INSET);
        assert!(macro_x(Macro::Leveler, 300.0) > macro_x(Macro::Presence, 300.0) + 40.0);
    }

    #[test]
    fn curve_hash_tracks_curve_relevant_params() {
        let base = DspParams::default();
        assert_eq!(curve_params_hash(&base), curve_params_hash(&base.clone()));

        let mut alt = base;
        alt.eq_gain_db[0] = 2.5;
        assert_ne!(curve_params_hash(&base), curve_params_hash(&alt));

        // A param the curve ignores does not invalidate it.
        let mut unrelated = base;
        unrelated.gate_release_ms = 42.0;
        assert_eq!(curve_params_hash(&base), curve_params_hash(&unrelated));
    }

    #[test]
    fn curve_db_memoizes_on_params_hash() {
        let p = DspParams::default();
        let (h0, e0) = curve_cache_stats();
        let first = curve_db(&p);
        let second = curve_db(&p);
        let (h1, e1) = curve_cache_stats();
        assert_eq!(first, second);
        assert!(h1 > h0, "second read must be a cache hit");
        assert_eq!(e1 - e0, 1, "exactly one DSP evaluation");

        let mut other = p;
        other.eq_gain_db[3] = -3.0;
        let third = curve_db(&other);
        let (_, e2) = curve_cache_stats();
        assert_ne!(first, third, "changed EQ gain must change the curve");
        assert_eq!(e2 - e1, 1, "one more evaluation for changed params");
    }

    #[test]
    fn peak_hold_holds_then_falls() {
        let mut ph = PeakHold::new();
        assert_eq!(ph.observe(-30.0, 100), -30.0);
        // A short excursion is held for HOLD_FRAMES frames.
        assert_eq!(ph.observe(-10.0, 101), -10.0);
        assert_eq!(ph.observe(-40.0, 130), -10.0, "still inside the 1.5 s window");
        assert_eq!(ph.observe(-40.0, 102 + HOLD_FRAMES), -40.0, "hold expired");
        // A new peak refreshes the window.
        assert_eq!(ph.observe(-5.0, 200 + HOLD_FRAMES), -5.0);
        assert_eq!(ph.observe(-20.0, 205 + HOLD_FRAMES), -5.0);
    }

    #[test]
    fn explanation_rows_come_from_macro_mapper_and_show_pins() {
        let model = crate::scenes::make_golden_line().model;

        // MacroMapper drive sets: Purity 2, Body 2, Presence 4, Leveler 3 rows.
        assert_eq!(macro_explanation_rows(Macro::Purity, &model).len(), 2);
        assert_eq!(macro_explanation_rows(Macro::Body, &model).len(), 2);
        assert_eq!(macro_explanation_rows(Macro::Presence, &model).len(), 4);
        assert_eq!(macro_explanation_rows(Macro::Leveler, &model).len(), 3);

        // EQ-gain rows read like the spec example: "+x.y dB @ 110 Hz".
        let rows = macro_explanation_rows(Macro::Body, &model);
        assert!(
            rows.iter().all(|r| r.contains("dB @") || r.contains("pinned (Pro)")),
            "EQ gain rows show their band frequency: {rows:?}"
        );
        // Deterministic for identical inputs.
        assert_eq!(macro_explanation_rows(Macro::Body, &model), rows);

        // A pinned driven parameter shows the D-007 tag on its row.
        let mut pinned = model.clone();
        pinned.pins.pin(ParamId::EqGainDb(Band::B1));
        let rows = macro_explanation_rows(Macro::Body, &pinned);
        assert!(
            rows.iter().any(|r| r.contains("pinned (Pro)")),
            "pinned row must be tagged: {rows:?}"
        );
        assert_eq!(rows.len(), 2, "pinning replaces, never adds rows");
    }

    #[test]
    fn spectrum_shape_draws_all_bars() {
        let t = TineTheme::dark();
        let s = spectrum_shape(&[0.5; 32], 640.0, 300.0, t);
        assert!(!s.is_empty());
    }

    #[test]
    fn knob_drag_supports_horizontal_and_vertical_motion() {
        // Right (+dx) and Up (-dy) both increase clockwise.
        assert_eq!(knob_drag_px(50.0, 0.0), 50.0);
        assert_eq!(knob_drag_px(-50.0, 0.0), -50.0);
        assert_eq!(knob_drag_px(0.0, -50.0), 50.0);
        assert_eq!(knob_drag_px(0.0, 50.0), -50.0);
        // Opposing slight vertical drift during horizontal swipe keeps horizontal dominant.
        assert_eq!(knob_drag_px(60.0, 12.0), 60.0);
        assert_eq!(knob_drag_px(-60.0, -12.0), -60.0);
        // Crossing the 45-degree diagonal never causes a giant sign-flip teleport.
        let before_diag = knob_drag_px(60.0, 59.0);
        let at_diag = knob_drag_px(60.0, 60.0);
        let after_diag = knob_drag_px(60.0, 61.0);
        assert!((before_diag - at_diag).abs() <= 2.0);
        assert!((at_diag - after_diag).abs() <= 2.0);
    }

    #[test]
    fn curve_db_flattens_when_bypassed() {
        let mut p = DspParams::default();
        p.eq_gain_db[Band::B0.index()] = 6.0;
        assert!(curve_db(&p).iter().any(|&d| d.abs() > 1.0));
        p.bypass = true;
        assert!(curve_db(&p).iter().all(|&d| d == 0.0));
    }
}
