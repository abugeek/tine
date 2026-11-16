//! Control plane for Tine: shared-memory client, controller, persistence,
//! `cpal` audio I/O, spectrum analyzer, Auto-Tune runner, monitor, and APO administration.
//!
//! Non-RT control plane (except `cpal` callbacks in `audio_io` and `monitor`).

pub mod apo_admin;
pub mod audio_io;
pub mod autotune;
pub mod controller;
pub mod model;
pub mod monitor;
pub mod paths;
pub mod persist;
pub mod shm_client;
pub mod spectrum;
pub mod virtual_cable;
pub mod visualizer;
pub mod win;
