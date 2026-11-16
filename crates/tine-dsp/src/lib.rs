//! Real-time 7-stage microphone DSP pipeline, response curves, macro mapping,
//! built-in archetype presets, and auto-calibration math.
//!
//! Real-time safe (`no_std + alloc`, zero allocations after `prepare()`, no panics).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing,)
)]

extern crate alloc;

pub mod calibrator;
pub mod curve;
pub mod macros;
pub mod meter;
pub mod pipeline;
pub mod presets;
pub mod primitives;
pub mod resample;
pub mod rt;
pub mod stages;
