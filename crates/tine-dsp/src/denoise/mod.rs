//! Pluggable frame-based denoiser trait and built-in implementations.

pub mod rnnoise;

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

    /// Processes a single frame of `inp` (length [`Denoiser::frame_len`]) into `out`,
    /// returning a speech/VAD probability in `[0.0, 1.0]` when available.
    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32;

    /// Resets internal denoiser state without allocating on real-time paths when avoidable.
    fn reset(&mut self);

    /// Optional one-time warm-up called from non-real-time `prepare()`.
    fn warm_up(&mut self) {}
}
