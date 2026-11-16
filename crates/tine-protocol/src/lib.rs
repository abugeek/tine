//! Platform-neutral protocol contracts for Tine: parameters, lock-free primitives,
//! shared-memory layout, telemetry, and persisted configuration schemas.
//!
//! Real-time safe (`no_std`, no allocations, no panics).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing,)
)]

pub mod ids;
pub mod params;
pub mod presets;
pub mod settings;
pub mod shm;
pub mod sync;
pub mod telemetry;
