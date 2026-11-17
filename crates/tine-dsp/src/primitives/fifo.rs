//! Fixed-capacity ring FIFO for framing audio into fixed-size chunks (e.g. 480-sample `RNNoise` frames).

use alloc::boxed::Box;
use alloc::vec;

/// Fixed-capacity ring FIFO of `f32` samples (zero allocations after `new`).
#[derive(Clone, Debug)]
pub struct FrameFifo {
    buf: Box<[f32]>,
    mask: usize,
    read_pos: usize,
    write_pos: usize,
    len: usize,
    frame_len: usize,
}

impl FrameFifo {
    /// Allocates a FIFO sized for `frame_len` frames and blocks up to `max_block`.
    #[must_use]
    pub fn new(frame_len: usize, max_block: usize) -> Self {
        let min_cap = (frame_len + max_block).max(frame_len * 2).max(2);
        let cap = min_cap.next_power_of_two();
        Self {
            buf: vec![0.0; cap].into_boxed_slice(),
            mask: cap - 1,
            read_pos: 0,
            write_pos: 0,
            len: 0,
            frame_len: frame_len.max(1),
        }
    }

    /// Returns the configured frame length in samples.
    #[inline]
    #[must_use]
    pub const fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Returns the number of buffered samples available to read.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if no samples are currently buffered.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `true` if at least one full `frame_len()` frame is available.
    #[inline]
    #[must_use]
    pub const fn can_pop_frame(&self) -> bool {
        self.len >= self.frame_len
    }

    /// Clears all buffered samples.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.read_pos = 0;
        self.write_pos = 0;
        self.len = 0;
    }

    /// Pre-fills the FIFO with `count` zero samples (e.g. to prime output latency).
    pub fn push_zeros(&mut self, count: usize) {
        let cap = self.mask + 1;
        let to_push = count.min(cap.saturating_sub(self.len));
        if to_push == 0 {
            return;
        }
        let first = to_push.min(cap - self.write_pos);
        let second = to_push - first;
        if let Some(dst) = self.buf.get_mut(self.write_pos..self.write_pos + first) {
            dst.fill(0.0);
        }
        if second > 0
            && let Some(dst) = self.buf.get_mut(..second)
        {
            dst.fill(0.0);
        }
        self.write_pos = self.write_pos.wrapping_add(to_push) & self.mask;
        self.len += to_push;
    }

    /// Pushes `samples` into the FIFO, returning the number of samples actually pushed
    /// (never exceeds remaining capacity).
    pub fn push_slice(&mut self, samples: &[f32]) -> usize {
        let cap = self.mask + 1;
        let free = cap.saturating_sub(self.len);
        let to_push = samples.len().min(free);
        if to_push == 0 {
            return 0;
        }

        let first = to_push.min(cap - self.write_pos);
        let second = to_push - first;

        if let (Some(dst), Some(src)) = (
            self.buf.get_mut(self.write_pos..self.write_pos + first),
            samples.get(..first),
        ) {
            dst.copy_from_slice(src);
        }
        if second > 0
            && let (Some(dst), Some(src)) = (self.buf.get_mut(..second), samples.get(first..to_push))
        {
            dst.copy_from_slice(src);
        }

        self.write_pos = self.write_pos.wrapping_add(to_push) & self.mask;
        self.len += to_push;
        to_push
    }

    /// Pops `dst.len()` samples into `dst`, returning the number of samples popped.
    pub fn pop_slice(&mut self, dst: &mut [f32]) -> usize {
        let cap = self.mask + 1;
        let to_pop = dst.len().min(self.len);
        if to_pop == 0 {
            return 0;
        }

        let first = to_pop.min(cap - self.read_pos);
        let second = to_pop - first;

        if let (Some(out), Some(src)) = (dst.get_mut(..first), self.buf.get(self.read_pos..self.read_pos + first)) {
            out.copy_from_slice(src);
        }
        if second > 0
            && let (Some(out), Some(src)) = (dst.get_mut(first..to_pop), self.buf.get(..second))
        {
            out.copy_from_slice(src);
        }

        self.read_pos = self.read_pos.wrapping_add(to_pop) & self.mask;
        self.len -= to_pop;
        to_pop
    }

    /// Pops a single `frame_len()` frame into `out` if enough samples are buffered.
    ///
    /// Returns `true` if a full frame was popped, or `false` if `out.len() < self.frame_len()`
    /// or fewer than `frame_len()` samples are available.
    pub fn pop_frame(&mut self, out: &mut [f32]) -> bool {
        if !self.can_pop_frame() || out.len() < self.frame_len {
            return false;
        }
        if let Some(frame_out) = out.get_mut(..self.frame_len) {
            self.pop_slice(frame_out);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_480_samples_across_arbitrary_blocks() {
        let mut fifo = FrameFifo::new(480, 512);
        let input: alloc::vec::Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let mut frame = [0.0f32; 480];
        let mut popped = alloc::vec::Vec::new();

        for chunk in input.chunks(137) {
            let pushed = fifo.push_slice(chunk);
            assert_eq!(pushed, chunk.len());
            while fifo.pop_frame(&mut frame) {
                popped.extend_from_slice(&frame);
            }
        }
        assert_eq!(popped.len(), 960);
        assert_eq!(fifo.len(), 40);
        for (i, &v) in popped.iter().enumerate() {
            assert_eq!(v, i as f32);
        }
    }
}
