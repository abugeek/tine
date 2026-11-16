//! Lock-free, allocation-free local jitter buffer and adaptive cubic resampler
//! for absorbing sample-rate differences and independent device clock drift.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

/// Jitter-buffer headroom: how many playback callbacks the adaptive resampler
/// centers its local buffer at (in source samples, scaled by `base_ratio`).
///
/// T16's 30-min monitor soak measured that the old 2-callback (≈ 20 ms)
/// center admits rare WASAPI capture hiccups as audible underruns (2 in
/// 30 min on a quiet machine, max callback 283 µs; the RT work was fine —
/// the scheduler simply delivered capture late; see T16 `## Requests`).
/// 6 callbacks (≈ 60 ms at the ~10 ms WASAPI shared-mode period) absorbs
/// hiccups up to ~5 callbacks deep without starving the Catmull-Rom reader.
/// The trade is ~40 ms of extra buffered monitor latency, on top of the
/// latency badge (badge accuracy is tracked as R-18).
pub(crate) const HEADROOM_CALLBACKS: f64 = 6.0;

/// Nominal WASAPI shared-mode callback size in frames at 48 kHz, used for the
/// pre-`render` center estimate (each `render` recomputes the target from the
/// actual `out.len()`).
const NOMINAL_CALLBACK_FRAMES: f64 = 480.0;

/// Fixed-capacity, power-of-two mono `f32` ring buffer owned by the playback RT thread.
pub(crate) struct LocalRing {
    buf: Box<[f32]>,
    mask: usize,
    read: usize,
    write: usize,
    len: usize,
}

impl LocalRing {
    /// Allocates a new [`LocalRing`] with capacity rounded up to a power of two (minimum 64).
    pub(crate) fn new(min_capacity: usize) -> Self {
        let cap = min_capacity.max(64).next_power_of_two();
        Self {
            buf: vec![0.0; cap].into_boxed_slice(),
            mask: cap.saturating_sub(1),
            read: 0,
            write: 0,
            len: 0,
        }
    }

    #[inline]
    pub(crate) const fn capacity(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) const fn free(&self) -> usize {
        self.buf.len().saturating_sub(self.len)
    }

    /// Pushes as many samples from `src` as fit; returns the number written.
    #[inline]
    pub(crate) fn push_slice(&mut self, src: &[f32]) -> usize {
        let count = src.len().min(self.free());
        for &s in src.iter().take(count) {
            if let Some(slot) = self.buf.get_mut(self.write & self.mask) {
                *slot = s;
            }
            self.write = self.write.wrapping_add(1);
        }
        self.len = self.len.saturating_add(count);
        count
    }

    /// Peeks the sample at `offset` frames ahead of the current read cursor.
    #[inline]
    fn peek(&self, offset: usize) -> f32 {
        if offset >= self.len {
            return 0.0;
        }
        let idx = self.read.wrapping_add(offset) & self.mask;
        self.buf.get(idx).copied().unwrap_or(0.0)
    }

    /// Advances the read cursor by `count` samples (clamped to `self.len`).
    #[inline]
    fn advance(&mut self, count: usize) {
        let n = count.min(self.len);
        self.read = self.read.wrapping_add(n);
        self.len = self.len.saturating_sub(n);
    }
}

/// Adaptive cubic (Catmull-Rom) resampler that tracks ring fill level and adjusts
/// the resampling ratio by up to `±0.1%` to absorb clock drift between independent devices.
pub(crate) struct AdaptiveResampler {
    ring: LocalRing,
    src_rate: u32,
    dst_rate: u32,
    base_ratio: f64,
    frac_pos: f64,
    err_filtered: f64,
    primed: bool,
    target_fill: usize,
    last_ratio: f64,
}

impl AdaptiveResampler {
    /// Creates a new [`AdaptiveResampler`] preallocated for `ring_capacity` samples.
    pub(crate) fn new(src_rate: u32, dst_rate: u32, ring_capacity: usize) -> Self {
        let s_rate = src_rate.max(1);
        let d_rate = dst_rate.max(1);
        let base_ratio = f64::from(s_rate) / f64::from(d_rate);
        let ring = LocalRing::new(ring_capacity);
        let default_target =
            ((HEADROOM_CALLBACKS * NOMINAL_CALLBACK_FRAMES * base_ratio) as usize).clamp(128, ring.capacity() / 2);
        Self {
            ring,
            src_rate: s_rate,
            dst_rate: d_rate,
            base_ratio,
            frac_pos: 0.0,
            err_filtered: 0.0,
            primed: false,
            target_fill: default_target,
            last_ratio: base_ratio,
        }
    }

    #[inline]
    pub(crate) const fn free(&self) -> usize {
        self.ring.free()
    }

    #[inline]
    pub(crate) const fn fill_level(&self) -> usize {
        self.ring.len()
    }

    #[inline]
    pub(crate) const fn last_ratio(&self) -> f64 {
        self.last_ratio
    }

    #[inline]
    pub(crate) fn push_input(&mut self, samples: &[f32]) -> usize {
        self.ring.push_slice(samples)
    }

    /// Renders `out.len()` mono samples at `dst_rate` from the internal jitter buffer.
    ///
    /// Returns `true` if an underrun occurred (after initial priming).
    #[allow(clippy::arithmetic_side_effects)] // Floating-point resampling & PI control math.
    pub(crate) fn render(&mut self, out: &mut [f32]) -> bool {
        if out.is_empty() {
            return false;
        }

        // Target fill = HEADROOM_CALLBACKS × callback size (scaled to source
        // sample rate), bounded safely inside the ring.
        let desired_target =
            ((HEADROOM_CALLBACKS * (out.len() as f64) * self.base_ratio) as usize).clamp(128, self.ring.capacity() / 2);
        self.target_fill = desired_target;

        if !self.primed {
            if self.ring.len() >= self.target_fill {
                self.primed = true;
            } else {
                out.fill(0.0);
                return false;
            }
        }

        // Smooth fill-level error and clamp drift adjustment to ±0.1 % (±0.001).
        let fill = self.ring.len() as f64;
        let target = (self.target_fill as f64).max(1.0);
        let err = (fill - target) / target;
        self.err_filtered += 0.02 * (err - self.err_filtered);
        let drift_adj = (self.err_filtered * 0.001).clamp(-0.001, 0.001);
        let step = self.base_ratio * (1.0 + drift_adj);
        self.last_ratio = step;

        let mut underrun = false;
        for dst in out.iter_mut() {
            // Catmull-Rom needs 4 samples: [0, 1, 2, 3], interpolating between 1 and 2.
            // When frac_pos == 0.0 we only strictly need index 1, so require >= 4 samples
            // for clean cubic interpolation (and fall back to silence on starvation).
            if self.ring.len() < 4 {
                *dst = 0.0;
                underrun = true;
                continue;
            }

            let y0 = f64::from(self.ring.peek(0));
            let y1 = f64::from(self.ring.peek(1));
            let y2 = f64::from(self.ring.peek(2));
            let y3 = f64::from(self.ring.peek(3));

            let t = self.frac_pos;
            let t2 = t * t;
            let t3 = t2 * t;

            // 4-point Catmull-Rom spline (exact at t == 0 -> y1, t == 1 -> y2).
            let c0 = y1;
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            let sample = c0 + c1 * t + c2 * t2 + c3 * t3;

            *dst = (sample as f32).clamp(-1.0, 1.0);

            self.frac_pos += step;
            if self.frac_pos >= 1.0 {
                let whole = self.frac_pos as usize;
                self.frac_pos -= whole as f64;
                self.ring.advance(whole);
            }
        }

        let _ = (self.src_rate, self.dst_rate);
        underrun
    }
}

#[cfg(test)]
mod tests {
    //! Jitter-buffer headroom and honest underrun/overrun semantics.
    //!
    //! T16's monitor soak fixed the headroom contract: the buffer must absorb
    //! rare multi-callback WASAPI capture hiccups (target: 0 underruns in a
    //! 30-min soak), while priming silence and *real* starvation keep their
    //! distinct, honest outcomes.

    use super::{AdaptiveResampler, HEADROOM_CALLBACKS, LocalRing};

    fn resampler_48k() -> AdaptiveResampler {
        AdaptiveResampler::new(48_000, 48_000, 16_384)
    }

    /// The headroom stays inside the orchestrator-approved 4–6 callback band.
    #[test]
    fn headroom_is_in_the_approved_band() {
        assert!(
            (4.0..=6.0).contains(&HEADROOM_CALLBACKS),
            "headroom out of the approved 4-6 callbacks: {HEADROOM_CALLBACKS}"
        );
    }

    /// Priming waits for `HEADROOM_CALLBACKS × callback` of input; the silence
    /// played while warming up must NOT be reported as an underrun.
    #[test]
    fn priming_waits_for_headroom_and_is_not_an_underrun() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];

        // 5 callbacks of input: short of the (default 6-callback) target.
        for _ in 0..5 {
            assert_eq!(r.push_input(&block), 480);
        }
        let underrun = r.render(&mut out);
        assert!(!underrun, "priming silence must not count as an underrun");
        assert!(out.iter().all(|s| *s == 0.0), "pre-prime render must be silent");
        assert_eq!(r.fill_level(), 2_400, "pre-prime render consumes nothing");

        // One more callback homes the buffer; audio flows.
        assert_eq!(r.push_input(&block), 480);
        assert!(!r.render(&mut out));
        assert!(
            out.iter().any(|s| *s > 0.1),
            "primed render must emit the buffered signal"
        );
        assert_eq!(r.fill_level(), 2_400);
    }

    /// The steady-state fill is deep enough that a 4-callback (≈ 40 ms)
    /// capture hiccup is absorbed with no underrun and no dropped audio,
    /// while a hiccup that genuinely drains the buffer is still reported.
    #[test]
    fn hiccup_absorption_and_honest_underrun_semantics() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];

        // Prime: 6 callbacks worth of input before the first render.
        for _ in 0..6 {
            assert_eq!(r.push_input(&block), 480);
        }
        assert!(!r.render(&mut out));

        // Steady state: feed one callback per render (drift settles).
        for _ in 0..10 {
            assert_eq!(r.push_input(&block), 480);
            assert!(!r.render(&mut out));
        }

        // A 4-callback hiccup: playback keeps rendering real audio.
        for _ in 0..4 {
            let underrun = r.render(&mut out);
            assert!(!underrun, "a 40 ms hiccup must be absorbed by the headroom");
            assert!(
                out.iter().all(|s| *s > 0.1),
                "hiccup window must still play buffered audio, not silence"
            );
        }

        // A gap that genuinely drains the buffer IS reported: no masking.
        assert!(r.render(&mut out), "starvation must still be flagged");
        assert!(out.contains(&0.0), "starved render must contain silence");
    }

    /// Playback-side overfill is absorbed by the ±0.1 % drift clamp, never
    /// by dropping samples on the floor.
    #[test]
    fn drift_ratio_stays_within_the_plus_minus_one_permille_clamp() {
        let mut r = resampler_48k();
        let block = [0.25_f32; 480];
        let mut out = [0.0_f32; 480];

        // Overfill hard (10 callbacks at once), then keep feeding.
        for _ in 0..10 {
            assert_eq!(r.push_input(&block), 480);
        }
        for _ in 0..200 {
            assert_eq!(r.push_input(&block), 480);
            assert!(!r.render(&mut out));
        }
        let step = r.last_ratio();
        assert!((step - 1.0).abs() <= 1.001e-3, "ratio must stay within ±0.1 %: {step}");
        assert!(step > 1.0, "overfilled buffer must drain faster than 1.0");
    }

    /// No allocation after construction: the RT paths (`push_input`, `render`)
    /// stay heap-free (construction may allocate — it runs off-thread).
    ///
    /// The counting allocator for this test binary is `AllocDisabler`,
    /// installed once per crate by `spectrum.rs`'s allocation test — a
    /// second `#[global_allocator]` would not compile. Negative-controlled:
    /// a deliberate `Vec` inside the region fails the test.
    #[test]
    fn push_and_render_are_allocation_free() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];

        // Prime outside the measured region, then 100 callbacks (1 s) pure RT.
        for _ in 0..7 {
            assert_eq!(r.push_input(&block), 480);
        }
        for _ in 0..100 {
            assert_no_alloc::assert_no_alloc(|| {
                assert_eq!(r.push_input(&block), 480);
                let _ = r.render(&mut out);
            });
        }
    }

    /// The power-of-two ring: capacity rounding, saturation on overflow, and
    /// the peek/advance invariants that hold the RT reader's bounds.
    #[test]
    fn local_ring_capacity_saturates_and_round_trips() {
        let mut ring = LocalRing::new(0);
        assert_eq!(ring.capacity(), 64, "minimum capacity is 64");

        let src = [0.25_f32; 100];
        assert_eq!(ring.push_slice(&src), 64, "writes saturate at capacity");
        assert_eq!(ring.len(), 64);
        assert_eq!(ring.free(), 0);
        assert_eq!(ring.peek(0), 0.25);

        // Advancing frees space; pushes wrap around the power-of-two mask.
        ring.advance(60);
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.push_slice(&src), 60);
        assert_eq!(ring.len(), 64);

        // Peek past the valid window is silence, and over-advance clamps.
        assert_eq!(ring.peek(100), 0.0);
        ring.advance(1_000);
        assert_eq!(ring.len(), 0, "advance clamps to the stored length");
    }
}
