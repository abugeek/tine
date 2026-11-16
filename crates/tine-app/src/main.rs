//! `tine.exe`: the real Windows app — tray + control plane (parent process),
//! `--ui` Rune window child (D-017), CLI, single instance with a command
//! pipe, logging, autostart, and clean shutdown.
//!
//! Non-RT application shell: no audio callbacks live here.
//! Release builds are `windows_subsystem = "windows"` (no console); CLI
//! mode attaches to the parent console (`AttachConsole`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod cli;
mod console;
mod diag;
mod hash;
mod logging;
mod proto;
mod security;
mod services;
mod shell;
mod single_instance;
mod soak;
mod tray;
mod ui;
mod version;
mod zip;

pub mod ui_bridge;

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mode = match cli::parse(args) {
        Ok(mode) => mode,
        Err(usage) => {
            console::attach_for_cli();
            if usage.is_empty() {
                println!("{}", cli::usage());
            } else {
                println!("error: {usage}\n\n{}", cli::usage());
            }
            std::process::exit(proto::exit_code::USAGE);
        }
    };

    match mode {
        cli::Mode::App { background, metrics } => {
            // App mode: no console attach (tray app; logging goes to file).
            let code = shell::run_app(background, metrics);
            std::process::exit(code);
        }
        cli::Mode::UiChild => ui::run_ui_child(),
        cli::Mode::Version => {
            console::attach_for_cli();
            println!("tine {}", version::tine_version());
            std::process::exit(proto::exit_code::OK);
        }
        cli::Mode::ElevatedHelper { rest } => {
            console::attach_for_cli();
            println!(
                "Nothing elevated to do (D-018): Tine v0.1 routes through a virtual audio cable and\n\
                 never installs a system APO. The `--elevated-helper` entry point is kept only so\n\
                 old shortcuts answer clearly. Arguments ignored: {rest}",
                rest = rest.join(" ")
            );
            std::process::exit(proto::exit_code::OK);
        }
        cli::Mode::Cli(cmd) => {
            console::attach_for_cli();
            let code = cli::run(cmd);
            std::process::exit(code);
        }
    }
}
