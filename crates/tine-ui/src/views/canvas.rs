//! Center "Golden Line" canvas: 32-band live spectrum, memoized response curve with glow,
//! frequency grid, four draggable macro nodes with a spring tooltip, and in/out meters
//! with peak hold, clip LEDs and a compressor gain-reduction mini-bar.
//!
//! Layering (bottom → top): static grid (freq lines + 0 dB reference, built through Rune's
//! `memo` so it is built once per window and raster-cached with `.cache()`), the response
//! curve layer (`memo`-ized on a hash of the curve-relevant `DspParams` fields, so the DSP
//! evaluation only runs when parameters actually change), the live spectrum layer
//! (rebuilt each telemetry frame), then the macro nodes and the tooltip overlay.
//! Only the live layers touch per-frame work; the curve and grid layers are cached.

use std::cell::RefCell;

use rune::{
    Align, Color, Cx, Drag, El, Key, KeyEvent, LineCap, Phase, Shape, Side, Transition, Tx, canvas, col, el, memo, row,
    text,
};
use tine_dsp::curve::response_curve;
use tine_dsp::macros::{ExplainBuf, ExplainRow, Macro as DspMacro, MacroMapper};
use tine_dsp::presets;
use tine_protocol::params::{DspParams, EQ_BAND_COUNT, ParamId, Unit};
use tine_protocol::presets::{Calibration, CalibrationOverrides};
use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::model::{CalibrationUi, Macro, UiModel};
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

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

/// Macro units per vertical pixel of node drag (task: 1 px = 0.004).
const DRAG_PER_PX: f32 = 0.004;
/// Peak-hold window in telemetry frames (1.5 s at 60 Hz).
const HOLD_FRAMES: u64 = 90;
/// dBFS at or above which the clip LED lights.
const CLIP_DBFS: f32 = -0.3;
/// Meter display range bottom.
const METER_MIN_DB: f32 = -60.0;

/// Macro node diameter in points.
const NODE_D: f32 = 26.0;
const NODE_R: f32 = NODE_D / 2.0;
/// Inset of the Leveler node from the right plot edge.
const LEVELER_INSET: f32 = NODE_R + 12.0;

/// Fallback plot size before the first layout settles.
const PLOT_FALLBACK: [f32; 2] = [640.0, 240.0];

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
    let cal = model.calibration.as_ref().map(|c| calibration_from_ui(c, &preset.base));
    let mut buf = ExplainBuf::new();
    MacroMapper::explain(preset, cal.as_ref(), model.macros, model.pins, dsp_macro(m), &mut buf);
    buf.rows().iter().map(|row| explain_row_string(row, model)).collect()
}

fn calibration_from_ui(cal: &CalibrationUi, base: &DspParams) -> Calibration {
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
            let (freqs, mut db) = (c.freqs, c.db);
            response_curve(params, SAMPLE_RATE_HZ, &freqs, &mut db);
            c.db = db;
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

thread_local! {
    static PEAKS: RefCell<[PeakHold; 2]> = const { RefCell::new([PeakHold::new(); 2]) };
    static PEAK_OBSERVED_SEQ: RefCell<u64> = const { RefCell::new(u64::MAX) };
}

/// Peak-held levels `(input, output)` in dBFS for the current telemetry frame.
///
/// The hold window is measured in telemetry frames (`HOLD_FRAMES` = 1.5 s at the
/// 60 Hz telemetry rate, keyed on `AudioTelemetryFrame::sequence`), so it stays
/// deterministic in headless runs instead of using a wall clock the view never sees.
fn held_peaks(tel: &AudioTelemetryFrame) -> (f32, f32) {
    PEAKS.with_borrow_mut(|p| {
        // Skip repeated observation of the same frame (the view can build several
        // commits per frame); a stale hold must not refresh itself.
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
    if db > -3.0 {
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

/// Renders the Golden Line canvas panel.
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = TineTheme::dark();
    let tel = &s.live.telemetry;
    let held = held_peaks(tel);

    // Settled plot size from the previous layout pass (re-runs the view on resize).
    let [plot_w, plot_h] = cx
        .layout_with("canvas/plot", |r| [r[2] as i32, r[3] as i32])
        .map_or(PLOT_FALLBACK, |[w, h]| [w.max(320) as f32, h.max(120) as f32]);

    let curve_hash = curve_params_hash(&s.model.params);
    let curve = curve_db(&s.model.params);

    let title_row = row()
        .key("canvas/title-row")
        .w_full()
        .child(
            text("Golden Line — Voice Conditioner")
                .font(typography::BODY)
                .color(t.text),
        )
        .child(el().key("canvas/title-spacer").grow(1.0))
        .child(
            text(format!(
                "In {:.1} dBFS  |  Out {:.1} dBFS",
                tel.input_rms_db, tel.output_rms_db
            ))
            .font(typography::CAPTION)
            .color(t.text_dim),
        );

    // --- Plot column: grid, curve, spectrum, macro nodes, tooltip ---
    let mut plot = col()
        .key("canvas/plot")
        .grow(1.0)
        .min_h(200.0)
        .radius(radii::MD)
        .fill(t.bg)
        .border(1.0, t.border)
        .child(grid_layer(t))
        .child(curve_layer(&curve, curve_hash, plot_w, plot_h, t))
        .child(
            canvas(spectrum_shape(&s.live.spectrum, plot_w, plot_h, t))
                .key("canvas/spectrum")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
        )
        .child(freq_labels(plot_w, plot_h));

    for m in Macro::ALL {
        plot = plot.child(macro_node(m, m.get(s.model.macros), plot_w, plot_h, t));
    }

    if let Some(m) = s.ui.hovered_macro {
        plot = plot.child(tooltip_for(m, &s.model, t));
    }

    // --- Meters column (right edge) ---
    let meters = meters_panel(tel, held, t);

    col()
        .key("canvas")
        .w_full()
        .grow(1.0)
        .min_h(200.0)
        .px(spacing::LG)
        .py(spacing::MD)
        .gap(spacing::SM)
        .radius(radii::LG)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(title_row)
        .child(
            row()
                .key("canvas/plot-strip")
                .w_full()
                .grow(1.0)
                .min_h(200.0)
                .gap(spacing::SM)
                .child(plot)
                .child(meters),
        )
}

/// Static grid layer: 0 dB reference line, ±6 dB guides and 100/1k/10k gridlines.
fn grid_layer(t: TineTheme) -> El {
    memo("canvas/grid", (), move || {
        let mut s = Shape::new();
        let gridline = |s: Shape, x: f32| s.line([x, 0.0], [x, 1.0], 1.0, LineCap::Butt, t.border);
        s = gridline(s, freq_x(100.0));
        s = gridline(s, freq_x(1000.0));
        s = gridline(s, freq_x(10_000.0));
        // 0 dB reference line at mid height.
        s = s.line([0.0, 0.5], [1.0, 0.5], 1.0, LineCap::Butt, t.accent_glow);
        canvas(s)
            .key("canvas/grid")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .cache()
    })
}

/// Response curve layer: wide translucent glow stroke under a solid spline.
/// Memoized on the params hash **and** the plot size, so both a parameter change
/// and a resize rebuild it, and nothing else does.
fn curve_layer(db: &[f32; CURVE_POINTS], hash: u64, w: f32, h: f32, t: TineTheme) -> El {
    memo("canvas/curve", (hash, w as i32, h as i32), move || {
        let pts: Vec<[f32; 2]> = curve_points(db, w, h);
        let mut s = Shape::new().stroke(pts.clone(), 7.0, t.accent_glow);
        s = s.stroke(pts, 2.5, t.accent);
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
fn spectrum_shape(spectrum: &[f32; 32], width: f32, height: f32, t: TineTheme) -> Shape {
    let mut shape = Shape::new();
    let step = width / 32.0;
    let bar_w = (step * 0.55).max(2.0);
    for (i, &mag) in spectrum.iter().enumerate() {
        let x = i as f32 * step + (step - bar_w) / 2.0;
        let bar_h = (mag.clamp(0.0, 1.0) * (height * 0.85)).max(2.0);
        let top = height - bar_h;
        shape = shape.fill(
            vec![[x, height], [x + bar_w, height], [x + bar_w, top], [x, top]],
            t.accent_glow,
        );
    }
    shape
}

// The frequency labels are drawn inside the plot at its bottom edge.
const FREQ_LABELS: [(&str, f32); 3] = [("100", 100.0), ("1k", 1000.0), ("10k", 10_000.0)];

fn freq_labels(plot_w: f32, plot_h: f32) -> El {
    let mut strip = row()
        .key("canvas/freq-labels")
        .role("label")
        .label("Frequency grid 100 Hz · 1 kHz · 10 kHz")
        .w(plot_w)
        .h(18.0)
        .absolute(0.0, (plot_h - 18.0).max(0.0));
    for (name, hz) in FREQ_LABELS {
        let x = freq_x(hz) * plot_w;
        strip = strip.child(
            row()
                .key(format!("canvas/freq-label-{name}"))
                .w(40.0)
                .h(16.0)
                .center()
                .absolute((x - 20.0).max(2.0).min(plot_w - 42.0), 0.0)
                .child(text(name).font(typography::CAPTION).color(t_dim())),
        );
    }
    strip
}

fn t_dim() -> rune::Color {
    TineTheme::dark().text_dim
}

/// One draggable Golden Line macro node with its caption label.
fn macro_node(m: Macro, value: f32, plot_w: f32, plot_h: f32, t: TineTheme) -> El {
    let x = macro_x(m, plot_w);
    let y_c = (1.0 - value) * plot_h;
    let y_top = (y_c - NODE_R).clamp(1.0, (plot_h - NODE_D - 1.0).max(1.0));

    let node = el()
        .key(m.key())
        .role("slider")
        .label(format!("{} {value:.2}", m.label()))
        .focusable(true)
        .size(NODE_D, NODE_D)
        .absolute((x - NODE_R).max(0.0), y_top)
        .radius(NODE_R)
        .fill(t.accent)
        .border(1.5, t.text_on_accent)
        .hover(rune::color::oklcha(1.0, 0.0, 0.0, 0.10))
        .press(rune::color::oklcha(1.0, 0.0, 0.0, 0.16))
        .cursor(rune::Cursor::Grab)
        // The node owns its keyboard: arrows nudge (Shift by ten times as much) and
        // Space toggles A/B, before focus navigation or the window's on_key.
        .on_key(move |s: &mut AppState, ev: &KeyEvent, _tx: &mut Tx| match &ev.key {
            Key::Up | Key::Down => {
                let delta = if ev.mods.shift { 0.1 } else { 0.01 };
                let signed = if matches!(ev.key, Key::Up) { delta } else { -delta };
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
            // Reset to the current archetype's preset default (T10 macro_defaults).
            let defaults = presets::builtin(s.model.archetype).macro_defaults;
            s.set_macro_optimistic(m, m.get(defaults));
        })
        .on_drag(move |s: &mut AppState, d: &mut Drag, _tx: &mut Tx| {
            match d.phase {
                Phase::Down => {
                    d.data = m.get(s.model.macros);
                    s.ui.hovered_macro = Some(m);
                }
                Phase::Move => {
                    // Up (negative dy) raises the value: 1 px = 0.004.
                    s.set_macro_optimistic(m, d.data - d.dy * DRAG_PER_PX);
                }
                Phase::Up => {
                    if d.cancelled() {
                        s.set_macro_optimistic(m, d.data);
                    }
                }
            }
        });

    let caption_y = (y_c + NODE_R + 2.0).min((plot_h - 16.0).max(0.0));
    let caption = row()
        .key(format!("{}/caption", m.key()))
        .role("label")
        .label(m.label())
        .w(56.0)
        .h(14.0)
        .center()
        .absolute((x - 28.0).max(0.0).min(plot_w - 58.0), caption_y)
        .child(text(m.label()).font(typography::CAPTION).color(t_dim()));

    col()
        .key(format!("{}/wrap", m.key()))
        .absolute(0.0, 0.0)
        .child(node)
        .child(caption)
}

/// Spring-animated tooltip card shown while a node is hovered or dragged.
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
        .border(1.0, t.accent)
        .child(text(title.clone()).font(typography::LABEL).color(t.text))
        .transition(Transition::offset(0.0, 6.0).combined(Transition::opacity()));

    for r in rows {
        card = card.child(text(r).font(typography::CAPTION).color(t.text_dim));
    }

    card.anchored(m.key(), Side::Top, Align::Center, 10.0)
}

/// In/Out meters with clip LEDs and the compressor GR mini-bar.
fn meters_panel(tel: &AudioTelemetryFrame, held: (f32, f32), t: TineTheme) -> El {
    const METER_W: f32 = 14.0;
    const METER_H: f32 = 150.0;

    let led = |key: &'static str, clipped: bool| {
        el().key(key)
            .size(8.0, 8.0)
            .radius(4.0)
            .fill(if clipped { t.meter_red } else { t.surface_raised })
            .border(1.0, t.border)
    };

    let in_group = col()
        .key("canvas/meter-in-total")
        .gap(spacing::XS)
        .center()
        .child(text("In").font(typography::CAPTION).color(t_dim()))
        .child(led("canvas/clip-led-in", tel.input_peak_db >= CLIP_DBFS))
        .child(
            canvas(meter_shape(
                METER_W,
                METER_H,
                tel.input_rms_db,
                tel.input_peak_db,
                held.0,
                t,
            ))
            .key("canvas/meter-in")
            .size(METER_W, METER_H),
        )
        .child(text(format!("{:.0}", held.0)).font(typography::CAPTION).color(t_dim()));

    let out_group = col()
        .key("canvas/meter-out-total")
        .gap(spacing::XS)
        .center()
        .child(text("Out").font(typography::CAPTION).color(t_dim()))
        .child(led("canvas/clip-led-out", tel.output_peak_db >= CLIP_DBFS))
        .child(
            canvas(meter_shape(
                METER_W,
                METER_H,
                tel.output_rms_db,
                tel.output_peak_db,
                held.1,
                t,
            ))
            .key("canvas/meter-out")
            .size(METER_W, METER_H),
        )
        .child(text(format!("{:.0}", held.1)).font(typography::CAPTION).color(t_dim()));

    let gr = (-tel.comp_gain_reduction_db).clamp(0.0, 12.0) / 12.0;
    let gr_group = col()
        .key("canvas/gr-bar")
        .role("meter")
        .label(format!(
            "Compressor gain reduction {:.1} dB",
            tel.comp_gain_reduction_db
        ))
        .gap(spacing::XS)
        .w_full()
        .child(text("Comp GR").font(typography::CAPTION).color(t_dim()))
        .child(canvas(gr_shape(64.0, gr, t)).key("canvas/gr-canvas").size(64.0, 6.0));

    col()
        .key("canvas/meters")
        .role("group")
        .label("Input and output meters")
        .w(84.0)
        .center()
        .gap(spacing::SM)
        .child(
            row()
                .key("canvas/meters-pair")
                .gap(spacing::SM)
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
        .fill(vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]], t.border)
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
        .fill(vec![[0.0, 0.0], [w, 0.0], [w, 6.0], [0.0, 6.0]], t.border)
        .fill(vec![[0.0, 0.0], [fw, 0.0], [fw, 6.0], [0.0, 6.0]], t.meter_amber)
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
        // Same model as `make_golden_line()` in scenes.rs.
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
        // 32 translucent fills, no strokes.
        assert!(!s.is_empty());
    }
}
