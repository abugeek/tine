//! `DeepFilterNet3` (`dfn3_rs`) "Studio Clean" denoiser implementation.

use alloc::boxed::Box;

use super::Denoiser;

/// `DeepFilterNet3` frame size at 48 kHz (10 ms).
pub const DFN3_FRAME: usize = dfn3_rs::FRAME_SIZE;

/// `DeepFilterNet3` native sample rate in Hz.
pub const DFN3_RATE: u32 = dfn3_rs::SAMPLE_RATE;

/// [`Denoiser`] backed by `dfn3_rs::Denoiser` at 48 kHz / 480-sample frames.
pub struct DeepFilterDenoiser {
    inner: Box<dfn3_rs::Denoiser>,
}

impl Default for DeepFilterDenoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepFilterDenoiser {
    /// Creates a new [`DeepFilterDenoiser`] with embedded `DeepFilterNet3` weights.
    ///
    /// Heap allocation happens here; [`Self::process_frame`] and [`Self::reset`] are allocation-free.
    #[must_use]
    pub fn new() -> Self {
        let mut inner = Box::new(dfn3_rs::Denoiser::new());
        inner.set_thresholds(-10.0, 30.0, 20.0);
        Self { inner }
    }
}

impl Denoiser for DeepFilterDenoiser {
    #[inline]
    fn frame_len(&self) -> usize {
        DFN3_FRAME
    }

    #[inline]
    fn native_rate(&self) -> u32 {
        DFN3_RATE
    }

    #[inline]
    fn algo_latency(&self) -> u32 {
        dfn3_rs::ALGORITHM_DELAY as u32
    }

    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32 {
        let Some(in_slice) = inp.get(..DFN3_FRAME) else {
            return 1.0;
        };
        let Some(out_slice) = out.get_mut(..DFN3_FRAME) else {
            return 1.0;
        };
        let (Ok(inp_arr), Ok(out_arr)) = (
            <&[f32; DFN3_FRAME]>::try_from(in_slice),
            <&mut [f32; DFN3_FRAME]>::try_from(out_slice),
        ) else {
            return 1.0;
        };

        let lsnr_db = self.inner.process(inp_arr, out_arr);
        if !lsnr_db.is_finite() {
            return 1.0;
        }
        ((lsnr_db + 8.0) / 16.0).clamp(0.0, 1.0)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn warm_up(&mut self) {
        self.inner.set_thresholds(-100.0, 100.0, 100.0);
        let frame = [1e-4_f32; DFN3_FRAME];
        let mut out = [0.0_f32; DFN3_FRAME];
        for _ in 0..4 {
            let _ = self.inner.process(&frame, &mut out);
        }
        self.inner.reset();
        self.inner.set_thresholds(-10.0, 30.0, 20.0);
    }
}
