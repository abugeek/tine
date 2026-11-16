//! Fixed power-of-two ring delay line (allocated once in `new`).

use alloc::boxed::Box;
use alloc::vec;

/// Fixed-capacity power-of-two ring delay line.
#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Box<[f32]>,
    mask: usize,
    write_pos: usize,
}

impl DelayLine {
    /// Allocates a delay line capable of holding at least `max_delay_samples` samples.
    #[must_use]
    pub fn new(max_delay_samples: usize) -> Self {
        let cap = (max_delay_samples + 1).next_power_of_two().max(2);
        Self {
            buf: vec![0.0; cap].into_boxed_slice(),
            mask: cap - 1,
            write_pos: 0,
        }
    }

    /// Returns the maximum supported delay in samples.
    #[inline]
    #[must_use]
    pub const fn max_delay(&self) -> usize {
        self.mask
    }

    /// Clears the delay buffer to zero and resets the write cursor.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
    }

    /// Pushes `x` and reads the sample delayed by `delay_samples` (clamped to `max_delay()`).
    #[inline]
    pub fn push_read(&mut self, x: f32, delay_samples: usize) -> f32 {
        let d = delay_samples.min(self.mask);
        if let Some(slot) = self.buf.get_mut(self.write_pos & self.mask) {
            *slot = x;
        }
        let read_idx = self.write_pos.wrapping_sub(d) & self.mask;
        let out = self.buf.get(read_idx).copied().unwrap_or(0.0);
        self.write_pos = self.write_pos.wrapping_add(1) & self.mask;
        out
    }

    /// Delays `buf` in-place by `delay_samples`.
    pub fn process_block(&mut self, buf: &mut [f32], delay_samples: usize) {
        if delay_samples == 0 {
            for sample in buf.iter() {
                let _ = self.push_read(*sample, 0);
            }
            return;
        }
        for sample in buf.iter_mut() {
            *sample = self.push_read(*sample, delay_samples);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_by_exact_samples() {
        let mut d = DelayLine::new(10);
        let input = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut out = [0.0; 6];
        for (i, &x) in input.iter().enumerate() {
            out[i] = d.push_read(x, 3);
        }
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn zero_delay_is_identity() {
        let mut d = DelayLine::new(8);
        let mut buf = [1.0, -2.0, 3.5, 0.25];
        let orig = buf;
        d.process_block(&mut buf, 0);
        assert_eq!(buf, orig);
    }
}
