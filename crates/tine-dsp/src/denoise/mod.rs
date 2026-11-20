//! Pluggable frame-based denoiser trait and built-in implementations.

#[cfg(feature = "deepfilter")]
pub mod deepfilter;
pub mod rnnoise;

#[cfg(feature = "deepfilter")]
pub use deepfilter::DeepFilterDenoiser;
pub use rnnoise::RnnoiseDenoiser;

/// Frame-based mono audio denoiser operating at its own native sample rate.
///
/// Implementations are owned by [`crate::stages::CleanupStage`] and must be
/// allocation-free and panic-free inside [`Denoiser::process_frame`].
pub trait Denoiser: Send {
    /// Number of samples expected per call to [`Denoiser::process_frame`].
    fn frame_len(&self) -> usize;

    /// Native sample rate in Hz at which this denoiser operates.
    fn native_rate(&self) -> u32;

    /// Algorithmic lookahead / synthesis latency in samples at [`Denoiser::native_rate`].
    fn algo_latency(&self) -> u32;

    /// `true` when this denoiser performs zero spectral processing and zero framing delay
    /// ([`BypassDenoiser`], enabling `CleanupStage` low-latency mode).
    fn is_passthrough(&self) -> bool {
        false
    }

    /// Processes a single frame of `inp` (length [`Denoiser::frame_len`]) into `out`,
    /// returning a speech/VAD probability in `[0.0, 1.0]` when available.
    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32;

    /// Resets internal denoiser state without allocating on real-time paths when avoidable.
    fn reset(&mut self);

    /// Optional one-time warm-up called from non-real-time `prepare()`.
    fn warm_up(&mut self) {}
}

/// Zero-latency passthrough denoiser used when `DenoiseEngine::Off` is selected.
pub struct BypassDenoiser;

impl Denoiser for BypassDenoiser {
    #[inline]
    fn frame_len(&self) -> usize {
        480
    }

    #[inline]
    fn native_rate(&self) -> u32 {
        48_000
    }

    #[inline]
    fn algo_latency(&self) -> u32 {
        0
    }

    #[inline]
    fn is_passthrough(&self) -> bool {
        true
    }

    #[inline]
    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32 {
        let n = inp.len().min(out.len());
        if let (Some(src), Some(dst)) = (inp.get(..n), out.get_mut(..n)) {
            dst.copy_from_slice(src);
        }
        1.0
    }

    #[inline]
    fn reset(&mut self) {}
}
