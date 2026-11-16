//! Development host (`cpal` mic -> `tine-dsp` -> headphones) for DSP work without the APO.
//!
//! Non-RT harness hosting RT `cpal` audio callbacks.

mod cli;
mod record;
mod run;

use std::process::ExitCode;

use cli::Command;

fn main() -> ExitCode {
    let cmd = match cli::parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            cli::print_usage();
            return ExitCode::FAILURE;
        }
    };

    let res = match cmd {
        Command::List => run::cmd_list(),
        Command::Run(ref args) => run::cmd_run(args),
        Command::RecordRaw(ref args) => run::cmd_record_raw(args),
        Command::Latency(ref args) => run::cmd_latency(args),
        Command::Help => {
            cli::print_usage();
            Ok(())
        }
    };

    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
