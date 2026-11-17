//! Native-rate resampling for frame-based denoisers at mismatched endpoint rates (D-006).

use alloc::boxed::Box;
use alloc::vec;

use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction, calculate_cutoff,
};

use crate::primitives::fifo::FrameFifo;

/// Resampling pair (`fs -> native_rate -> fs`) wrapping a frame-based denoiser at
/// non-native endpoint sample rates.
///
/// All buffers are pre-allocated in [`Self::new`]; [`Self::process_frames`] is allocation-free.
pub struct NativeRateResampler {
    up: Async<f32>,
    down: Async<f32>,
    in_fifo: FrameFifo,
    out_fifo: FrameFifo,
    primed_zeros: usize,
    frame_len: usize,
    up_in_buf: Box<[f32]>,
    down_out_buf: Box<[f32]>,
    frame_in: Box<[f32]>,
    frame_out: Box<[f32]>,
    latency_samples: u32,
}

/// Backward-compatible alias for [`NativeRateResampler`].
pub type RnnoiseResampler = NativeRateResampler;

impl core::fmt::Debug for NativeRateResampler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NativeRateResampler")
            .field("frame_len", &self.frame_len)
            .field("primed_zeros", &self.primed_zeros)
            .field("latency_samples", &self.latency_samples)
            .finish_non_exhaustive()
    }
}

impl NativeRateResampler {
    /// Creates a new `fs -> native_rate -> fs` resampler pair for `frame_len`-sample frames.
    ///
    /// Returns `None` if `fs` is non-finite or non-positive, `native_rate` or `frame_len` is 0,
    /// or if `rubato` rejects the ratio.
    #[must_use]
    pub fn new(fs: f32, native_rate: u32, frame_len: usize, max_block: usize) -> Option<Self> {
        if !fs.is_finite() || fs <= 0.0 || native_rate == 0 || frame_len == 0 {
            return None;
        }
        let fs_clamped = fs.clamp(8_000.0, 192_000.0);
        let fs_f64 = f64::from(fs_clamped);
        let native_f64 = f64::from(native_rate);
        let ratio_up = native_f64 / fs_f64;
        let ratio_down = fs_f64 / native_f64;

        let sinc_len = 32;
        let window = WindowFunction::BlackmanHarris2;
        let params = SincInterpolationParameters {
            sinc_len,
            f_cutoff: Some(calculate_cutoff(sinc_len, window)),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window,
        };

        let up = Async::<f32>::new_sinc(ratio_up, 1.0, &params, frame_len, 1, FixedAsync::Output).ok()?;
        let down = Async::<f32>::new_sinc(ratio_down, 1.0, &params, frame_len, 1, FixedAsync::Input).ok()?;

        let up_in_max = up.input_frames_max();
        let down_out_max = down.output_frames_max();

        // Prime output FIFO with enough zeros to cover one full frame + jitter margin.
        let nominal_frame_at_fs = ((frame_len as f64) * fs_f64 / native_f64 + 0.5) as usize;
        let primed_zeros = up_in_max.max(nominal_frame_at_fs) + 8;

        let in_fifo = FrameFifo::new(up_in_max, max_block.max(up_in_max));
        let mut out_fifo = FrameFifo::new(down_out_max, primed_zeros + down_out_max + max_block);
        out_fifo.push_zeros(primed_zeros);

        let mut resampler = Self {
            up,
            down,
            in_fifo,
            out_fifo,
            primed_zeros,
            frame_len,
            up_in_buf: vec![0.0; up_in_max].into_boxed_slice(),
            down_out_buf: vec![0.0; down_out_max].into_boxed_slice(),
            frame_in: vec![0.0; frame_len].into_boxed_slice(),
            frame_out: vec![0.0; frame_len].into_boxed_slice(),
            latency_samples: 0,
        };

        resampler.latency_samples = resampler.measure_latency(fs_f64, native_f64);
        resampler.reset();
        Some(resampler)
    }

    /// Returns the measured round-trip resampler + framing latency in samples at `fs`
    /// (excluding the denoiser's own algorithmic latency).
    #[inline]
    #[must_use]
    pub const fn latency(&self) -> u32 {
        self.latency_samples
    }

    /// Clears all FIFOs and resampler states, re-priming the output FIFO.
    pub fn reset(&mut self) {
        self.up.reset();
        self.down.reset();
        self.in_fifo.reset();
        self.out_fifo.reset();
        self.out_fifo.push_zeros(self.primed_zeros);
        self.frame_in.fill(0.0);
        self.frame_out.fill(0.0);
    }

    /// Processes `buf` through `up -> frame_fn(native_rate) -> down`, writing wet samples to `out_buf`.
    ///
    /// `out_buf` must have the same length as `buf`. Zero heap allocations.
    pub fn process_frames<F>(&mut self, buf: &[f32], out_buf: &mut [f32], mut frame_fn: F)
    where
        F: FnMut(&[f32], &mut [f32]),
    {
        self.in_fifo.push_slice(buf);

        loop {
            let needed_in = self.up.input_frames_next();
            if self.in_fifo.len() < needed_in {
                break;
            }

            let Some(up_in_slice) = self.up_in_buf.get_mut(..needed_in) else {
                break;
            };
            self.in_fifo.pop_slice(up_in_slice);

            let Ok(in_adapter) = SequentialSlice::new(&*up_in_slice, 1, needed_in) else {
                break;
            };
            let Ok(mut mid_adapter) = SequentialSlice::new_mut(&mut self.frame_in, 1, self.frame_len) else {
                break;
            };
            if self
                .up
                .process_into_buffer(&in_adapter, &mut mid_adapter, None)
                .is_err()
            {
                self.frame_in.fill(0.0);
            }

            frame_fn(&self.frame_in, &mut self.frame_out);

            let out_len = self.down.output_frames_next();
            let Some(down_out_slice) = self.down_out_buf.get_mut(..out_len) else {
                break;
            };

            let Ok(mid_in_adapter) = SequentialSlice::new(&self.frame_out, 1, self.frame_len) else {
                break;
            };
            let Ok(mut out_adapter) = SequentialSlice::new_mut(down_out_slice, 1, out_len) else {
                break;
            };
            if self
                .down
                .process_into_buffer(&mid_in_adapter, &mut out_adapter, None)
                .is_err()
            {
                down_out_slice.fill(0.0);
            }

            self.out_fifo.push_slice(down_out_slice);
        }

        let popped = self.out_fifo.pop_slice(out_buf);
        if popped < out_buf.len()
            && let Some(tail) = out_buf.get_mut(popped..)
        {
            tail.fill(0.0);
        }
    }

    /// Measures exact round-trip resampler + framing latency by running an impulse through
    /// `up -> identity -> down`.
    fn measure_latency(&mut self, fs_f64: f64, native_f64: f64) -> u32 {
        let half_frame = (self.frame_len as f64) * 0.5;
        let five_frames = (self.frame_len as f64) * 5.0;
        let impulse_offset = ((half_frame * fs_f64 / native_f64) as usize).max(16);
        let test_len = impulse_offset + ((five_frames * fs_f64 / native_f64) as usize).max(512);
        let mut input = vec![0.0f32; test_len];
        if let Some(slot) = input.get_mut(impulse_offset) {
            *slot = 1.0;
        }
        let mut output = vec![0.0f32; test_len];

        for (in_chunk, out_chunk) in input.chunks(64).zip(output.chunks_mut(64)) {
            self.process_frames(in_chunk, out_chunk, |frame_in, frame_out| {
                frame_out.copy_from_slice(frame_in);
            });
        }

        let mut peak_idx = 0usize;
        let mut peak_val = 0.0f32;
        for (i, &v) in output.iter().enumerate() {
            let av = v.abs();
            if av > peak_val {
                peak_val = av;
                peak_idx = i;
            }
        }

        peak_idx.saturating_sub(impulse_offset) as u32
    }
}
