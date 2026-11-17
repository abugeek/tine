//! `tine-app` internals, exposed as a library target so the integration
//! suite in `tests/` can drive the full control plane headlessly
//! (D-017: `main.rs` stays a thin binary over this crate).
//!
//! Non-RT application shell library: no audio callbacks live here.

pub mod autostart;
#[allow(clippy::must_use_candidate)] // exit-code returns are often ignored at call sites
pub mod cli;
pub mod console;
pub mod diag;
pub mod hash;
pub mod logging;
pub mod proto;
pub mod security;
pub mod services;
pub mod shell;
#[allow(clippy::must_use_candidate)] // guard tuples are pattern-matched, not consumed
pub mod single_instance;
pub mod soak;
pub mod tray;
pub mod ui_bridge;
pub mod version;
pub mod zip;
