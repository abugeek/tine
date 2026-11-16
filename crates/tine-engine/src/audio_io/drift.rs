//! Lock-free, allocation-free local jitter buffer and adaptive cubic resampler
//! for absorbing sample-rate differences and independent device clock drift.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

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
        let default_target = ((960.0 * base_ratio) as usize).clamp(128, ring.capacity() / 2);
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

        // Target fill = 2 * callback size (scaled to source sample rate), bounded safely.
        let desired_target =
            ((2.0 * (out.len() as f64) * self.base_ratio) as usize).clamp(128, self.ring.capacity() / 2);
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
