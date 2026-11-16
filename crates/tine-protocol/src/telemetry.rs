//! Real-time audio telemetry types published from the APO to the control plane.

use bytemuck::{Pod, Zeroable};

/// Bitflags for [`AudioTelemetryFrame::flags`] and [`crate::shm::ShmHeader::state`].
pub mod flags {
    /// Global A/B bypass is active.
    pub const BYPASS: u32 = 1 << 0;
    /// A fault (panic, NaN, or Inf) latched the APO into pass-through mode.
    pub const PASSTHROUGH_LATCHED: u32 = 1 << 1;
    /// Input signal clipped (`|x| >= 1.0`) during the frame.
    pub const INPUT_CLIPPED: u32 = 1 << 2;
    /// Denoiser resampler is active (endpoint sample rate != 48 kHz).
    pub const RESAMPLING_ACTIVE: u32 = 1 << 3;
}

/// Telemetry snapshot published at ~60 Hz by the APO via [`crate::sync::Triple`].
///
/// Spectrum data is intentionally omitted here; `tine.exe` computes the 32-band
/// spectrum from the post-limiter audio tap ring.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct AudioTelemetryFrame {
    /// Monotonically increasing frame counter.
    pub sequence: u64,
    /// Bitmask of [`flags`].
    pub flags: u32,
    /// Pre-DSP input RMS level in dBFS.
    pub input_rms_db: f32,
    /// Pre-DSP input peak level in dBFS.
    pub input_peak_db: f32,
    /// Post-DSP output RMS level in dBFS.
    pub output_rms_db: f32,
    /// Post-DSP output peak level in dBFS.
    pub output_peak_db: f32,
    /// Expander / gate attenuation in dB (0.0 when open, negative or positive magnitude per meter convention).
    pub gate_attenuation_db: f32,
    /// Compressor gain reduction in dB.
    pub comp_gain_reduction_db: f32,
    /// Split-band de-esser gain reduction in dB.
    pub deesser_reduction_db: f32,
}

const _: () = assert!(core::mem::size_of::<AudioTelemetryFrame>() == 40);
const _: () = assert!(core::mem::align_of::<AudioTelemetryFrame>() == 8);
