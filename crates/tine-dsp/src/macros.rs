//! Golden Line macro mapping (Layer C) with Pro-mode pin support (D-007).
//!
//! [`MacroMapper`] is the pure, allocation-free bridge between the four "Golden
//! Line" macro nodes and the concrete [`DspParams`] the pipeline runs on:
//!
//! ```text
//! DspParams = MacroMapper( archetype_base ⊕ calibration , macro_positions )  ⊕  pins
//! ```
//!
//! # Curve semantics (D-020)
//!
//! Every macro position `m` is normalized (`0.0..=1.0`) and, at `m = 0.5`,
//! reproduces its *effective base* exactly (preset Layer A value, or the
//! calibrated value when one exists for that parameter).
//!
//! * **Purity** — `denoise_mix` moves ±0.5 around the effective mix; the gate
//!   threshold moves ±15 dB around the effective threshold (the documented
//!   `−60 → −30 dB` span re-anchored onto the base, rising: more purity trusts
//!   the denoiser and gates less).
//! * **Body** — low-shelf gain (band 0) swings ±4 dB around the base (documented
//!   `−2 → +6 dB` span); the mud cut (band 1) only *deepens* above the centre,
//!   down to `−5 dB` at `m = 1` (never boosts the mud).
//! * **Presence** — the presence peak (band 3) and air shelf (band 4) only
//!   *boost*, up to `+5 dB` / `+6 dB` above the centre. The de-esser turns on
//!   above `0.6` (or stays on when the archetype base enables it), with its
//!   amount scaling `0.6 → 1.0` ⇒ `3 → 8 dB`.
//! * **Leveler** — compressor ratio swings ±1.75 around the base (documented
//!   `1.5 → 5` span); the threshold only *lowers* from the effective threshold,
//!   by up to `10 dB` at `m = 1` (`calibrated threshold − 0..10 dB`); auto-makeup
//!   is on (a calibration supplies a verified manual makeup instead).
//!
//! # Pin rule (D-007)
//!
//! A parameter edited in Pro mode is *pinned*: macros (and calibration fields)
//! stop driving it until it is unpinned. In [`MacroMapper::apply`] every driven
//! parameter that is pinned is copied verbatim from `current`.
//!
//! Real-time safety: every function here is pure arithmetic on [`Copy`] data —
//! no allocation, no locks, no syscalls, no panics (APO-safe).

use tine_protocol::params::{Band, DspParams, ParamId};
use tine_protocol::presets::{Calibration, MacroPositions, PinSet};

use crate::presets::PresetDef;

/// Purity: total `denoise_mix` swing around the effective base (doc: `0.0 → 1.0`).
pub const PURITY_MIX_SPAN: f32 = 1.0;
/// Purity: total gate threshold swing in dB around the effective base (doc: `−60 → −30 dB`).
pub const PURITY_GATE_SPAN_DB: f32 = 30.0;

/// Body: total low-shelf (band 0) gain swing in dB around the base (doc: `−2 → +6 dB`).
pub const BODY_SHELF_SPAN_DB: f32 = 8.0;
/// Body: deepest mud-cut (band 1) offset in dB at `body = 1` (doc: `0 → −5 dB`).
pub const BODY_MUD_CUT_DB: f32 = 5.0;

/// Presence: presence-peak (band 3) boost in dB at `presence = 1` (doc: `0 → +5 dB`).
pub const PRESENCE_PEAK_BOOST_DB: f32 = 5.0;
/// Presence: air-shelf (band 4) boost in dB at `presence = 1` (doc: `0 → +6 dB`).
pub const PRESENCE_AIR_BOOST_DB: f32 = 6.0;
/// Presence: macro position above which the dynamic de-esser engages.
pub const PRESENCE_DEESSER_POS: f32 = 0.6;
/// Presence: de-esser amount (dB) at the engagement position.
pub const PRESENCE_DEESSER_MIN_DB: f32 = 3.0;
/// Presence: de-esser amount (dB) at `presence = 1`.
pub const PRESENCE_DEESSER_MAX_DB: f32 = 8.0;

/// Leveler: total compressor ratio swing around the base (doc: `1.5 → 5.0`).
pub const LEVELER_RATIO_SPAN: f32 = 3.5;
/// Leveler: threshold drop (dB) at `leveler = 1` (doc: `effective threshold − 0..10 dB`).
pub const LEVELER_THRESH_CUT_DB: f32 = 10.0;

/// The four Golden Line macro nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Macro {
    /// Noise & gate: `RNNoise` wet mix and expander threshold.
    Purity,
    /// Warmth & mud control: low-shelf boost and mud cut.
    Body,
    /// Clarity & de-ess: presence peak, air shelf, dynamic de-esser.
    Presence,
    /// Compression & voice focus: ratio, threshold, auto-makeup.
    Leveler,
}

impl Macro {
    /// All four macros, in Golden Line order.
    pub const ALL: [Macro; 4] = [Macro::Purity, Macro::Body, Macro::Presence, Macro::Leveler];

    /// Returns the normalized position (`0.0..=1.0`) of this macro in `macros`.
    #[must_use]
    pub const fn position(self, macros: MacroPositions) -> f32 {
        match self {
            Macro::Purity => macros.purity,
            Macro::Body => macros.body,
            Macro::Presence => macros.presence,
            Macro::Leveler => macros.leveler,
        }
    }
}

/// Maximum number of rows an [`ExplainBuf`] can hold (contract: ≤ 8).
pub const MAX_EXPLAIN_ROWS: usize = 8;

/// One before/after row of a macro tooltip: what the macro changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplainRow {
    /// The driven parameter.
    pub id: ParamId,
    /// Effective base value (preset ⊕ calibration), before this macro acts.
    pub before: f32,
    /// Value the macro maps to (when unpinned).
    pub after: f32,
    /// D-007: `true` when the parameter is pinned, so a macro no longer controls it.
    pub pinned: bool,
}

/// Fixed-capacity, allocation-free tooltip buffer: ≤ [`MAX_EXPLAIN_ROWS`] rows.
#[derive(Clone, Copy, Debug)]
pub struct ExplainBuf {
    rows: [ExplainRow; MAX_EXPLAIN_ROWS],
    len: usize,
}

impl ExplainBuf {
    const ROW_ZERO: ExplainRow = ExplainRow {
        id: ParamId::Bypass,
        before: 0.0,
        after: 0.0,
        pinned: false,
    };

    /// Creates an empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            rows: [Self::ROW_ZERO; MAX_EXPLAIN_ROWS],
            len: 0,
        }
    }

    /// Number of rows currently held.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when no rows are held.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The rows written so far (valid prefix of the fixed array).
    #[must_use]
    pub fn rows(&self) -> &[ExplainRow] {
        self.rows.get(..self.len).unwrap_or(&[])
    }

    /// Removes all rows (capacity is kept; nothing is freed).
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Appends one row; silently ignores overflow past [`MAX_EXPLAIN_ROWS`]
    /// (fixed capacity, so this branch is fully predictable on the RT path).
    fn push(&mut self, row: ExplainRow) {
        if self.len < MAX_EXPLAIN_ROWS
            && let Some(slot) = self.rows.get_mut(self.len)
        {
            *slot = row;
            self.len += 1;
        }
    }
}

impl Default for ExplainBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters driven by **Purity** (both rise with the macro).
const PURITY_DRIVES: [ParamId; 2] = [ParamId::DenoiseMix, ParamId::GateThresholdDb];

/// Parameters driven by **Body** (shelf rises, mud gain falls = cut deepens).
const BODY_DRIVES: [ParamId; 2] = [ParamId::EqGainDb(Band::B0), ParamId::EqGainDb(Band::B1)];

/// Parameters driven by **Presence** (gains and de-essing rise).
const PRESENCE_DRIVES: [ParamId; 4] = [
    ParamId::EqGainDb(Band::B3),
    ParamId::EqGainDb(Band::B4),
    ParamId::DeesserEnabled,
    ParamId::DeesserAmountDb,
];

/// Parameters driven by **Leveler** (threshold falls, ratio rises, auto-makeup on).
const LEVELER_DRIVES: [ParamId; 3] = [ParamId::CompThreshDb, ParamId::CompRatio, ParamId::CompAutoMakeup];

/// All parameters the calibration layer replaces (used for the pin checks).
const CALIBRATION_DRIVES: [ParamId; 5] = [
    ParamId::InputGainDb,
    ParamId::GateThresholdDb,
    ParamId::DenoiseMix,
    ParamId::CompThreshDb,
    ParamId::CompMakeupDb,
];

/// Pure, allocation-free Golden Line → [`DspParams`] mapping (Layer C, D-007).
pub struct MacroMapper;

impl MacroMapper {
    /// Returns the parameters a macro drives, in stable tooltip order.
    #[must_use]
    pub const fn drives(which: Macro) -> &'static [ParamId] {
        match which {
            Macro::Purity => &PURITY_DRIVES,
            Macro::Body => &BODY_DRIVES,
            Macro::Presence => &PRESENCE_DRIVES,
            Macro::Leveler => &LEVELER_DRIVES,
        }
    }

    /// Maps `(preset, calibration, macros, pins)` onto a fresh [`DspParams`].
    ///
    /// Order of composition:
    /// 1. Layer A base curve (`preset.base`);
    /// 2. calibration replaces gain, gate threshold, denoise mix, compressor
    ///    threshold and (as manual makeup, overriding auto-makeup) makeup;
    /// 3. macro offsets are applied around the resulting effective base values;
    /// 4. every driven parameter that is [`PinSet`]-pinned is copied verbatim
    ///    from `current`;
    /// 5. everything is clamped through the `tine-protocol` `SPECS` ranges
    ///    (non-finite inputs collapse to the neutral defaults).
    ///
    /// Deterministic and idempotent for identical inputs.
    #[must_use]
    pub fn apply(
        preset: &PresetDef,
        cal: Option<&Calibration>,
        macros: MacroPositions,
        pins: PinSet,
        current: &DspParams,
    ) -> DspParams {
        let mut out = effective_base(preset, cal);
        let m = macros.clamped();

        // Purity: wet mix and gate threshold move around the effective base.
        let purity = centered(m.purity);
        out.denoise_mix += purity * PURITY_MIX_SPAN * 0.5;
        out.gate_threshold_db += purity * PURITY_GATE_SPAN_DB * 0.5;

        // Body: low-shelf warms up symmetrically; the mud cut only deepens.
        let body = centered(m.body);
        if let Some(gain) = out.eq_gain_db.get_mut(Band::B0.index()) {
            *gain += body * BODY_SHELF_SPAN_DB * 0.5;
        }
        if let Some(gain) = out.eq_gain_db.get_mut(Band::B1.index()) {
            *gain -= upper(m.body) * BODY_MUD_CUT_DB;
        }

        // Presence: peak/air boosts, dynamic de-essing above the engagement point.
        if let Some(gain) = out.eq_gain_db.get_mut(Band::B3.index()) {
            *gain += upper(m.presence) * PRESENCE_PEAK_BOOST_DB;
        }
        if let Some(gain) = out.eq_gain_db.get_mut(Band::B4.index()) {
            *gain += upper(m.presence) * PRESENCE_AIR_BOOST_DB;
        }
        out.deesser_enabled = out.deesser_enabled || m.presence > PRESENCE_DEESSER_POS;
        out.deesser_amount_db = out.deesser_amount_db.max(deesser_amount(m.presence));

        // Leveler: ratio opens up, threshold eases down, auto-makeup on.
        let leveler = centered(m.leveler);
        out.comp_ratio += leveler * LEVELER_RATIO_SPAN * 0.5;
        out.comp_thresh_db -= upper(m.leveler) * LEVELER_THRESH_CUT_DB;
        out.comp_auto_makeup = cal.is_none();

        // D-007: pinned parameters are no longer driven — copy from `current`.
        for id in CALIBRATION_DRIVES {
            if pins.is_pinned(id) {
                out.set(id, current.get(id));
            }
        }
        for which in Macro::ALL {
            for id in Self::drives(which) {
                if pins.is_pinned(*id) {
                    out.set(*id, current.get(*id));
                }
            }
        }

        out.clamped()
    }

    /// Writes the tooltip rows for `which`: the parameters it drives, with the
    /// effective base (`before`), the macro-mapped value (`after`), and a
    /// `pinned` marker when a Pro edit froze the parameter (D-007). Pinned rows
    /// keep their would-be `after` so the UI can say what the macro *would* do.
    pub fn explain(
        preset: &PresetDef,
        cal: Option<&Calibration>,
        macros: MacroPositions,
        pins: PinSet,
        which: Macro,
        out: &mut ExplainBuf,
    ) {
        let base = effective_base(preset, cal);
        let mapped = Self::apply(preset, cal, macros, PinSet::EMPTY, &DspParams::DEFAULT);

        out.clear();
        for id in Self::drives(which) {
            out.push(ExplainRow {
                id: *id,
                before: base.get(*id),
                after: mapped.get(*id),
                pinned: pins.is_pinned(*id),
            });
        }
    }
}

/// Effective Layer A/B base: preset curve with the calibration values
/// (when present) replacing base gain, gate threshold, mix, comp threshold and
/// manual makeup (auto-makeup off, see D-020).
fn effective_base(preset: &PresetDef, cal: Option<&Calibration>) -> DspParams {
    let mut base = preset.base;
    if let Some(c) = cal {
        base.input_gain_db = c.derived.input_gain_db;
        base.gate_threshold_db = c.derived.gate_threshold_db;
        base.denoise_mix = c.derived.denoise_mix;
        base.comp_thresh_db = c.derived.comp_thresh_db;
        base.comp_makeup_db = c.derived.comp_makeup_db;
        base.comp_auto_makeup = false;
    }
    base
}

/// Macro position re-centred on the base: `0.0` at `m = 0` and `+1.0` at `m = 1`.
#[inline]
fn centered(m: f32) -> f32 {
    (m * 2.0) - 1.0
}

/// Upper half of the macro range: `0.0` below the centre, `0.0..=1.0` above.
#[inline]
fn upper(m: f32) -> f32 {
    centered(m).max(0.0)
}

/// De-esser amount curve: `3 → 8 dB` over `presence = 0.6 → 1.0`, flat below.
#[inline]
fn deesser_amount(presence: f32) -> f32 {
    let t = (presence - PRESENCE_DEESSER_POS) / (1.0 - PRESENCE_DEESSER_POS);
    PRESENCE_DEESSER_MIN_DB + t.clamp(0.0, 1.0) * (PRESENCE_DEESSER_MAX_DB - PRESENCE_DEESSER_MIN_DB)
}

// Unit tests for the curve primitives.
#[cfg(test)]
mod tests {
    use super::{PRESENCE_DEESSER_MAX_DB, PRESENCE_DEESSER_MIN_DB, centered, deesser_amount, upper};

    #[test]
    fn centered_is_zero_at_half() {
        assert!((centered(0.5)).abs() < f32::EPSILON);
        assert_eq!(centered(0.0), -1.0);
        assert_eq!(centered(1.0), 1.0);
    }

    #[test]
    fn upper_is_zero_below_half() {
        assert_eq!(upper(0.0), 0.0);
        assert_eq!(upper(0.5), 0.0);
        assert!((upper(0.75) - 0.5).abs() < f32::EPSILON);
        assert_eq!(upper(1.0), 1.0);
    }

    #[test]
    fn deesser_amount_curve_matches_doc() {
        assert_eq!(deesser_amount(0.0), PRESENCE_DEESSER_MIN_DB);
        assert_eq!(deesser_amount(0.6), PRESENCE_DEESSER_MIN_DB);
        assert!((deesser_amount(1.0) - PRESENCE_DEESSER_MAX_DB).abs() < f32::EPSILON);
    }
}
