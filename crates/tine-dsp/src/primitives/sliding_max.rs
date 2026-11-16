//! Monotone deque sliding-window maximum in O(1) amortized time per sample.

use alloc::boxed::Box;
use alloc::vec;

/// Fixed-capacity monotone deque computing the maximum over a sliding window.
#[derive(Clone, Debug)]
pub struct SlidingMax {
    ring: Box<[(u64, f32)]>,
    mask: usize,
    head: usize,
    tail: usize,
    len: usize,
    index: u64,
    max_window: usize,
}

impl SlidingMax {
    /// Allocates a [`SlidingMax`] supporting windows up to `max_window` samples.
    #[must_use]
    pub fn new(max_window: usize) -> Self {
        let w = max_window.max(1);
        let cap = (w + 1).next_power_of_two();
        Self {
            ring: vec![(0, 0.0); cap].into_boxed_slice(),
            mask: cap - 1,
            head: 0,
            tail: 0,
            len: 0,
            index: 0,
            max_window: w,
        }
    }

    /// Resets the deque state.
    pub fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
        self.index = 0;
    }

    /// Pushes `val` and returns the maximum over the last `window` samples
    /// (clamped to `1..=max_window`), including `val`.
    #[inline]
    pub fn push(&mut self, val: f32, window: usize) -> f32 {
        let w = window.clamp(1, self.max_window) as u64;
        let cur_idx = self.index;
        self.index = cur_idx.wrapping_add(1);

        // Evict elements outside the sliding window from the front.
        while self.len > 0 {
            if let Some(&(front_idx, _)) = self.ring.get(self.head) {
                if cur_idx.wrapping_sub(front_idx) >= w {
                    self.head = self.head.wrapping_add(1) & self.mask;
                    self.len -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Maintain decreasing monotonicity from front to back.
        while self.len > 0 {
            let back_pos = self.tail.wrapping_sub(1) & self.mask;
            if let Some(&(_, back_val)) = self.ring.get(back_pos) {
                if back_val <= val {
                    self.tail = back_pos;
                    self.len -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Push new element at back.
        if let Some(slot) = self.ring.get_mut(self.tail) {
            *slot = (cur_idx, val);
        }
        self.tail = self.tail.wrapping_add(1) & self.mask;
        self.len += 1;

        self.ring.get(self.head).map_or(val, |&(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn naive_sliding_max(input: &[f32], window: usize) -> alloc::vec::Vec<f32> {
        let w = window.max(1);
        let mut out = alloc::vec::Vec::with_capacity(input.len());
        for i in 0..input.len() {
            let start = (i + 1).saturating_sub(w);
            let mut m = f32::NEG_INFINITY;
            for &v in &input[start..=i] {
                if v > m {
                    m = v;
                }
            }
            out.push(m);
        }
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]
        #[test]
        fn matches_naive_reference(
            seq in prop::collection::vec(-100.0f32..100.0f32, 1..250),
            window in 1usize..64,
        ) {
            let mut sm = SlidingMax::new(64);
            let expected = naive_sliding_max(&seq, window);
            for (&x, &exp) in seq.iter().zip(expected.iter()) {
                let got = sm.push(x, window);
                prop_assert_eq!(got, exp);
            }
        }
    }

    #[test]
    fn matches_naive_on_100k_samples() {
        let mut sm = SlidingMax::new(72);
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut ring = [0.0f32; 72];
        for i in 0..100_000usize {
            // xorshift64*
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let val = ((state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / 16_777_216.0;
            ring[i % 72] = val;
            let count = (i + 1).min(72);
            let mut naive = f32::NEG_INFINITY;
            for &v in &ring[..count] {
                if v > naive {
                    naive = v;
                }
            }
            let got = sm.push(val, 72);
            assert_eq!(got, naive, "mismatch at sample {i}");
        }
    }
}
