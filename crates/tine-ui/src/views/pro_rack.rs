//! Bottom collapsible "Surgical Pro Rack" drawer: one section per pipeline stage 1–6,
//! with every control generated from the `tine-protocol` `SPECS` table — log-taper
//! sliders, numeric fields with unit suffixes and inline validation, stage enable
//! switches, EQ band-type selects, pin indicators (D-007) — plus live gain-reduction
//! meters for the gate, de-esser and compressor, and an output meter with the limiter
//! ceiling marker.
//!
//! Drawer content scrolls vertically in a bounded viewport (`.scroll()`), so at
//! 960×640 nothing ever clips outside it. The collapsed drawer is byte-identical to
//! the T19 scaffold (same elements, keys and text), so the `golden_line` snapshots
//! don't move.

use rune::{
    Cx, Drag, El, Key, Phase, PopupMenu, Spring, SpringPair, Transition, Tx, canvas, col, el, popup_menu_supported,
    row, text,
};
use tine_protocol::params::{Band, FilterType, PARAM_COUNT, ParamId, ParamSpec, Scale, Unit};
use tine_protocol::presets::PinSet;

use crate::intent::Intent;
use crate::state::{AppState, ProFieldEdit};
use crate::theme::{TineTheme, radii, spacing, typography};
use crate::views::kit::{self, Tone, glyph};

/// One pipeline stage section of the rack.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    /// Stage number `1..=6`, shown in the section title.
    pub number: u8,
    /// Section title (without the leading number).
    pub title: &'static str,
    /// The stage's boolean "enable" parameter rendered as a switch in the section
    /// header (`None` for stages without one).
    pub enable: Option<ParamId>,
    /// Every primary parameter of the section, in focus order. EQ band
    /// freq/Q/type sub-rows are appended by [`section_params`].
    pub params: &'static [ParamId],
}

impl Stage {
    /// Human id used in element keys (e.g. `"pro/sec/2"`).
    #[must_use]
    pub const fn id(self) -> u8 {
        self.number
    }
}

/// Rack sections, stage 1–6 (stage 7's metering tap lives in the canvas).
///
/// Generated from `SPECS`: every parameter except the global `Bypass` (which lives
/// in the header's A/B switch) belongs to exactly one section.
pub const STAGES: [Stage; 6] = [
    Stage {
        number: 1,
        title: "Input & High-Pass",
        enable: Some(ParamId::HpfEnabled),
        params: &[ParamId::InputGainDb, ParamId::HpfFreqHz],
    },
    Stage {
        number: 2,
        title: "AI Denoise & Gate",
        enable: Some(ParamId::GateEnabled),
        params: &[
            ParamId::DenoiseMix,
            ParamId::GateThresholdDb,
            ParamId::GateRatio,
            ParamId::GateHysteresisDb,
            ParamId::GateAttackMs,
            ParamId::GateReleaseMs,
        ],
    },
    Stage {
        number: 3,
        title: "5-Band Parametric EQ",
        enable: None,
        params: &[
            ParamId::EqGainDb(Band::B0),
            ParamId::EqGainDb(Band::B1),
            ParamId::EqGainDb(Band::B2),
            ParamId::EqGainDb(Band::B3),
            ParamId::EqGainDb(Band::B4),
        ],
    },
    Stage {
        number: 4,
        title: "De-Esser",
        enable: Some(ParamId::DeesserEnabled),
        params: &[
            ParamId::DeesserFreqHz,
            ParamId::DeesserThreshDb,
            ParamId::DeesserAmountDb,
        ],
    },
    Stage {
        number: 5,
        title: "Compressor",
        enable: Some(ParamId::CompEnabled),
        params: &[
            ParamId::CompThreshDb,
            ParamId::CompRatio,
            ParamId::CompAttackMs,
            ParamId::CompReleaseMs,
            ParamId::CompKneeDb,
            ParamId::CompMakeupDb,
            ParamId::CompAutoMakeup,
        ],
    },
    Stage {
        number: 6,
        title: "Limiter",
        enable: None,
        params: &[ParamId::LimiterCeilingDb, ParamId::LimiterTruePeak],
    },
];

/// The five EQ bands, each of which gets a type select and freq/Q slider rows
/// after the section's gain rows.
const EQ_BANDS: [Band; 5] = [Band::B0, Band::B1, Band::B2, Band::B3, Band::B4];

/// The seven filter types an EQ band can take (popup menu items).
const FILTER_OPTIONS: [(FilterType, &str); 7] = [
    (FilterType::LowShelf, "Low-Shelf"),
    (FilterType::HighShelf, "High-Shelf"),
    (FilterType::Peaking, "Peaking"),
    (FilterType::Notch, "Notch"),
    (FilterType::LowPass, "Low-Pass"),
    (FilterType::HighPass, "High-Pass"),
    (FilterType::BandPass, "Band-Pass"),
];

/// The section a [`ParamId`] belongs to (`None` for `Bypass`, the header's A/B).
#[must_use]
pub fn stage_of(id: ParamId) -> Option<u8> {
    match id {
        ParamId::Bypass => None,
        // EQ band freq/Q/type sub-rows belong to stage 3.
        ParamId::EqType(_) | ParamId::EqFreqHz(_) | ParamId::EqQ(_) => Some(3),
        _ => STAGES
            .iter()
            .find(|st| st.enable == Some(id) || st.params.contains(&id))
            .map(|st| st.number),
    }
}

/// Every [`ParamId`] drawn in a section, in focus order (header enable + body rows).
///
/// Returns the pair `(section params, enable switch)` callers need to rebuild the
/// exact focus order: the enable switch comes first, then the body rows.
#[must_use]
pub fn section_params(stage: Stage) -> Vec<ParamId> {
    let mut ids: Vec<ParamId> = Vec::new();
    if let Some(e) = stage.enable {
        ids.push(e);
    }
    ids.extend_from_slice(stage.params);
    if stage.number == 3 {
        for b in EQ_BANDS {
            ids.push(ParamId::EqType(b));
            ids.push(ParamId::EqFreqHz(b));
            ids.push(ParamId::EqQ(b));
        }
    }
    ids
}

/// The section of a parameter as a [`Stage`], for tests and callers that want the entry.
#[must_use]
pub fn stage_entry(id: ParamId) -> Option<Stage> {
    let n = stage_of(id)?;
    STAGES.iter().copied().find(|st| st.number == n)
}

/// Stable element key of the interactive control for `id` (slider/switch/select).
#[must_use]
pub fn control_key(id: ParamId) -> String {
    let key = id.spec().key;
    match id.spec().scale {
        Scale::Bool => format!("switch/{key}"),
        Scale::Enum => format!("select/{key}"),
        Scale::Linear | Scale::Log => format!("slider/{key}"),
    }
}

/// Stable element key of the unpin button for `id`.
#[must_use]
pub fn pin_key(id: ParamId) -> String {
    format!("pin/{}", id.spec().key)
}

/// All pinned parameters, enumerated in `SPECS` order.
#[must_use]
pub fn pinned_params(pins: &PinSet) -> Vec<ParamId> {
    let mut out = Vec::new();
    for i in 0..PARAM_COUNT {
        if let Some(id) = ParamId::from_index(i)
            && pins.is_pinned(id)
        {
            out.push(id);
        }
    }
    out
}

/// Plain numeric seed text for the inline field (no unit suffixes, so the user's
/// typed number replaces it round-trip safely; [`parse_numeric`] accepts units
/// anyway).
#[must_use]
pub fn numeric_seed(id: ParamId, v: f32) -> String {
    let spec = id.spec();
    if spec.step >= 1.0 {
        format!("{v:.0}")
    } else if spec.step >= 0.1 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

/// Edge-case: `ProFieldEdit::open` seeds the field with the parameter's current
/// value in plain numeric form.
#[must_use]
pub fn pro_field_seed(id: ParamId, model: &crate::model::UiModel) -> String {
    numeric_seed(id, model.params.get(id))
}

/// Formats a parameter value with its unit for readouts and labels.
#[must_use]
pub fn value_text(id: ParamId, v: f32) -> String {
    match id.spec().unit {
        Unit::Db => format!("{v:.1} dB"),
        Unit::Dbfs => format!("{v:.1} dBFS"),
        Unit::Hz => {
            if v >= 1000.0 {
                let k = v / 1000.0;
                if (k - k.round()).abs() < 5e-3 {
                    format!("{k:.0} kHz")
                } else {
                    format!("{k:.1} kHz")
                }
            } else {
                format!("{v:.0} Hz")
            }
        }
        Unit::Ms => format!("{v:.2} ms"),
        Unit::Ratio => format!("{v:.1}:1"),
        Unit::None => match id {
            ParamId::DenoiseMix => format!("{:.0} %", v * 100.0),
            ParamId::EqType(b) => filter_type_label(FilterType::from_f32(v), b),
            _ => format!("{v:.2}"),
        },
    }
}

/// Filter-type label for an EQ band select (e.g. `"EQ 1 Type: Low-Shelf"`).
#[must_use]
pub fn filter_type_label(ft: FilterType, band: Band) -> String {
    let name = FILTER_OPTIONS
        .iter()
        .find(|(t, _)| *t == ft)
        .map_or("Peaking", |(_, n)| *n);
    format!("{}: {name}", ParamId::EqType(band).spec().label)
}

/// Slider position `0..=1` for a value under the spec's taper.
fn to_pos(v: f32, spec: &ParamSpec) -> f32 {
    match spec.scale {
        Scale::Log => {
            let ratio = (spec.max / spec.min).ln();
            (((v.clamp(spec.min, spec.max) / spec.min).ln()) / ratio).clamp(0.0, 1.0)
        }
        _ => ((v - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0),
    }
}

/// Inverse of [`to_pos`].
fn from_pos(p: f32, spec: &ParamSpec) -> f32 {
    let p = p.clamp(0.0, 1.0);
    match spec.scale {
        Scale::Log => (spec.min * (spec.max / spec.min).powf(p)).clamp(spec.min, spec.max),
        _ => (spec.min + p * (spec.max - spec.min)).clamp(spec.min, spec.max),
    }
}

/// True when `v` is finite and inside the spec range.
fn in_range(v: f32, spec: &ParamSpec) -> bool {
    v.is_finite() && v >= spec.min && v <= spec.max
}

/// Parses user-typed numeric text: tolerates unit suffixes (`"dB"`, `"Hz"`, `":1"`,
/// `" %"`) and surrounding whitespace; `None` for anything unparseable.
#[must_use]
pub fn parse_numeric(input: &str) -> Option<f32> {
    let s = input.trim();
    // Strip a trailing unit suffix: keep the leading run that can belong to a number.
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    // A bare ratio like "4:1" is just the number before the colon.
    let num = s[..end].trim();
    if num.is_empty() {
        // Ratio style " 4 : 1" — also split at the colon.
        if let Some(colon) = s.find(':') {
            return parse_numeric(s[..colon].trim());
        }
        return None;
    }
    match num.parse::<f32>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

/// Outcome of committing the active numeric field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldCommit {
    /// Parsed and applied: `value` is what landed (clamped to the spec range when
    /// `clamped` is `true`).
    Applied {
        /// The value that landed, inside the spec range.
        value: f32,
        /// The raw text was outside the range and got clamped.
        clamped: bool,
    },
    /// The text wasn't a number; the inline message explains.
    Rejected {
        /// Inline validation message for the field.
        message: String,
    },
}

/// Pure commit for the numeric field: parse, validate, clamp. Separate from
/// [`commit_field`] so the clamp/inline-message contract is unit-testable.
#[must_use]
pub fn commit_value(id: ParamId, input: &str) -> FieldCommit {
    let spec = id.spec();
    match parse_numeric(input) {
        Some(v) if in_range(v, spec) => FieldCommit::Applied {
            value: v,
            clamped: false,
        },
        Some(v) => FieldCommit::Applied {
            value: v.clamp(spec.min, spec.max),
            clamped: true,
        },
        None => FieldCommit::Rejected {
            message: String::from("Enter a number"),
        },
    }
}

/// Plain-English beginner explanation for every parameter in `SPECS`, shown in
/// both the Pro Controls coach banner and the main console's hint bar.
#[must_use]
pub const fn param_hint(id: ParamId) -> &'static str {
    match id {
        ParamId::Bypass => "Switches all Tine voice processing on or off (A/B compare with raw mic).",
        ParamId::InputGainDb => "Input volume before processing. Aim for -18 dBFS speech on the meter.",
        ParamId::HpfEnabled => "Turns on the low-cut filter to block desk thumps, AC hum, and rumble.",
        ParamId::HpfFreqHz => "Cuts deep bass rumble below this frequency (80-110 Hz is ideal for voice).",
        ParamId::DenoiseMix => "Blends AI noise removal. 100% removes max noise; 50-80% keeps natural tone.",
        ParamId::GateEnabled => "Automatically quiets the mic during pauses so background noise vanishes.",
        ParamId::GateThresholdDb => "Volume below which the gate closes. Set just above your quiet room floor.",
        ParamId::GateRatio => "How strongly room noise gets turned down when you stop speaking.",
        ParamId::GateHysteresisDb => "Closing margin that stops the gate from chattering mid-sentence.",
        ParamId::GateAttackMs => "How fast the gate opens on speech. Keep fast (0.5-2 ms) to catch first words.",
        ParamId::GateReleaseMs => "How smoothly the gate fades closed after a pause so word tails stay intact.",
        ParamId::EqType(b) => match b.as_u8() {
            0 => "Filter shape for Band 1 (Low-Shelf boosts or trims deep bass warmth).",
            1 => "Filter shape for Band 2 (Peaking scoops out hollow room/desk mud).",
            2 => "Filter shape for Band 3 (Peaking fixes nasal or boxy midrange).",
            3 => "Filter shape for Band 4 (Peaking lifts crisp vocal clarity).",
            _ => "Filter shape for Band 5 (High-Shelf adds airy studio brightness).",
        },
        ParamId::EqFreqHz(b) => match b.as_u8() {
            0 => "Band 1 center frequency. Move Band 1 Gain off 0 dB to hear changes.",
            1 => "Band 2 target frequency (250-400 Hz targets hollow/muddy voice tone).",
            2 => "Band 3 target frequency (800-1200 Hz targets nasal/headset honk).",
            3 => "Band 4 target frequency (3-5 kHz lifts vocal articulation and edge).",
            _ => "Band 5 target frequency (7-12 kHz controls breathy studio sparkle).",
        },
        ParamId::EqGainDb(b) => match b.as_u8() {
            0 => "Boost (+) for chest warmth or cut (-) to trim boomy proximity bass.",
            1 => "Cut (-) to clear out muddy, boxy, or tubby low-mid frequencies.",
            2 => "Cut (-) to tame nasal headset honk, or boost (+) for mid presence.",
            3 => "Boost (+) so your voice cuts clearly through games, calls, and music.",
            _ => "Boost (+) for bright studio air, or cut (-) to soften harsh treble.",
        },
        ParamId::EqQ(b) => match b.as_u8() {
            0 => "Width/slope of Band 1. Lower Q is gentle; higher Q is tighter.",
            1 => "Width of Band 2 mud cut. Higher Q targets a narrow boxy resonance.",
            2 => "Width of Band 3 midrange curve. Higher Q targets a narrow nasal peak.",
            3 => "Width of Band 4 clarity curve (~1.0 gives a smooth vocal lift).",
            _ => "Slope of Band 5 air shelf (~0.7 gives a smooth natural tilt).",
        },
        ParamId::DeesserEnabled => "Tames sharp 'S', 'SH', and 'T' sounds without dulling your voice.",
        ParamId::DeesserFreqHz => "Sibilance frequency to listen for (typically 6-8 kHz for sharp 'S').",
        ParamId::DeesserThreshDb => "Brightness level where the de-esser catches harsh 'S' peaks.",
        ParamId::DeesserAmountDb => "Max reduction on harsh 'S' sounds (3-8 dB sounds smooth and natural).",
        ParamId::CompEnabled => "Evens out volume so whispers stay clear and loud laughs never clip.",
        ParamId::CompThreshDb => "Loudness where leveling begins. Lowering this smooths out more speech.",
        ParamId::CompRatio => "Compression firmness above Threshold (3:1 is natural; 5:1+ is radio-tight).",
        ParamId::CompAttackMs => "How fast leveling grabs loud transients (3-10 ms keeps natural punch).",
        ParamId::CompReleaseMs => "How smoothly volume recovers after loud words (100-200 ms feels natural).",
        ParamId::CompKneeDb => "Softness of compression onset. Higher dB blends smoothly without pumping.",
        ParamId::CompMakeupDb => "Manual volume boost after compression (editing turns Auto Makeup off).",
        ParamId::CompAutoMakeup => "Automatically restores loudness lost during compression.",
        ParamId::LimiterCeilingDb => "Hard safety ceiling so output never distorts or clips in Discord/OBS.",
        ParamId::LimiterTruePeak => "Catches inter-sample peaks with 4x oversampling for strict broadcast safety.",
    }
}

/// Parses and applies the active numeric field: routes through
/// `set_param_optimistic` (pins per D-007 and auto-enables inactive stages) and
/// leaves an inline clamped/out-of-range message when needed.
fn commit_field(s: &mut AppState, id: ParamId, input: &str) {
    match commit_value(id, input) {
        FieldCommit::Applied { value, clamped } => {
            s.set_param_optimistic(id, value);
            s.ui.pro_field.close();
            if clamped {
                s.ui.pro_field.message = Some(format!(
                    "{} clamped to {} – {}",
                    id.spec().label,
                    value_text(id, id.spec().min),
                    value_text(id, id.spec().max)
                ));
            }
        }
        FieldCommit::Rejected { message } => {
            s.ui.pro_field.message = Some(message);
        }
    }
}

/// Sets a parameter from the rack: optimistic local write + auto-enable + pin + `SetParam`.
fn set_param(s: &mut AppState, id: ParamId, v: f32) {
    s.set_param_optimistic(id, v);
}

/// Renders the Pro Rack drawer.
///
/// Collapsed, the DOM is byte-identical to the T19 scaffold so the blessed
/// `golden_line`/`apo_not_installed` snapshots stay intact.
pub fn view(s: &AppState, cx: &mut Cx<AppState>, t: TineTheme) -> El {
    let toggle_label = if s.ui.pro_drawer_open {
        "Pro Controls ▲"
    } else {
        "Pro Controls ▾"
    };

    let open = s.ui.pro_drawer_open;
    let toggle = kit::button(
        "pro/toggle",
        "button",
        toggle_label,
        if open { Tone::Selected } else { Tone::Secondary },
        Some(&glyph::SLIDERS),
        t,
        |s| {
            let open = !s.ui.pro_drawer_open;
            s.ui.pro_drawer_open = open;
            s.ui.pro_field.close();
            s.emit(Intent::SetProDrawerOpen(open));
        },
    );

    let tel = &s.live.telemetry;
    let mut header = row()
        .key("pro/header")
        .w_full()
        .gap(spacing::SM)
        .center()
        .child(toggle)
        .child(el().key("pro/header-spacer").grow(1.0));
    // Collapsed: compact live readouts. Open: the drawer's own meters show the same
    // values as bars, so the chips would only repeat them.
    if !open {
        header = header
            .child(gr_chip("pro/chip-gate", "Gate", tel.gate_attenuation_db, t))
            .child(gr_chip("pro/chip-deess", "De-Ess", tel.deesser_reduction_db, t))
            .child(gr_chip("pro/chip-comp", "Comp", tel.comp_gain_reduction_db, t));
    }
    let mut container = kit::card("pro-drawer", t)
        .px(spacing::LG)
        .py(if open { spacing::SM } else { spacing::MD })
        .gap(if open { 6.0 } else { spacing::SM })
        .child(header);

    if s.ui.pro_drawer_open {
        // Bounded drawer: takes the leftover window height (the canvas keeps its
        // 196 pt compact height) so the rack scrolls inside it instead of off-window.
        container = container
            .grow(1.0)
            .min_h(0.0)
            .child(drawer_header(s, t))
            .child(coach_banner(s, t))
            .child(gr_row(s, t))
            .child(
                rack_scroll(s, cx, t).transition(
                    Transition::move_edge(rune::Edge::Bottom)
                        .combined(Transition::opacity())
                        .hold_layout()
                        .springs(SpringPair::new(Spring::SNAPPY, Spring::SMOOTH)),
                ),
            );
    }

    container
}

/// Live beginner coach banner in Pro Controls: explains the hovered control or
/// how Pro pins & stages work when idle.
fn coach_banner(s: &AppState, t: TineTheme) -> El {
    let (badge, hint) = match s.ui.hovered_param {
        Some(id) => (id.spec().label, param_hint(id)),
        None => (
            "PRO TIP",
            "Hover or drag any slider to tune it. Custom edits show 'pinned' — click 'pinned' or 'Unpin all' to relink.",
        ),
    };
    row()
        .key("pro/coach-banner")
        .role("status")
        .label(format!("{badge}: {hint}"))
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
                .key("pro/coach-badge")
                .h(18.0)
                .px(6.0)
                .center()
                .radius(radii::SM)
                .fill(if s.ui.hovered_param.is_some() {
                    t.deck_blue_soft
                } else {
                    t.accent_soft
                })
                .child(
                    text(badge)
                        .font(typography::EYEBROW)
                        .color(if s.ui.hovered_param.is_some() {
                            t.deck_blue_text
                        } else {
                            t.accent_text
                        }),
                ),
        )
        .child(
            row()
                .key("pro/coach-text-wrap")
                .grow(1.0)
                .child(text(hint).key("pro/coach-text").font(typography::CAPTION).color(t.text)),
        )
}

/// Rack header: title, pinned count and the global **Unpin all**.
fn drawer_header(s: &AppState, t: TineTheme) -> El {
    let pin_count = pinned_params(&s.model.pins).len();

    let mut unpin = pill_button("pro/unpin-all", "button", "Unpin all", true, t, |s| {
        s.model.pins.clear();
        s.ui.pro_field.close();
        s.emit(Intent::UnpinAll);
    });
    if pin_count == 0 {
        unpin = unpin.disabled(true);
    }

    let keep_warm = !s.model.park_when_unused;
    let ptt_toggle = pill_button("pro/ptt-toggle", "switch", "Keep Warm (PTT)", keep_warm, t, |s| {
        let next_park = !s.model.park_when_unused;
        s.model.park_when_unused = next_park;
        s.emit(Intent::SetParkWhenUnused(next_park));
    })
    .checked(keep_warm);

    row()
        .key("pro/rack-header")
        .w_full()
        .gap(spacing::MD)
        .center()
        .child(
            text("Surgical Pro Rack · stages 1–6")
                .font(typography::HEADING)
                .color(t.text),
        )
        .child(el().key("pro/rack-spacer").grow(1.0))
        .child(
            text(if pin_count == 0 {
                String::from("No custom pins — 4 main knobs drive everything")
            } else {
                format!("{pin_count} pinned · click 'pinned' or move main knob to relink")
            })
            .font(typography::CAPTION)
            .color(t.text_dim),
        )
        .child(ptt_toggle)
        .child(unpin)
}

/// Live GR trio (gate / de-esser / compressor) plus the output meter with the
/// limiter-ceiling marker.
fn gr_row(s: &AppState, t: TineTheme) -> El {
    let tel = &s.live.telemetry;

    let gr_group = |key: &'static str, label: &str, db: f32| {
        let norm = (-db).clamp(0.0, 24.0) / 24.0;
        row()
            .key(key)
            .role("meter")
            .label(format!("{label} gain reduction {:.1} dB", -db))
            .gap(spacing::SM)
            .center()
            .child(text(label).font(typography::CAPTION).color(t.text_dim))
            .child(
                el().key(format!("{key}/track"))
                    .w(90.0)
                    .h(8.0)
                    .radius(radii::SM)
                    .fill(t.well)
                    .border(1.0, t.border)
                    .child(canvas(track_fill(90.0, 8.0, norm, t.accent)).key(format!("{key}/fill"))),
            )
            .child(
                text(format!("{:.1} dB", -db))
                    .font(typography::CAPTION)
                    .color(t.text_dim),
            )
    };

    let ceil = s.model.params.limiter_ceiling_db;
    let out_norm = ((tel.output_peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    let ceil_norm = ((ceil + 60.0) / 60.0).clamp(0.0, 1.0);
    let out_group = row()
        .key("pro/out-meter")
        .role("meter")
        .label(format!("Output {:.1} dBFS", tel.output_peak_db))
        .gap(spacing::SM)
        .center()
        .child(text("Out").font(typography::CAPTION).color(t.text_dim))
        .child(
            el().key("pro/out-track")
                .w(150.0)
                .h(8.0)
                .radius(radii::SM)
                .fill(t.surface_raised)
                .border(1.0, t.border)
                .child(
                    canvas(with_tick(
                        track_fill(150.0, 8.0, out_norm, meter_color(tel.output_peak_db, t)),
                        150.0,
                        8.0,
                        ceil_norm,
                        t.accent,
                    ))
                    .key("pro/out-fill")
                    .size(150.0, 8.0),
                ),
        )
        .child(
            text(format!("{:.1} dBFS", tel.output_peak_db))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );

    row()
        .key("pro/gr-row")
        .role("group")
        .label("Live gain reduction meters")
        .w_full()
        .gap(spacing::LG)
        .center()
        .child(gr_group("pro/gr-gate", "Gate", tel.gate_attenuation_db))
        .child(gr_group("pro/gr-deess", "De-Ess", tel.deesser_reduction_db))
        .child(gr_group("pro/gr-comp", "Comp", tel.comp_gain_reduction_db))
        .child(el().key("pro/gr-spacer").grow(1.0))
        .child(out_group)
}

/// Filled portion of a horizontal meter track.
fn track_fill(w: f32, h: f32, norm: f32, color: rune::Color) -> rune::Shape {
    let fw = (norm.clamp(0.0, 1.0) * w).max(0.0);
    rune::Shape::new().fill(vec![[0.0, 0.0], [fw, 0.0], [fw, h], [0.0, h]], color)
}

fn meter_color(db: f32, t: TineTheme) -> rune::Color {
    if db > -1.5 {
        t.meter_red
    } else if db > -12.0 {
        t.meter_amber
    } else {
        t.meter_green
    }
}

/// Adds a vertical ceiling tick to a shape.
fn with_tick(mut shape: rune::Shape, w: f32, h: f32, norm: f32, color: rune::Color) -> rune::Shape {
    let x = (norm.clamp(0.0, 1.0) * w).max(0.5);
    shape = shape.fill(vec![[x - 0.5, 0.0], [x + 0.5, 0.0], [x + 0.5, h], [x - 0.5, h]], color);
    shape
}

/// The bounded, vertically scrolling rack body. The viewport grows into the
/// drawer's leftover height (at least `VIEWPORT_MIN` pt) and never runs the rack
/// off the window.
const VIEWPORT_MIN: f32 = 120.0;

fn rack_scroll(s: &AppState, cx: &mut Cx<AppState>, t: TineTheme) -> El {
    let mut body = col().key("pro/scroll-content").w_full().gap(spacing::SM);

    for stage in STAGES {
        body = body.child(section(s, cx, stage, t));
    }

    col()
        .key("pro/scroll-viewport")
        .w_full()
        .grow(1.0)
        .min_h(VIEWPORT_MIN)
        .radius(radii::MD)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(body)
}

/// A compact live gain-reduction readout chip for the drawer header.
fn gr_chip(key: &'static str, name: &'static str, db: f32, t: TineTheme) -> El {
    row()
        .key(key)
        .role("status")
        .label(format!("{name} {db:.1} dB"))
        .h(24.0)
        .px(spacing::SM)
        .gap(6.0)
        .center()
        .radius(radii::PILL)
        .fill(t.well)
        .border(1.0, t.border)
        .child(text(name).font(typography::EYEBROW).color(t.text_dim))
        .child(text(format!("{db:.1} dB")).font(typography::CAPTION).color(t.text))
}

/// Unpins every pinned parameter in `stage` and emits `Intent::Unpin(id)` for each.
fn unpin_section(s: &mut AppState, stage: Stage) {
    for id in section_params(stage) {
        if s.model.pins.is_pinned(id) {
            s.model.pins.unpin(id);
            s.emit(Intent::Unpin(id));
        }
    }
    s.ui.pro_field.close();
}

fn stage_subtitle(number: u8) -> &'static str {
    match number {
        1 => "Mic gain & low-rumble cut",
        2 => "AI noise removal & silence gate",
        3 => "Tone shaping — move Gain off 0 dB first, then adjust Freq & Width (Q)",
        4 => "Tames harsh 'S' and 'T' sounds",
        5 => "Evens out whispers & loud speech",
        6 => "Brickwall ceiling to prevent clipping",
        _ => "",
    }
}

fn band_role(band: Band) -> (&'static str, &'static str) {
    match band.as_u8() {
        0 => ("Band 1 · Low Warmth", "Chest warmth (+) or trim boomy bass (−)"),
        1 => ("Band 2 · Mud & Boxiness", "Cuts hollow room or desk mud (−)"),
        2 => (
            "Band 3 · Nasal / Midrange",
            "Fixes honky mids (−) or adds vocal body (+)",
        ),
        3 => ("Band 4 · Vocal Clarity", "Brings speech forward through game/music (+)"),
        _ => ("Band 5 · Air & Sparkle", "Adds crisp studio brightness (+)"),
    }
}

fn filter_type_short(ft: FilterType) -> &'static str {
    FILTER_OPTIONS
        .iter()
        .find(|(t, _)| *t == ft)
        .map_or("Peaking", |(_, n)| *n)
}

/// Beginner-friendly Band card for Stage 3: groups Gain, Freq, Filter Type, and Q together.
fn eq_band_card(s: &AppState, cx: &mut Cx<AppState>, band: Band, t: TineTheme) -> El {
    let idx = band.index();
    let (title, role_desc) = band_role(band);
    let ft = s.model.params.eq_type[idx];
    let freq = s.model.params.eq_freq_hz[idx];
    let gain = s.model.params.eq_gain_db[idx];
    let is_gain_shape = matches!(ft, FilterType::LowShelf | FilterType::HighShelf | FilterType::Peaking);

    let (status_text, status_color) = if is_gain_shape && gain.abs() < 0.05 {
        (String::from("0.0 dB · Flat (move Gain to activate)"), t.text_dim)
    } else if is_gain_shape {
        (
            format!("{gain:+.1} dB @ {}", value_text(ParamId::EqFreqHz(band), freq)),
            t.accent_text,
        )
    } else {
        (
            format!(
                "{} @ {}",
                filter_type_short(ft),
                value_text(ParamId::EqFreqHz(band), freq)
            ),
            t.accent_text,
        )
    };

    let band_header = row()
        .key(format!("pro/eq-band/{idx}/header"))
        .w_full()
        .gap(spacing::SM)
        .center()
        .child(text(title).font(typography::LABEL).color(t.text))
        .child(text(role_desc).font(typography::CAPTION).color(t.text_dim))
        .child(el().key(format!("pro/eq-band/{idx}/spacer")).grow(1.0))
        .child(text(status_text).font(typography::CAPTION).color(status_color));

    let left = col()
        .key(format!("pro/eq-band/{idx}/col-l"))
        .w_full()
        .grow(1.0)
        .gap(2.0)
        .child(param_row(s, cx, ParamId::EqGainDb(band), t))
        .child(param_row(s, cx, ParamId::EqFreqHz(band), t));

    let right = col()
        .key(format!("pro/eq-band/{idx}/col-r"))
        .w_full()
        .grow(1.0)
        .gap(2.0)
        .child(param_row(s, cx, ParamId::EqType(band), t))
        .child(param_row(s, cx, ParamId::EqQ(band), t));

    col()
        .key(format!("pro/eq-band/{idx}"))
        .w_full()
        .gap(spacing::XS)
        .px(spacing::SM)
        .py(spacing::XS + 2.0)
        .radius(radii::SM)
        .fill(t.surface)
        .border(1.0, t.border)
        .child(band_header)
        .child(
            row()
                .key(format!("pro/eq-band/{idx}/grid"))
                .w_full()
                .gap(spacing::MD)
                .child(left)
                .child(right),
        )
}

/// One stage section: header (title, optional enable switch, optional section
/// "Unpin all") and a grouped body of generated control rows.
fn section(s: &AppState, cx: &mut Cx<AppState>, stage: Stage, t: TineTheme) -> El {
    let mut header = row()
        .key(format!("pro/sec/{}/header", stage.id()))
        .w_full()
        .gap(spacing::MD)
        .center()
        .child(
            row()
                .key("pro/sec-badge")
                .size(20.0, 20.0)
                .center()
                .radius(radii::SM)
                .fill(t.accent_soft)
                .child(
                    text(format!("{}", stage.number))
                        .font(typography::CAPTION)
                        .color(t.accent_text),
                ),
        )
        .child(text(stage.title).font(typography::LABEL).color(t.text))
        .child(
            text(stage_subtitle(stage.number))
                .font(typography::CAPTION)
                .color(t.text_dim),
        );

    if let Some(enable) = stage.enable {
        let spec = enable.spec();
        let on = s.model.params.get(enable) >= 0.5;
        let mut spacer_area = row().key("pro/sec-spacer").grow(1.0);
        if !on {
            spacer_area = spacer_area.child(
                text("Off (drag any slider below to auto-enable)")
                    .font(typography::CAPTION)
                    .color(t.text_dim),
            );
        }
        header = header.child(spacer_area).child(
            row()
                .key(control_key(enable))
                .role("switch")
                .checked(on)
                .label(format!("{} {}", spec.label, if on { "on" } else { "off" }))
                .focusable(true)
                .h(24.0)
                .w(64.0)
                .gap(6.0)
                .center()
                .radius(radii::PILL)
                .cursor(rune::Cursor::Pointer)
                .on_hover_change(move |s: &mut AppState, inside: bool, _tx: &mut Tx| {
                    if inside {
                        s.ui.hovered_param = Some(enable);
                    } else if s.ui.hovered_param == Some(enable) {
                        s.ui.hovered_param = None;
                    }
                })
                .on_press(move |s: &mut AppState| {
                    let next = if s.model.params.get(enable) >= 0.5 { 0.0 } else { 1.0 };
                    set_param(s, enable, next);
                })
                .child(kit::switch_track(format!("switch-track/{}", spec.key), on, t))
                .child(
                    text(if on { "On" } else { "Off" })
                        .font(typography::CAPTION)
                        .color(if on { t.text } else { t.text_dim }),
                ),
        );
    }

    let section_ids = section_params(stage);
    let npinned = section_ids.iter().filter(|id| s.model.pins.is_pinned(**id)).count();

    if npinned > 0 {
        header = header.child(pill_button(
            format!("pro/sec/{}/unpin-all", stage.id()),
            "button",
            format!("Unpin all ({npinned})"),
            false,
            t,
            move |s| {
                unpin_section(s, stage);
            },
        ));
    }

    let body = if stage.number == 3 {
        let mut bands_col = col()
            .key(format!("pro/sec/{}/body", stage.id()))
            .w_full()
            .gap(spacing::XS);
        for band in EQ_BANDS {
            bands_col = bands_col.child(eq_band_card(s, cx, band, t));
        }
        bands_col
    } else {
        // Two-column body: rows split cleanly so the rack stays compact.
        let rows = section_rows(s, cx, stage, t);
        let half = rows.len().div_ceil(2);
        let (left, right) = rows.split_at(half);
        let mk_col = |parts: &[El], side: &'static str| {
            parts.iter().fold(
                col().key(format!("pro/sec-col-{side}")).w_full().grow(1.0).gap(2.0),
                |acc, item| acc.child(item.clone()),
            )
        };
        row()
            .key(format!("pro/sec/{}/body", stage.id()))
            .w_full()
            .gap(spacing::MD)
            .child(mk_col(left, "l"))
            .child(mk_col(right, "r"))
    };

    col()
        .key(format!("pro/sec/{}", stage.id()))
        .w_full()
        .gap(spacing::SM)
        .radius(radii::MD)
        .fill(t.surface_raised)
        .border(1.0, t.border)
        .px(spacing::MD)
        .py(spacing::SM)
        .child(header)
        .child(body)
}

/// One control row per body parameter of the section (the enable switch is drawn in
/// the header, not duplicated here).
fn section_rows(s: &AppState, cx: &mut Cx<AppState>, stage: Stage, t: TineTheme) -> Vec<El> {
    let enable = stage.enable;
    section_params(stage)
        .into_iter()
        .filter(|id| Some(*id) != enable)
        .map(|id| param_row(s, cx, id, t))
        .collect()
}

fn pill_button(
    key: impl Into<String>,
    role: &'static str,
    label: impl Into<String>,
    active: bool,
    t: TineTheme,
    on_press: impl Fn(&mut AppState) + 'static,
) -> El {
    let tone = if active { Tone::Selected } else { Tone::Secondary };
    kit::button(key, role, label, tone, None, t, on_press)
}

/// Width of a parameter row's label cell (fits "pinned" + the longest label).
const LABEL_CELL_W: f32 = 172.0;

/// One generated control row: pin indicator (when pinned) + label + control.
fn param_row(s: &AppState, cx: &mut Cx<AppState>, id: ParamId, t: TineTheme) -> El {
    let spec = id.spec();
    let pinned = s.model.pins.is_pinned(id);

    let mut head = row()
        .key(format!("pro/param/{}", spec.key))
        .w_full()
        .gap(spacing::SM)
        .center()
        .h(30.0)
        .on_hover_change(move |s: &mut AppState, inside: bool, _tx: &mut Tx| {
            if inside {
                s.ui.hovered_param = Some(id);
            } else if s.ui.hovered_param == Some(id) {
                s.ui.hovered_param = None;
            }
        });

    // Fixed-width label cell (pin pill + name) so every slider in a column starts
    // at the same x.
    let mut cell = row()
        .key(format!("pro/param-label/{}", spec.key))
        .w(LABEL_CELL_W)
        .gap(spacing::XS)
        .center();
    if pinned {
        cell = cell.child(unpin_pill(id, t));
    }
    cell = cell
        .child(
            text(spec.label)
                .font(typography::CAPTION)
                .color(if pinned { t.accent_text } else { t.text_dim }),
        )
        .child(el().key("pro/param-label-pad").grow(1.0));
    head = head.child(cell);

    let control = control_body(s, cx, id, t);
    head.child(control)
}

/// The interactive part of a row, by the spec's control scale.
fn control_body(s: &AppState, cx: &mut Cx<AppState>, id: ParamId, t: TineTheme) -> El {
    match id.spec().scale {
        Scale::Bool => switch_row(s, id, t),
        Scale::Enum => select_row(s, id, t),
        Scale::Linear | Scale::Log => {
            let spec = id.spec();
            let active_edit = s.ui.pro_field.param == Some(id);
            let mut body = row()
                .key(format!("pro/param-body/{}", spec.key))
                .gap(spacing::XS)
                .grow(1.0)
                .child(slider_el(s, cx, id, t));

            if active_edit {
                body = body
                    .child(
                        rune::text_input(&s.ui.pro_field.edit, typography::CAPTION)
                            .key(format!("field-edit/{}", spec.key))
                            .single_line()
                            .w(96.0)
                            .on_edit(|s: &mut AppState| &mut s.ui.pro_field.edit)
                            .on_input_key(move |s: &mut AppState, ev: &rune::KeyEvent, tx: &mut Tx| match ev.key {
                                Key::Enter => {
                                    if s.ui.pro_field.param == Some(id) {
                                        let txt = s.ui.pro_field.edit.text.clone();
                                        commit_field(s, id, &txt);
                                        tx.instant();
                                    }
                                    true
                                }
                                Key::Escape => {
                                    s.ui.pro_field.close();
                                    tx.instant();
                                    true
                                }
                                _ => false,
                            }),
                    )
                    .child(
                        text("Enter to apply")
                            .key(format!("field-hint/{}", spec.key))
                            .font(typography::CAPTION)
                            .color(t.text_dim),
                    );
            } else {
                let readout = if id == ParamId::CompMakeupDb && s.model.params.comp_auto_makeup {
                    String::from("Auto")
                } else {
                    value_text(id, s.model.params.get(id))
                };
                body = body.child(
                    row()
                        .key(format!("field/{}", spec.key))
                        .role("textbox")
                        .label(format!(
                            "{} value {}",
                            spec.label,
                            value_text(id, s.model.params.get(id))
                        ))
                        .h(22.0)
                        .w(104.0)
                        .center()
                        .radius(radii::SM)
                        .fill(t.well)
                        .border(1.0, t.border)
                        .hover(t.surface_hover)
                        .focusable(false)
                        .cursor(rune::Cursor::Pointer)
                        .on_press(move |s: &mut AppState| {
                            if s.ui.pro_field.param != Some(id) {
                                s.ui.pro_field = ProFieldEdit::open(id, &s.model);
                            }
                        })
                        .child(text(readout).font(typography::CAPTION).color(t.text)),
                );
            }

            if let Some(msg) = s.ui.pro_field.message.clone()
                && s.ui.pro_field.message_shows(id)
            {
                body = body.child(
                    text(msg)
                        .key(format!("field-msg/{}", spec.key))
                        .font(typography::CAPTION)
                        .color(t.danger),
                );
            }

            body
        }
    }
}

impl ProFieldEdit {
    /// Whether the inline message belongs to this parameter's row.
    fn message_shows(&self, id: ParamId) -> bool {
        self.param == Some(id) || self.message.as_ref().is_some_and(|m| m.starts_with(id.spec().label))
    }
}

/// The tiny "pinned" pill — the pin indicator; clicking it unpins (D-007).
fn unpin_pill(id: ParamId, t: TineTheme) -> El {
    let spec = id.spec();
    row()
        .key(pin_key(id))
        .role("button")
        .label(format!("Unpin {}", spec.label))
        .focusable(true)
        .h(20.0)
        .px(spacing::SM)
        .center()
        .radius(radii::PILL)
        .fill(t.accent_soft)
        .border(1.0, t.accent_glow)
        .on_press(move |s: &mut AppState| {
            s.model.pins.unpin(id);
            if s.ui.pro_field.param == Some(id) {
                s.ui.pro_field.close();
            }
            s.emit(Intent::Unpin(id));
        })
        .child(text("pinned").font(typography::CAPTION).color(t.accent_text))
}

/// Boolean parameter row: pill switch labelled by the spec.
fn switch_row(s: &AppState, id: ParamId, t: TineTheme) -> El {
    let spec = id.spec();
    let on = s.model.params.get(id) >= 0.5;

    row()
        .key(control_key(id))
        .role("switch")
        .checked(on)
        .label(format!("{} {}", spec.label, if on { "on" } else { "off" }))
        .focusable(true)
        .h(24.0)
        .w(64.0)
        .gap(6.0)
        .center()
        .radius(radii::PILL)
        .cursor(rune::Cursor::Pointer)
        .on_press(move |s: &mut AppState| {
            let next = if s.model.params.get(id) >= 0.5 { 0.0 } else { 1.0 };
            set_param(s, id, next);
        })
        .child(kit::switch_track(format!("switch-track/{}", spec.key), on, t))
        .child(
            text(if on { "On" } else { "Off" })
                .font(typography::CAPTION)
                .color(if on { t.text } else { t.text_dim }),
        )
}

/// EQ band type select: a native popup menu of the seven filter types.
fn select_row(s: &AppState, id: ParamId, t: TineTheme) -> El {
    let Some(ParamId::EqType(band)) = Some(id) else {
        return el().key(control_key(id));
    };
    let current = FilterType::from_f32(s.model.params.get(id));
    let label = filter_type_label(current, band);
    let current_name = FILTER_OPTIONS
        .iter()
        .find(|(ft, _)| *ft == current)
        .map_or_else(String::new, |(_, n)| (*n).to_owned());

    row()
        .key(control_key(id))
        .role("combobox")
        .label(label.clone())
        .focusable(true)
        .h(22.0)
        .px(spacing::SM)
        .center()
        .radius(radii::SM)
        .fill(t.well)
        .border(1.0, t.border)
        .hover(t.surface_hover)
        .on_press_tx(move |s: &mut AppState, tx: &mut rune::Tx| {
            if !popup_menu_supported() {
                // Without native menus cycle Low-Shelf → Peaking → High-Shelf.
                let cycled = match FilterType::from_f32(s.model.params.get(id)) {
                    FilterType::LowShelf => FilterType::Peaking,
                    FilterType::Peaking => FilterType::HighShelf,
                    _ => FilterType::LowShelf,
                };
                set_param(s, id, cycled.as_f32());
                return;
            }
            let items: Vec<(String, String)> = FILTER_OPTIONS
                .iter()
                .map(|(_ft, name)| ((*name).to_owned(), (*name).to_owned()))
                .collect();
            let menu = PopupMenu::picker(items, &current_name);
            tx.popup_menu(
                menu,
                control_key(id),
                move |s: &mut AppState, picked: Option<&str>, _tx| {
                    if let Some(picked) = picked
                        && let Some((ft, _)) = FILTER_OPTIONS.iter().find(|(_, name)| *name == picked)
                    {
                        set_param(s, id, ft.as_f32());
                    }
                },
            );
        })
        .child(text(format!("{label} ▾")).font(typography::CAPTION).color(t.text))
}

/// Log/linear slider: a focusable `role="slider"` with native arrow stepping
/// (`on_step`), Home/End limits, double-click reset/unpin, and pointer jump/scrub
/// through `on_drag` mapped onto the spec's taper using element-local coordinates.
fn slider_el(s: &AppState, cx: &mut Cx<AppState>, id: ParamId, t: TineTheme) -> El {
    let spec = id.spec();
    let value = s.model.params.get(id);
    let pos = to_pos(value, spec);

    // Settled track rect from the previous layout pass, as fallback only.
    let track = cx.layout(&control_key(id));

    let mut sl = row()
        .key(control_key(id))
        .role("slider")
        .label(format!("{} = {}", spec.label, value_text(id, value)))
        .range_value(f64::from(value), f64::from(spec.min), f64::from(spec.max))
        .range_step(f64::from(spec.step))
        .grow(1.0)
        .h(22.0)
        .center()
        .cursor(rune::Cursor::Pointer)
        .on_step(move |s: &mut AppState, delta: i32, _tx: &mut Tx| {
            let spec = id.spec();
            let next = s.model.params.get(id) + (delta as f32) * spec.step;
            set_param(s, id, from_step(next, spec));
        })
        .on_step_limit(move |s: &mut AppState, to_max: bool, _tx: &mut Tx| {
            let spec = id.spec();
            set_param(s, id, if to_max { spec.max } else { spec.min });
        })
        .on_double_press(move |s: &mut AppState| {
            s.reset_param_or_unpin(id);
        })
        .on_drag(move |s: &mut AppState, d: &mut Drag, _tx: &mut Tx| {
            let spec = id.spec();
            let pos_from_drag = |d: &Drag| -> Option<f32> {
                if d.w > 1.0 {
                    Some((d.x / d.w).clamp(0.0, 1.0))
                } else {
                    track.map(|[x, _y, w, _h]| ((d.window_point()[0] - x) / w.max(1.0)).clamp(0.0, 1.0))
                }
            };
            match d.phase {
                Phase::Down => {
                    d.data = s.model.params.get(id);
                    s.ui.hovered_param = Some(id);
                    if let Some(p) = pos_from_drag(d) {
                        set_param(s, id, from_pos(p, spec));
                    }
                }
                Phase::Move => {
                    if let Some(p) = pos_from_drag(d) {
                        set_param(s, id, from_pos(p, spec));
                    } else {
                        let p = (to_pos(d.data, spec) + d.dx / 200.0).clamp(0.0, 1.0);
                        set_param(s, id, from_pos(p, spec));
                    }
                }
                Phase::Up => {
                    if d.cancelled() {
                        set_param(s, id, d.data);
                    }
                }
            }
        });

    sl = sl.child(
        el().key(format!("slider/{}/fill", spec.key))
            .grow(pos.max(0.001))
            .h(6.0)
            .radius(3.0)
            .fill(t.accent)
            .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, t.accent_hi), (1.0, t.accent)]),
    );
    sl = sl.child(
        el().key(format!("slider/{}/knob", spec.key))
            .size(14.0, 14.0)
            .radius(7.0)
            .fill(t.knob_outer)
            .border(2.0, t.deck_blue)
            .shadow(1.0, 4.0, kit::oklcha_black(0.28)),
    );
    sl = sl.child(
        el().key(format!("slider/{}/rest", spec.key))
            .grow((1.0 - pos).max(0.001))
            .h(6.0)
            .radius(3.0)
            .fill(t.well)
            .border(1.0, t.border),
    );

    sl
}

/// Steps keep the value inside the spec range and on the step grid.
fn from_step(v: f32, spec: &ParamSpec) -> f32 {
    let clamped = v.clamp(spec.min, spec.max);
    if spec.step > 0.0 {
        ((clamped / spec.step).round() * spec.step).clamp(spec.min, spec.max)
    } else {
        clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_bypass_param_has_a_section() {
        for i in 0..PARAM_COUNT {
            let Some(id) = ParamId::from_index(i) else { continue };
            if id == ParamId::Bypass {
                assert!(stage_of(id).is_none(), "Bypass lives in the header, not the rack");
                continue;
            }
            let Some(stage) = stage_entry(id) else {
                panic!("{id:?} has no pro rack section");
            };
            assert!(
                section_params(stage).contains(&id),
                "{id:?} not enumerated in section {}",
                stage.number
            );
        }
    }

    #[test]
    fn log_taper_round_trips() {
        let log_spec = ParamId::EqFreqHz(Band::B1).spec(); // 20 Hz..20 kHz, Log
        assert_eq!(log_spec.scale, Scale::Log);
        for &v in &[20.0, 110.0, 320.0, 6800.0, 8500.0, 20_000.0] {
            let back = from_pos(to_pos(v, log_spec), log_spec);
            assert!((back - v).abs() <= v * 1e-4, "{v} → {back}");
        }

        let lin_spec = ParamId::CompRatio.spec();
        for &v in &[1.0, 7.3, 20.0] {
            let back = from_pos(to_pos(v, lin_spec), lin_spec);
            assert!((back - v).abs() < 1e-4);
        }
    }

    #[test]
    fn parse_numeric_tolerates_units() {
        let near = |got: Option<f32>, want: f32| {
            assert!((got.unwrap_or(f32::NAN) - want).abs() < 1e-6, "{got:?} != {want}");
        };
        near(parse_numeric("3.0"), 3.0);
        near(parse_numeric("-48 dB"), -48.0);
        near(parse_numeric(" 6800 Hz "), 6800.0);
        near(parse_numeric("4:1"), 4.0);
        near(parse_numeric("85 %"), 85.0);
        assert!(parse_numeric("abc").is_none());
        assert!(parse_numeric("").is_none());
        assert!(parse_numeric("nan").is_none());
    }

    #[test]
    fn commit_clamps_and_messages() {
        let id = ParamId::CompRatio; // 1.0..=20.0, step 0.1
        let FieldCommit::Applied { value, clamped } = commit_value(id, "12.5") else {
            panic!("Expected Applied");
        };
        assert!((value - 12.5).abs() < 1e-6);
        assert!(!clamped);

        let FieldCommit::Applied { value, clamped } = commit_value(id, "99") else {
            panic!("Expected clamped Applied");
        };
        assert!((value - 20.0).abs() < 1e-6, "clamps to range");
        assert!(clamped);

        assert_eq!(
            commit_value(id, "xx"),
            FieldCommit::Rejected {
                message: String::from("Enter a number"),
            }
        );
    }

    #[test]
    fn pinned_params_enumerate_specs_order() {
        let mut pins = PinSet::EMPTY;
        pins.pin(ParamId::CompRatio);
        pins.pin(ParamId::HpfFreqHz);
        assert_eq!(pinned_params(&pins), vec![ParamId::HpfFreqHz, ParamId::CompRatio]);
        assert_eq!(pinned_params(&PinSet::EMPTY).len(), 0);
    }

    #[test]
    fn value_text_formats_units() {
        assert_eq!(value_text(ParamId::CompRatio, 3.0), "3.0:1");
        assert_eq!(value_text(ParamId::HpfFreqHz, 80.0), "80 Hz");
        assert_eq!(value_text(ParamId::EqFreqHz(Band::B3), 4200.0), "4.2 kHz");
        assert_eq!(value_text(ParamId::DenoiseMix, 0.85), "85 %");
        assert_eq!(value_text(ParamId::EqType(Band::B0), 0.0), "EQ 1 Type: Low-Shelf");
        assert_eq!(value_text(ParamId::GateThresholdDb, -48.0), "-48.0 dBFS");
    }

    #[test]
    fn section_unpin_lists_own_params_only() {
        let s1 = STAGES[0];
        let s3 = STAGES[2];
        assert!(section_params(s1).contains(&ParamId::HpfEnabled));
        assert!(section_params(s3).contains(&ParamId::EqType(Band::B2)));
        assert!(!section_params(s1).contains(&ParamId::CompRatio));
    }

    #[test]
    fn unpin_section_emits_per_param_intents_and_preserves_other_stages() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::intent::IntentSink;

        struct VecSink(Rc<RefCell<Vec<Intent>>>);
        impl IntentSink for VecSink {
            fn send(&self, intent: Intent) {
                self.0.borrow_mut().push(intent);
            }
        }

        let recorded = Rc::new(RefCell::new(Vec::<Intent>::new()));
        let mut s = AppState::default();
        s.set_sink(VecSink(Rc::clone(&recorded)));

        // Pin two params in Stage 1 and one in Stage 5 (Compressor).
        s.model.pins.pin(ParamId::InputGainDb);
        s.model.pins.pin(ParamId::HpfFreqHz);
        s.model.pins.pin(ParamId::CompRatio);

        unpin_section(&mut s, STAGES[0]);

        assert!(!s.model.pins.is_pinned(ParamId::InputGainDb));
        assert!(!s.model.pins.is_pinned(ParamId::HpfFreqHz));
        assert!(
            s.model.pins.is_pinned(ParamId::CompRatio),
            "other stage pins must be preserved"
        );

        let intents = recorded.borrow();
        assert!(!intents.contains(&Intent::UnpinAll), "must not emit Intent::UnpinAll");
        assert!(intents.contains(&Intent::Unpin(ParamId::InputGainDb)));
        assert!(intents.contains(&Intent::Unpin(ParamId::HpfFreqHz)));
        assert_eq!(intents.len(), 2);
    }
}
