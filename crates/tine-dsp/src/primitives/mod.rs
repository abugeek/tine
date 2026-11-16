//! Core DSP building blocks (no allocations after `prepare()`).

pub mod biquad;
pub mod db;
pub mod delay;
pub mod envelope;
pub mod fifo;
pub mod sliding_max;
pub mod smoother;
