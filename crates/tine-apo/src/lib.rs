//! Windows Audio Processing Object (`tine_apo.dll`) hosting `tine-dsp` inside `audiodg.exe`.
//!
//! Hard real-time on `APOProcess`; every COM boundary is `catch_unwind`-guarded.

#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing,)
)]

pub mod boot;
pub mod com;
pub mod formats;
pub mod process;
pub mod shm_host;
pub mod test_host;
pub mod wrap;
