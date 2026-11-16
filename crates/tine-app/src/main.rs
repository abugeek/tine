//! `tine.exe`: single-process Windows tray, Rune window shell, CLI, and elevated APO helper.
//!
//! Non-RT application shell.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod ui_bridge;

fn main() {}
