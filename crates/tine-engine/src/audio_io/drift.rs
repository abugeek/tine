//! Lock-free, allocation-free local jitter buffer and adaptive cubic resampler
//! for absorbing sample-rate differences and independent device clock drift.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

/// Default jitter-buffer headroom: how many playback callbacks the adaptive
/// resampler centers its local buffer at (in source samples, scaled by `base_ratio`).
///
/// `2.0` callbacks (≈ 20 ms at the ~10 ms WASAPI shared-mode period) is the
/// low-latency default for `VirtualCableRouter`; `Monitor` can configure `3.0`
/// callbacks (≈ 30 ms) via [`AdaptiveResampler::with_headroom`].
pub(crate) const HEADROOM_CALLBACKS: f64 = 2.0;

/// Start-up priming depth as a multiple of the steady target (T24).
///
/// Playback starts only once the local buffer holds this many targets. When
/// the sink already has an active reader (an app recording `CABLE Output`),
/// the shared engine's first render callbacks drain the buffer in a burst; a
/// 1x prime starved once in ~1 of 5 such starts (measured). The surplus is
/// drained by a fixed start-up drain ([`STARTUP_DRAIN_RATE`]), not by the
/// drift loop, so steady-state latency and control behaviour are unchanged.
pub(crate) const STARTUP_PRIME_FACTOR: usize = 2;

/// Playback-speed surplus used to drain the start-up prime back to the steady
/// target: +0.5 % (~0.09 semitone, inaudible) drains one extra target in ~4 s.
///
/// The drift PI loop must not do this: its +-0.1 % clamp needs ~20 s, during
/// which the integral winds up and then drives the fill ~45 % below target
/// for tens of seconds; with scheduler jitter that measured as 164 underruns
/// between 20 s and 50 s after start (T24). The loop is frozen while draining
/// and starts from zeroed state exactly at the target.
pub(crate) const STARTUP_DRAIN_RATE: f64 = 0.005;

// +0.5 % playback speed is ~0.09 semitone; keep the drain inaudibly small.
const _: () = assert!(STARTUP_DRAIN_RATE > 0.0 && STARTUP_DRAIN_RATE <= 0.005);

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
    headroom_callbacks: f64,
    frac_pos: f64,
    err_filtered: f64,
    err_integral: f64,
    primed: bool,
    /// `true` from the (doubled) first prime until the fill is back at target.
    draining_startup: bool,
    target_fill: usize,
    last_ratio: f64,
}

impl AdaptiveResampler {
    /// Creates a new [`AdaptiveResampler`] with the default [`HEADROOM_CALLBACKS`]
    /// preallocated for `ring_capacity` samples.
    #[allow(dead_code)]
    pub(crate) fn new(src_rate: u32, dst_rate: u32, ring_capacity: usize) -> Self {
        Self::with_headroom(src_rate, dst_rate, ring_capacity, HEADROOM_CALLBACKS)
    }

    /// Creates a new [`AdaptiveResampler`] with a custom `headroom_callbacks`
    /// target (clamped to `1.0..=12.0`), preallocated for `ring_capacity` samples.
    pub(crate) fn with_headroom(src_rate: u32, dst_rate: u32, ring_capacity: usize, headroom_callbacks: f64) -> Self {
        let s_rate = src_rate.max(1);
        let d_rate = dst_rate.max(1);
        let base_ratio = f64::from(s_rate) / f64::from(d_rate);
        let headroom = headroom_callbacks.clamp(1.0, 12.0);
        let ring = LocalRing::new(ring_capacity);
        let default_target =
            ((headroom * NOMINAL_CALLBACK_FRAMES * base_ratio) as usize).clamp(128, ring.capacity() / 2);
        Self {
            ring,
            src_rate: s_rate,
            dst_rate: d_rate,
            base_ratio,
            headroom_callbacks: headroom,
            frac_pos: 0.0,
            err_filtered: 0.0,
            err_integral: 0.0,
            primed: false,
            draining_startup: false,
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
    pub(crate) const fn is_primed(&self) -> bool {
        self.primed
    }

    #[inline]
    pub(crate) const fn last_ratio(&self) -> f64 {
        self.last_ratio
    }

    /// Computes the target fill level in source samples for an output block of `out_frames`.
    #[inline]
    #[allow(clippy::arithmetic_side_effects)]
    pub(crate) fn desired_target_for(&self, out_frames: usize) -> usize {
        let frames = if out_frames > 0 {
            out_frames as f64
        } else {
            NOMINAL_CALLBACK_FRAMES
        };
        ((self.headroom_callbacks * frames * self.base_ratio) as usize).clamp(128, self.ring.capacity() / 2)
    }

    /// Fill level that must be reached before playback starts: the steady
    /// target, times [`STARTUP_PRIME_FACTOR`] until the first prime.
    #[inline]
    pub(crate) fn priming_target_for(&self, out_frames: usize) -> usize {
        let target = self.desired_target_for(out_frames);
        if self.primed {
            target
        } else {
            target
                .saturating_mul(STARTUP_PRIME_FACTOR)
                .min(self.ring.capacity().saturating_mul(3) / 4)
        }
    }

    /// Drops oldest queued samples so the ring fill level does not exceed `target`.
    #[inline]
    pub(crate) fn trim_to_target(&mut self, target: usize) {
        if self.ring.len() > target {
            self.ring.advance(self.ring.len().saturating_sub(target));
        }
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

        // Target fill = headroom_callbacks × callback size (scaled to source
        // sample rate), bounded safely inside the ring.
        self.target_fill = self.desired_target_for(out.len());

        if !self.primed {
            if self.ring.len() >= self.priming_target_for(out.len()) {
                self.primed = true;
                self.draining_startup = STARTUP_PRIME_FACTOR > 1;
            } else {
                out.fill(0.0);
                return false;
            }
        }

        // Hard-resync if fill diverges from target by more than 2× target_fill,
        // so large startup/scheduler spikes recover immediately instead of
        // taking minutes at ±0.1%.
        if self.ring.len().abs_diff(self.target_fill) > self.target_fill.saturating_mul(2)
            && self.ring.len() > self.target_fill
        {
            self.ring.advance(self.ring.len().saturating_sub(self.target_fill));
            self.draining_startup = false;
            self.err_filtered = 0.0;
            self.err_integral = 0.0;
            self.frac_pos = 0.0;
        }

        // Start-up drain: consume the doubled prime at a fixed +0.5 % with the
        // PI loop frozen, then hand over from exactly the target with zeroed
        // controller state (no integral windup, no undershoot).
        if self.draining_startup && self.ring.len() <= self.target_fill {
            self.draining_startup = false;
            self.err_filtered = 0.0;
            self.err_integral = 0.0;
        }
        let drift_adj = if self.draining_startup {
            STARTUP_DRAIN_RATE
        } else {
            // Smooth fill-level error (PI controller) and clamp drift adjustment to ±0.1 % (±0.001).
            let fill = self.ring.len() as f64;
            let target = (self.target_fill as f64).max(1.0);
            let err = (fill - target) / target;
            self.err_filtered += 0.02 * (err - self.err_filtered);
            self.err_integral = (self.err_integral + err * 0.002).clamp(-1.0, 1.0);
            (self.err_filtered * 0.001 + self.err_integral * 0.0005).clamp(-0.001, 0.001)
        };
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

    use super::{AdaptiveResampler, HEADROOM_CALLBACKS, LocalRing, STARTUP_PRIME_FACTOR};

    fn resampler_48k() -> AdaptiveResampler {
        AdaptiveResampler::new(48_000, 48_000, 16_384)
    }

    /// The default headroom is 2.0 callbacks (~20 ms at 48 kHz).
    #[test]
    fn headroom_is_in_the_approved_band() {
        assert!(
            (HEADROOM_CALLBACKS - 2.0).abs() < f64::EPSILON,
            "default headroom should be 2.0 callbacks: {HEADROOM_CALLBACKS}"
        );
    }

    /// Priming waits for `STARTUP_PRIME_FACTOR × HEADROOM_CALLBACKS × callback`
    /// of input; the silence played while warming up must NOT be reported as
    /// an underrun.
    #[test]
    fn priming_waits_for_headroom_and_is_not_an_underrun() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];
        let prime_callbacks = 2 * STARTUP_PRIME_FACTOR; // default 2-callback target, doubled

        // Short of the start-up prime: silent, nothing consumed, no underrun.
        for n in 1..prime_callbacks {
            assert_eq!(r.push_input(&block), 480);
            let underrun = r.render(&mut out);
            assert!(!underrun, "priming silence must not count as an underrun");
            assert!(out.iter().all(|s| *s == 0.0), "pre-prime render must be silent");
            assert_eq!(r.fill_level(), 480 * n, "pre-prime render consumes nothing");
        }

        // The last callback reaches the start-up prime; audio flows.
        assert_eq!(r.push_input(&block), 480);
        assert!(!r.render(&mut out));
        assert!(
            out.iter().any(|s| *s > 0.1),
            "primed render must emit the buffered signal"
        );
        // The first primed render already runs the +0.5 % start-up drain.
        let expected = 480 * (prime_callbacks - 1);
        assert!(
            (expected - 4..=expected).contains(&r.fill_level()),
            "fill {} vs {expected}",
            r.fill_level()
        );
    }

    /// T24: right after priming, a sink that already has an active reader
    /// pulls a burst of callbacks with no new input. The doubled start-up
    /// prime must absorb a 2-callback burst without starving.
    #[test]
    fn startup_burst_after_priming_does_not_underrun() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];
        for _ in 0..2 * STARTUP_PRIME_FACTOR {
            assert_eq!(r.push_input(&block), 480);
        }
        assert!(!r.render(&mut out), "priming render");
        assert!(!r.render(&mut out), "burst callback 1 must not starve");
        assert!(!r.render(&mut out), "burst callback 2 must not starve");
        assert!(out.iter().all(|s| *s > 0.1), "burst must play buffered audio");
    }

    /// The steady-state fill with `with_headroom(6.0)` absorbs a 4-callback
    /// (≈ 40 ms) capture hiccup with no underrun, while a gap that genuinely
    /// drains the buffer is still reported.
    #[test]
    fn hiccup_absorption_and_honest_underrun_semantics() {
        let mut r = AdaptiveResampler::with_headroom(48_000, 48_000, 16_384, 6.0);
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];

        // Prime: the doubled start-up prime of a 6-callback target.
        for _ in 0..6 * STARTUP_PRIME_FACTOR {
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
        let mut starved = false;
        for _ in 0..64 {
            if r.render(&mut out) {
                starved = true;
                break;
            }
        }
        assert!(starved, "starvation must still be flagged");
        assert!(out.contains(&0.0), "starved render must contain silence");
    }

    /// T24 regression: after the doubled prime, the fill must settle at the
    /// steady target without undershooting it (the drift loop alone wound up
    /// and fell ~45 % below target 20-50 s after start, causing underruns).
    #[test]
    fn startup_surplus_drains_without_windup_or_undershoot() {
        let mut r = resampler_48k();
        let block = [0.5_f32; 480];
        let mut out = [0.0_f32; 480];
        for _ in 0..2 * STARTUP_PRIME_FACTOR {
            assert_eq!(r.push_input(&block), 480);
        }
        assert!(!r.render(&mut out));
        let mut drained_at = None;
        let mut min_post_render = usize::MAX;
        for n in 0..9_000 {
            // 90 s of matched-rate input
            assert_eq!(r.push_input(&block), 480);
            assert!(!r.render(&mut out), "no underrun at callback {n}");
            if drained_at.is_none() && !r.draining_startup {
                drained_at = Some(n);
            }
            if drained_at.is_some() {
                min_post_render = min_post_render.min(r.fill_level());
            }
        }
        let drained_at = drained_at.unwrap_or(usize::MAX);
        assert!(
            drained_at <= 500,
            "surplus must drain within ~5 s, took {drained_at} callbacks"
        );
        // Steady post-render fill is target - one callback = 480; allow a few
        // samples of interpolation phase, never the old ~200-sample undershoot.
        assert!(
            min_post_render >= 470,
            "fill undershot after the drain: {min_post_render}"
        );
        assert!((r.last_ratio() - 1.0).abs() <= 1.001e-3, "back inside the ±0.1 % loop");
    }

    /// Moderate playback-side overfill (within the hard-resync threshold) is
    /// absorbed by the ±0.1 % drift clamp, never by dropping samples.
    #[test]
    fn drift_ratio_stays_within_the_plus_minus_one_permille_clamp() {
        let mut r = resampler_48k();
        let block = [0.25_f32; 480];
        let mut out = [0.0_f32; 480];

        // Overfill moderately (4 callbacks = 1920 < 3 * 960 = 2880 hard-resync threshold).
        for _ in 0..4 {
            assert_eq!(r.push_input(&block), 480);
        }
        // Let the T24 start-up drain finish first (it runs outside the clamp;
        // this test starts with 2400 samples, more than the trimmed 1920),
        // then overfill moderately again so the drift loop has work to do.
        for _ in 0..1_200 {
            assert_eq!(r.push_input(&block), 480);
            assert!(!r.render(&mut out));
        }
        for _ in 0..2 {
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

    /// Large overfill (`|fill - target_fill| > target_fill * 2`) triggers an
    /// immediate hard-resync back down to `target_fill`.
    #[test]
    fn hard_resync_drops_excess_when_error_exceeds_twice_target() {
        let mut r = resampler_48k();
        let block = [0.25_f32; 480];
        let mut out = [0.0_f32; 480];

        // Prime normally at 2 callbacks (960).
        for _ in 0..2 {
            assert_eq!(r.push_input(&block), 480);
        }
        assert!(!r.render(&mut out));

        // Inject a huge spike (10 callbacks = 4800 > 3 * 960 = 2880).
        for _ in 0..10 {
            assert_eq!(r.push_input(&block), 480);
        }
        assert!(!r.render(&mut out));
        // After hard-resync to target_fill (960) and rendering 480 samples, fill is ~480.
        assert!(
            r.fill_level() <= 960,
            "hard resync should snap fill back near target_fill, got {}",
            r.fill_level()
        );
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
