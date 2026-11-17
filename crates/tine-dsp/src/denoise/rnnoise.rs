//! `RNNoise` (`nnnoiseless`) denoiser implementation.

use alloc::boxed::Box;

use nnnoiseless::DenoiseState;

use super::Denoiser;

/// `RNNoise` frame size at 48 kHz (10 ms).
pub const RNNOISE_FRAME: usize = 480;

/// `RNNoise` native sample rate in Hz.
pub const RNNOISE_RATE: u32 = 48_000;

/// Scale factor between `[-1.0, 1.0]` float audio and `RNNoise` 16-bit range.
const RNNOISE_SCALE: f32 = 32_768.0;
const RNNOISE_INV_SCALE: f32 = 1.0 / 32_768.0;

/// [`Denoiser`] backed by `nnnoiseless::DenoiseState` at 48 kHz / 480-sample frames.
pub struct RnnoiseDenoiser {
    state: Box<DenoiseState<'static>>,
    scratch_in: [f32; RNNOISE_FRAME],
}

impl Default for RnnoiseDenoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl RnnoiseDenoiser {
    /// Creates a new [`RnnoiseDenoiser`] with fresh `nnnoiseless` state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            scratch_in: [0.0; RNNOISE_FRAME],
        }
    }
}

impl Denoiser for RnnoiseDenoiser {
    #[inline]
    fn frame_len(&self) -> usize {
        RNNOISE_FRAME
    }

    #[inline]
    fn native_rate(&self) -> u32 {
        RNNOISE_RATE
    }

    #[inline]
    fn algo_latency(&self) -> u32 {
        // 480-sample RNNoise overlap-add synthesis delay at 48 kHz.
        RNNOISE_FRAME as u32
    }

    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32 {
        if inp.len() < RNNOISE_FRAME {
            return 0.0;
        }
        let Some(out_frame) = out.get_mut(..RNNOISE_FRAME) else {
            return 0.0;
        };

        for (dst, &src) in self.scratch_in.iter_mut().zip(inp.iter()) {
            *dst = src * RNNOISE_SCALE;
        }
        let vad = self.state.process_frame(out_frame, &self.scratch_in);
        for s in out_frame.iter_mut() {
            *s *= RNNOISE_INV_SCALE;
        }
        vad
    }

    fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.scratch_in.fill(0.0);
    }

    fn warm_up(&mut self) {
        // Warm up nnnoiseless global OnceCell state outside RT, then start with a fresh DenoiseState.
        let mut warmup = DenoiseState::new();
        let zeros = [0.0f32; RNNOISE_FRAME];
        let mut dummy_out = [0.0f32; RNNOISE_FRAME];
        let _ = warmup.process_frame(&mut dummy_out, &zeros);
        self.state = DenoiseState::new();
        self.scratch_in.fill(0.0);
    }
}
