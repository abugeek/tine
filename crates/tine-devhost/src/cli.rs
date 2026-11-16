//! Command-line argument parsing for `tine-devhost` using `lexopt`.

use std::path::PathBuf;

use lexopt::Arg::{Long, Short, Value};

/// Parsed `tine-devhost` subcommand.
#[derive(Debug)]
pub enum Command {
    /// List available input and output audio devices.
    List,
    /// Run live capture -> DSP -> playback.
    Run(RunArgs),
    /// Capture raw unprocessed microphone audio to a WAV file.
    RecordRaw(RecordRawArgs),
    /// Measure round-trip loopback latency with an impulse click.
    Latency(LatencyArgs),
    /// Print usage help.
    Help,
}

/// Arguments for `tine-devhost run`.
#[derive(Debug)]
pub struct RunArgs {
    pub input: String,
    pub output: String,
    pub params_path: Option<PathBuf>,
    pub record_processed: Option<PathBuf>,
    pub record_raw: Option<PathBuf>,
}

/// Arguments for `tine-devhost record-raw`.
#[derive(Debug)]
pub struct RecordRawArgs {
    pub input: String,
    pub seconds: f32,
    pub rate: Option<u32>,
    pub out_path: PathBuf,
}

/// Arguments for `tine-devhost latency`.
#[derive(Debug)]
pub struct LatencyArgs {
    pub input: String,
    pub output: String,
    pub seconds: f32,
}

/// Parses CLI arguments from `std::env::args_os()`.
pub fn parse_args() -> Result<Command, String> {
    let mut parser = lexopt::Parser::from_env();
    let Some(arg) = parser.next().map_err(|e| e.to_string())? else {
        return Ok(Command::Help);
    };

    match arg {
        Short('h') | Long("help") => Ok(Command::Help),
        Value(val) => {
            let cmd = val.to_string_lossy();
            match cmd.as_ref() {
                "list" => Ok(Command::List),
                "run" => parse_run(parser).map(Command::Run),
                "record-raw" => parse_record_raw(parser).map(Command::RecordRaw),
                "latency" => parse_latency(parser).map(Command::Latency),
                other => Err(format!(
                    "unknown subcommand `{other}` (expected list, run, record-raw, latency)"
                )),
            }
        }
        other => Err(format!("unexpected argument: {other:?}")),
    }
}

fn parse_run(mut parser: lexopt::Parser) -> Result<RunArgs, String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut params_path: Option<PathBuf> = None;
    let mut record_processed: Option<PathBuf> = None;
    let mut record_raw: Option<PathBuf> = None;

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("input") | Short('i') => {
                input = Some(
                    parser
                        .value()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Long("output") | Short('o') => {
                output = Some(
                    parser
                        .value()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Long("params") | Short('p') => {
                params_path = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?));
            }
            Long("record") | Short('r') => {
                record_processed = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?));
            }
            Long("record-raw") => {
                record_raw = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?));
            }
            Long("help") | Short('h') => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unexpected option in `run`: {other:?}")),
        }
    }

    Ok(RunArgs {
        input: input.unwrap_or_else(|| "default".to_owned()),
        output: output.unwrap_or_else(|| "default".to_owned()),
        params_path,
        record_processed,
        record_raw,
    })
}

fn parse_record_raw(mut parser: lexopt::Parser) -> Result<RecordRawArgs, String> {
    let mut input: Option<String> = None;
    let mut seconds: Option<f32> = None;
    let mut rate: Option<u32> = None;
    let mut out_path: Option<PathBuf> = None;

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("input") | Short('i') => {
                input = Some(
                    parser
                        .value()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Long("seconds") | Short('s') => {
                let s = parser
                    .value()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                seconds = Some(s.parse::<f32>().map_err(|e| format!("invalid --seconds: {e}"))?);
            }
            Long("rate") => {
                let s = parser
                    .value()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                rate = Some(s.parse::<u32>().map_err(|e| format!("invalid --rate: {e}"))?);
            }
            Value(val) if out_path.is_none() => {
                out_path = Some(PathBuf::from(val));
            }
            Long("help") | Short('h') => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unexpected option in `record-raw`: {other:?}")),
        }
    }

    Ok(RecordRawArgs {
        input: input.unwrap_or_else(|| "default".to_owned()),
        seconds: seconds.unwrap_or(5.0).max(0.1),
        rate,
        out_path: out_path.ok_or_else(|| "missing output WAV path for `record-raw`".to_owned())?,
    })
}

fn parse_latency(mut parser: lexopt::Parser) -> Result<LatencyArgs, String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut seconds: Option<f32> = None;

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("input") | Short('i') => {
                input = Some(
                    parser
                        .value()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Long("output") | Short('o') => {
                output = Some(
                    parser
                        .value()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Long("seconds") | Short('s') => {
                let s = parser
                    .value()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                seconds = Some(s.parse::<f32>().map_err(|e| format!("invalid --seconds: {e}"))?);
            }
            Long("help") | Short('h') => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unexpected option in `latency`: {other:?}")),
        }
    }

    Ok(LatencyArgs {
        input: input.unwrap_or_else(|| "default".to_owned()),
        output: output.unwrap_or_else(|| "default".to_owned()),
        seconds: seconds.unwrap_or(2.0).max(0.5),
    })
}

/// Prints CLI usage instructions.
pub fn print_usage() {
    println!(
        "\
tine-devhost — Live microphone DSP development host

USAGE:
  tine-devhost list
  tine-devhost run [--input <id|name>] [--output <id|name>] [--params <file.json>] [--record <out.wav>] [--record-raw <raw.wav>]
  tine-devhost record-raw [--input <id|name>] [--seconds <N>] [--rate <Hz>] <out.wav>
  tine-devhost latency [--input <id|name>] [--output <id|name>] [--seconds <N>]

INTERACTIVE COMMANDS (during `run`):
  set <key> <value>    Set a DSP parameter by key (e.g., set hpf_freq_hz 100)
  macro <name> <v>     Set a Golden Line macro (available once T10 MacroMapper lands)
  bypass               Toggle global A/B bypass
  stats                Print stream callback/underrun/overrun stats
  quit                 Stop and exit"
    );
}
