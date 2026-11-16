//! Stage 3: 5-band parametric EQ.

use tine_protocol::params::{DspParams, EQ_BAND_COUNT, FilterType};

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::biquad::Biquad;
use crate::primitives::smoother::Smoother;

const COEFF_UPDATE_CHUNK: usize = 32;
const RETUNE_EPS: f32 = 1e-4;
const SKIP_GAIN_DB: f32 = 0.01;
const SMOOTH_TAU_MS: f32 = 10.0;
const XFADE_MS: f64 = 10.0;

#[inline]
const fn is_gain_filter(ft: FilterType) -> bool {
    matches!(ft, FilterType::LowShelf | FilterType::HighShelf | FilterType::Peaking)
}

#[inline]
fn sanitize_finite(val: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if val.is_finite() { val.clamp(min, max) } else { fallback }
}

#[derive(Clone, Copy)]
struct BandTarget {
    ft: FilterType,
    freq_hz: f32,
    gain_db: f32,
    q: f32,
}

#[derive(Clone, Copy, Debug)]
struct BandState {
    active_type: FilterType,
    biquad: Biquad,
    old_biquad: Biquad,
    xfade_pos: u32,
    xfade_len: u32,
    freq: Smoother,
    gain_db: Smoother,
    q: Smoother,
}

impl BandState {
    fn new(ft: FilterType, freq: f32, gain_db: f32, q: f32, fs: f32) -> Self {
        let xfade_len = ((XFADE_MS * 1e-3 * f64::from(fs) + 0.5) as u32).max(1);
        Self {
            active_type: ft,
            biquad: Biquad::default(),
            old_biquad: Biquad::default(),
            xfade_pos: xfade_len,
            xfade_len,
            freq: Smoother::new(SMOOTH_TAU_MS, fs, freq),
            gain_db: Smoother::new(SMOOTH_TAU_MS, fs, gain_db),
            q: Smoother::new(SMOOTH_TAU_MS, fs, q),
        }
    }

    fn prepare(&mut self, ft: FilterType, freq: f32, gain_db: f32, q: f32, fs: f32) {
        self.xfade_len = ((XFADE_MS * 1e-3 * f64::from(fs) + 0.5) as u32).max(1);
        self.xfade_pos = self.xfade_len;
        self.active_type = ft;
        self.biquad.reset();
        self.old_biquad.reset();
        self.freq.set_time_constant(SMOOTH_TAU_MS, fs);
        self.gain_db.set_time_constant(SMOOTH_TAU_MS, fs);
        self.q.set_time_constant(SMOOTH_TAU_MS, fs);
        self.freq.snap_to(freq);
        self.gain_db.snap_to(gain_db);
        self.q.snap_to(q);
    }

    fn reset(&mut self) {
        self.biquad.reset();
        self.old_biquad.reset();
        self.xfade_pos = self.xfade_len;
        self.freq.snap();
        self.gain_db.snap();
        self.q.snap();
    }

    fn process_band(&mut self, buf: &mut [f32], fs: f64, target: BandTarget, snap: bool) {
        let BandTarget {
            ft,
            freq_hz: freq_t,
            gain_db: gain_t,
            q: q_t,
        } = target;
        if snap {
            self.active_type = ft;
            self.freq.snap_to(freq_t);
            self.gain_db.snap_to(gain_t);
            self.q.snap_to(q_t);
            self.xfade_pos = self.xfade_len;
            self.biquad.reset();
            self.old_biquad.reset();
        } else {
            self.freq.set_target(freq_t);
            self.gain_db.set_target(gain_t);
            self.q.set_target(q_t);

            if ft != self.active_type {
                self.old_biquad = self.biquad;
                self.active_type = ft;
                self.biquad.reset();
                self.biquad.retune_if_changed(
                    ft,
                    fs,
                    self.freq.current(),
                    self.q.current(),
                    self.gain_db.current(),
                    0.0,
                );
                self.xfade_pos = 0;
            }
        }

        // Fast path: all smoothers settled and no topology cross-fade active.
        if self.xfade_pos >= self.xfade_len
            && self.freq.is_settled()
            && self.gain_db.is_settled()
            && self.q.is_settled()
        {
            if is_gain_filter(self.active_type) && self.gain_db.current().abs() < SKIP_GAIN_DB {
                self.biquad.reset();
                return;
            }
            self.biquad.retune_if_changed(
                self.active_type,
                fs,
                self.freq.current(),
                self.q.current(),
                self.gain_db.current(),
                RETUNE_EPS,
            );
            self.biquad.process_block(buf);
            return;
        }

        for chunk in buf.chunks_mut(COEFF_UPDATE_CHUNK) {
            let n = chunk.len();
            if !self.freq.is_settled() {
                self.freq.skip(n);
            }
            if !self.gain_db.is_settled() {
                self.gain_db.skip(n);
            }
            if !self.q.is_settled() {
                self.q.skip(n);
            }

            if self.xfade_pos >= self.xfade_len
                && is_gain_filter(self.active_type)
                && self.gain_db.is_settled()
                && self.gain_db.current().abs() < SKIP_GAIN_DB
            {
                self.biquad.reset();
                continue;
            }

            self.biquad.retune_if_changed(
                self.active_type,
                fs,
                self.freq.current(),
                self.q.current(),
                self.gain_db.current(),
                RETUNE_EPS,
            );

            if self.xfade_pos < self.xfade_len {
                let inv_len = 1.0 / (self.xfade_len as f32);
                for sample in chunk.iter_mut() {
                    let x = *sample;
                    let old_y = self.old_biquad.process_sample(x);
                    let new_y = self.biquad.process_sample(x);
                    if self.xfade_pos < self.xfade_len {
                        let t = (self.xfade_pos as f32) * inv_len;
                        *sample = old_y + (new_y - old_y) * t;
                        self.xfade_pos += 1;
                    } else {
                        *sample = new_y;
                    }
                }
            } else {
                self.biquad.process_block(chunk);
            }
        }
    }
}

/// Stage 3: 5-band cascaded parametric EQ.
#[derive(Debug)]
pub(crate) struct EqStage {
    fs: f64,
    first_process: bool,
    bands: [BandState; EQ_BAND_COUNT],
}

impl Default for EqStage {
    fn default() -> Self {
        Self::new()
    }
}

impl EqStage {
    pub fn new() -> Self {
        let d = DspParams::DEFAULT;
        let fs = 48_000.0;
        Self {
            fs: f64::from(fs),
            first_process: true,
            bands: [
                BandState::new(d.eq_type[0], d.eq_freq_hz[0], d.eq_gain_db[0], d.eq_q[0], fs),
                BandState::new(d.eq_type[1], d.eq_freq_hz[1], d.eq_gain_db[1], d.eq_q[1], fs),
                BandState::new(d.eq_type[2], d.eq_freq_hz[2], d.eq_gain_db[2], d.eq_q[2], fs),
                BandState::new(d.eq_type[3], d.eq_freq_hz[3], d.eq_gain_db[3], d.eq_q[3], fs),
                BandState::new(d.eq_type[4], d.eq_freq_hz[4], d.eq_gain_db[4], d.eq_q[4], fs),
            ],
        }
    }
}

impl Stage for EqStage {
    fn prepare(&mut self, fs: f32, _max_block: usize) {
        let fs_val = if fs.is_finite() && fs > 0.0 { fs } else { 48_000.0 };
        self.fs = f64::from(fs_val);
        self.first_process = true;
        let d = DspParams::DEFAULT;
        for (i, band) in self.bands.iter_mut().enumerate() {
            let ft = d.eq_type.get(i).copied().unwrap_or(FilterType::Peaking);
            let freq = d.eq_freq_hz.get(i).copied().unwrap_or(1_000.0);
            let gain = d.eq_gain_db.get(i).copied().unwrap_or(0.0);
            let q = d.eq_q.get(i).copied().unwrap_or(1.0);
            band.prepare(ft, freq, gain, q, fs_val);
        }
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, _m: &mut StageMeters) {
        if buf.is_empty() {
            return;
        }
        let snap = self.first_process;
        let fs = self.fs;
        for (band, (((&ft, &freq), &gain), &q)) in self.bands.iter_mut().zip(
            p.eq_type
                .iter()
                .zip(p.eq_freq_hz.iter())
                .zip(p.eq_gain_db.iter())
                .zip(p.eq_q.iter()),
        ) {
            let target = BandTarget {
                ft,
                freq_hz: sanitize_finite(freq, 20.0, 20_000.0, 1_000.0),
                gain_db: sanitize_finite(gain, -18.0, 18.0, 0.0),
                q: sanitize_finite(q, 0.1, 10.0, 1.0),
            };
            band.process_band(buf, fs, target, snap);
        }
        self.first_process = false;
    }

    fn latency(&self) -> u32 {
        0
    }
}
