//! CLI: `tine <command> …` — argument parsing (lexopt), dispatch to the
//! running instance over the pipe where it makes sense, direct execution
//! otherwise (`status`, `diag`, `apo`), and exit codes.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use lexopt::Parser;
use serde_json::{Value, json};
use tine_protocol::params::{SPECS, ParamId};
use tine_protocol::presets::Archetype;
use tine_engine::model::Macro;

use crate::proto::{PipeRequest, PipeResponse, exit_code};
use crate::single_instance;

/// Flags that take a value (`--device X` etc.).
const VALUE_FLAGS: [&str; 5] = ["device", "endpoint", "out", "minutes", "report"];

/// Top-level mode selected by argv.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// Start (or join) the tray app.
    App {
        /// `--background`: autostart/relaunch marker; a second instance stays
        /// quiet instead of opening the window.
        background: bool,
        /// `--metrics`: print `metric:` lines to stderr (measurements).
        metrics: bool,
    },
    /// `--ui`: the Rune window child (spawned by the app, D-017).
    UiChild,
    /// `--version`: print the version and exit.
    Version,
    /// `--elevated-helper …`: the legacy UAC helper path. D-018 removed the
    /// system APO (everything runs non-elevated); kept as a documented stub
    /// so old shortcuts and log entries produce a clear answer.
    ElevatedHelper {
        /// Rest of the helper command line (echoed in the notice).
        rest: Vec<String>,
    },
    /// A CLI subcommand.
    Cli(CliCommand),
}

/// One CLI subcommand with its parsed arguments.
#[derive(Clone, Debug, PartialEq)]
pub enum CliCommand {
    /// `status` — live state from the running instance or a static snapshot.
    Status,
    /// `diag [--out <file>]` — privacy-safe diagnostics bundle.
    Diag {
        /// Output file (default `tine-diag.zip` in the current dir).
        out: PathBuf,
    },
    /// `set <key> <value>` — pin one Pro parameter (SPECS key).
    Set {
        /// Machine key, e.g. `input_gain_db`.
        key: String,
        /// Raw value (clamped by the engine).
        value: f32,
    },
    /// `macro <name> <value>`.
    Macro {
        /// One of `purity`, `body`, `presence`, `leveler`.
        name: String,
        /// `0.0..=1.0`.
        value: f32,
    },
    /// `archetype <name>`.
    Archetype {
        /// One of `dynamic`, `condenser`, `headset`, `laptop` (case-free).
        name: String,
    },
    /// `bypass on|off`.
    Bypass {
        /// Enable state.
        on: bool,
    },
    /// `monitor on|off [--device <id>]`.
    Monitor {
        /// Enable state.
        on: bool,
        /// Optional playback device id.
        device: Option<String>,
    },
    /// `calibrate` — start Auto-Tune (8 s).
    Calibrate,
    /// `meters` — live telemetry + spectrum.
    Meters,
    /// `apo status|install|uninstall|repair [--endpoint <id>]` (D-018:
    /// virtual-cable admin, zero UAC).
    Apo {
        /// Sub-action.
        sub: ApoSub,
        /// Optional endpoint filter.
        endpoint: Option<String>,
    },
    /// `soak --minutes <N> --report <file>`.
    Soak {
        /// Soak window in whole minutes.
        minutes: u64,
        /// Report file path.
        report: PathBuf,
    },
}

/// `apo` sub-actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApoSub {
    /// Virtual-cable + endpoint state.
    Status,
    /// Not applicable under D-018 (informational).
    Install,
    /// Not applicable under D-018 (informational).
    Uninstall,
    /// Not applicable under D-018 (informational).
    Repair,
}

/// argv split into positionals, value flags, and standalone flags.
#[derive(Default)]
struct Args {
    positional: Vec<String>,
    value_flags: BTreeMap<String, String>,
    flags: Vec<String>,
}

impl Args {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|f| f.as_str() == name).map(String::as_str)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.value_flags.get(name).map(String::as_str)
    }
}

/// Owned mirror of `lexopt::Arg<'_>`: lexopt's variant borrows the parser,
/// so it is copied out before the arms run (`parser.value()` needs the
/// parser back).
enum OwnedArg {
    /// A long option, dashes not included.
    Long(String),
    /// A short option.
    Short(char),
    /// A free value.
    Value(std::ffi::OsString),
}

fn scan(mut parser: Parser) -> Result<Args, String> {
    let mut args = Args::default();
    loop {
        let owned = match parser.next().map_err(|e| e.to_string())? {
            Some(lexopt::Arg::Long(name)) => OwnedArg::Long(name.to_owned()),
            Some(lexopt::Arg::Short(c)) => OwnedArg::Short(c),
            Some(lexopt::Arg::Value(v)) => OwnedArg::Value(v),
            None => break,
        };
        match owned {
            OwnedArg::Long(name) if name == "background" => args.flags.push("background".to_owned()),
            OwnedArg::Long(name) if name == "metrics" => args.flags.push("metrics".to_owned()),
            OwnedArg::Long(name) if name == "ui" => args.flags.push("ui".to_owned()),
            OwnedArg::Long(name) if name == "elevated-helper" => {
                args.flags.push("elevated-helper".to_owned());
            }
            OwnedArg::Long(name) if name == "help" => return Err(String::new()),
            OwnedArg::Short('h') => return Err(String::new()),
            OwnedArg::Long(name) if VALUE_FLAGS.contains(&name.as_str()) => {
                let value = parser
                    .value()
                    .map_err(|e| format!("--{name} expects a value: {e}"))?
                    .to_string_lossy()
                    .into_owned();
                args.value_flags.insert(name, value);
            }
            OwnedArg::Long(name) if name == "version" => args.flags.push("version".to_owned()),
            OwnedArg::Short('v') => args.flags.push("version".to_owned()),
            OwnedArg::Long(other) => return Err(format!("unknown flag `--{other}`")),
            OwnedArg::Short(other) => return Err(format!("unknown flag `-{other}`")),
            OwnedArg::Value(v) => args.positional.push(v.to_string_lossy().into_owned()),
        }
    }
    Ok(args)
}

/// Parses argv (without the program path).
///
/// # Errors
/// Returns a usage error message for malformed invocations (empty string =
/// plain `--help`).
pub fn parse(args: Vec<OsString>) -> Result<Mode, String> {
    let args = scan(Parser::from_args(args.into_iter()))?;
    if args.flag("version").is_some() {
        return Ok(Mode::Version);
    }
    if args.flag("ui").is_some() {
        return Ok(Mode::UiChild);
    }
    if args.flag("elevated-helper").is_some() {
        return Ok(Mode::ElevatedHelper {
            rest: args.positional.clone(),
        });
    }
    let background = args.flag("background").is_some();
    let metrics = args.flag("metrics").is_some();
    let Some(sub) = args.positional.first().cloned() else {
        return Ok(Mode::App { background, metrics });
    };
    let p = Args {
        positional: args.positional[1..].to_vec(),
        value_flags: args.value_flags,
        flags: args.flags,
    };
    let cmd = match sub.as_str() {
        "status" => CliCommand::Status,
        "diag" => CliCommand::Diag {
            out: p
                .value("out")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("tine-diag.zip")),
        },
        "set" => CliCommand::Set {
            key: take_word(&p, 0, "set <key> <value>")?,
            value: take_f32(&p, 1, "set <key> <value>")?,
        },
        "macro" => CliCommand::Macro {
            name: take_word(&p, 0, "macro <name> <value>")?,
            value: take_f32(&p, 1, "macro <name> <value>")?,
        },
        "archetype" => CliCommand::Archetype {
            name: take_word(&p, 0, "archetype <name>")?,
        },
        "bypass" => CliCommand::Bypass {
            on: on_off(&p, 0, "bypass on|off")?,
        },
        "monitor" => CliCommand::Monitor {
            on: on_off(&p, 0, "monitor on|off")?,
            device: p.value("device").map(str::to_owned),
        },
        "calibrate" => CliCommand::Calibrate,
        "meters" => CliCommand::Meters,
        "apo" => CliCommand::Apo {
            sub: match p.positional.first().map(String::as_str) {
                Some("status") => ApoSub::Status,
                Some("install") => ApoSub::Install,
                Some("uninstall") => ApoSub::Uninstall,
                Some("repair") => ApoSub::Repair,
                Some(other) => return Err(format!("apo: unknown subcommand `{other}`")),
                None => return Err("apo expects status|install|uninstall|repair".to_owned()),
            },
            endpoint: p.value("endpoint").map(str::to_owned),
        },
        "soak" => CliCommand::Soak {
            minutes: p
                .value("minutes")
                .and_then(|m| m.parse().ok())
                .ok_or("soak needs --minutes <whole number>")?,
            report: p
                .value("report")
                .map(PathBuf::from)
                .ok_or("soak needs --report <file>")?,
        },
        "help" => return Err(String::new()),
        other => return Err(format!("unknown command `{other}`")),
    };
    Ok(Mode::Cli(cmd))
}

fn take_word(p: &Args, idx: usize, usage: &str) -> Result<String, String> {
    p.positional
        .get(idx)
        .cloned()
        .ok_or_else(|| format!("usage: tine {usage}"))
}

fn take_f32(p: &Args, idx: usize, usage: &str) -> Result<f32, String> {
    p.positional
        .get(idx)
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("usage: tine {usage} (numeric value)"))
}

fn on_off(p: &Args, idx: usize, usage: &str) -> Result<bool, String> {
    match p.positional.get(idx).map(String::as_str) {
        Some("on") | Some("1") | Some("true") => Ok(true),
        Some("off") | Some("0") | Some("false") => Ok(false),
        _ => Err(format!("usage: tine {usage}")),
    }
}

/// Finds a `ParamId` by machine key.
#[must_use]
pub fn param_id_by_key(key: &str) -> Option<ParamId> {
    SPECS.iter().find(|spec| spec.key == key).map(|spec| spec.id)
}

/// Parses an archetype from id or display name (case-insensitive).
#[must_use]
pub fn archetype_from_name(name: &str) -> Option<Archetype> {
    let lower = name.to_ascii_lowercase();
    [
        Archetype::Dynamic,
        Archetype::Condenser,
        Archetype::Headset,
        Archetype::Laptop,
    ]
    .into_iter()
    .find(|a| a.id().eq_ignore_ascii_case(&lower) || a.display_name().to_ascii_lowercase() == lower)
}

/// Runs a CLI command (console already attached).
///
/// Commands needing live engine state go through the running instance's
/// pipe; `status`, `diag`, `apo`, and a cold `soak` also work standalone.
pub fn run(cmd: CliCommand) -> i32 {
    match cmd {
        CliCommand::Status => cmd_status(),
        CliCommand::Diag { out } => cmd_diag(&out),
        CliCommand::Set { key, value } => forward(PipeRequest::Set { key, value }, "set"),
        CliCommand::Macro { name, value } => {
            if Macro::from_name(&name).is_none() {
                println!("error: unknown macro `{name}` (purity|body|presence|leveler)");
                return exit_code::USAGE;
            }
            forward(PipeRequest::Macro { name, value }, "macro")
        }
        CliCommand::Archetype { name } => {
            if archetype_from_name(&name).is_none() {
                println!("error: unknown archetype `{name}` (dynamic|condenser|headset|laptop)");
                return exit_code::USAGE;
            }
            forward(PipeRequest::Archetype { name }, "archetype")
        }
        CliCommand::Bypass { on } => forward(PipeRequest::Bypass { on }, "bypass"),
        CliCommand::Monitor { on, device } => {
            forward(PipeRequest::Monitor { on, device }, "monitor")
        }
        CliCommand::Calibrate => forward(PipeRequest::Calibrate, "calibrate"),
        CliCommand::Meters => forward(PipeRequest::Meters, "meters"),
        CliCommand::Apo { sub, endpoint } => cmd_apo(sub, endpoint.as_deref()),
        CliCommand::Soak { minutes, report } => forward(
            PipeRequest::Soak {
                minutes,
                report: report.display().to_string(),
            },
            "soak",
        ),
    }
}

fn forward(req: PipeRequest, what: &str) -> i32 {
    let name = single_instance::pipe_name_for_user();
    match single_instance::send_request(&name, &req) {
        Ok(resp) => {
            print_response(&resp, what);
            resp.code
        }
        Err(e) => {
            println!("{e}");
            exit_code::NOT_RUNNING
        }
    }
}

fn print_response(resp: &PipeResponse, what: &str) {
    if resp.ok {
        match resp.data.as_ref() {
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    if k != "spectrum" {
                        println!("{k}: {}", compact(v, k));
                    }
                }
                if let Some(spec) = map.get("spectrum").and_then(Value::as_array) {
                    let values: Vec<String> = spec
                        .iter()
                        .map(|b| format!("{b:.1}", b = b.as_f64().unwrap_or(0.0)))
                        .collect();
                    println!("spectrum: {}", values.join(" "));
                }
            }
            Some(_) | None => println!("{what} ok"),
        }
    } else {
        println!("error: {}", resp.error.clone().unwrap_or_else(|| "unknown".into()));
    }
}

fn compact(v: &Value, key: &str) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        other => match key {
            "telemetry" | "macros" => other
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| format!("{k}={}", compact(v, k)))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_else(|| other.to_string()),
            _ => other.to_string(),
        },
    }
}

fn cmd_status() -> i32 {
    let name = single_instance::pipe_name_for_user();
    match single_instance::send_request(&name, &PipeRequest::Status) {
        Ok(resp) if resp.ok => {
            let field = |k: &str| {
                resp.data
                    .as_ref()
                    .and_then(|d| d.get(k).cloned())
                    .unwrap_or(Value::Null)
            };
            let started = field("uptime_s")
                .as_f64()
                .map(|s| format!("{s:.0} s"))
                .unwrap_or_else(|| "—".to_owned());
            println!("tine {} — running ({started})", crate::version::tine_version());
            println!("endpoint: {} ({})", compact(&field("mic_name"), "mic_name"), compact(&field("endpoint_id"), "endpoint_id"));
            println!("archetype: {}", compact(&field("archetype"), "archetype"));
            println!("state: {}", compact(&field("state"), "state"));
            println!("link: {}", compact(&field("link"), "link"));
            println!("autostart: {}", compact(&field("autostart"), "autostart"));
            println!("monitor: {} ({})", compact(&field("monitor"), "monitor"), compact(&field("monitor_device"), "monitor_device"));
            println!("windows: {}", compact(&field("windows"), "windows"));
            exit_code::OK
        }
        Ok(resp) => {
            println!("error: {}", resp.error.unwrap_or_default());
            resp.code
        }
        Err(_) => {
            // No instance: static snapshot.
            println!("tine {} — not running", crate::version::tine_version());
            println!("windows: {}", crate::version::windows_edition());
            println!("cpu: {}", crate::version::cpu_model());
            println!("autostart: {}", crate::autostart::is_enabled().map(|b| b.to_string()).unwrap_or_else(|e| e));
            println!("capture endpoints:");
            match tine_engine::apo_admin::list_capture_endpoints() {
                Ok(list) => {
                    for ep in list {
                        println!("  {} — {}", ep.name, crate::diag::apo_state_string(&ep.state));
                    }
                }
                Err(e) => println!("  (enumeration failed: {e})"),
            }
            exit_code::OK
        }
    }
}

fn cmd_diag(out: &std::path::Path) -> i32 {
    let paths = match tine_engine::paths::EnginePaths::system() {
        Ok(p) => p,
        Err(e) => {
            println!("error: cannot resolve Tine paths: {e}");
            return exit_code::FAILURE;
        }
    };
    let live = single_instance::send_request(&single_instance::pipe_name_for_user(), &PipeRequest::Status)
        .ok()
        .and_then(|r| r.ok.then_some(r).and_then(|r| r.data));
    let extra = json!({
        "context": {
            "instance_running": live.is_some(),
            "live_status": live.unwrap_or(Value::Null),
        }
    });
    let entries = crate::diag::build_zip(&paths, extra);
    match crate::diag::write(out, entries) {
        Ok(()) => {
            println!("wrote {}", out.display());
            exit_code::OK
        }
        Err(e) => {
            println!("error: {e}");
            exit_code::FAILURE
        }
    }
}

fn cmd_apo(sub: ApoSub, endpoint: Option<&str>) -> i32 {
    match sub {
        ApoSub::Status => match tine_engine::apo_admin::list_capture_endpoints() {
            Ok(list)
                if list.is_empty() && endpoint.is_none() =>
            {
                println!("no capture endpoints found");
                exit_code::OK
            }
            Ok(list) => {
                let mut matched = 0;
                for ep in &list {
                    if let Some(needle) = endpoint
                        && !ep.id.contains(needle)
                        && !ep.name.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
                    {
                        continue;
                    }
                    matched += 1;
                    println!(
                        "{} [{}] — {} ({})",
                        ep.name,
                        format!("{:?}", ep.form_factor),
                        crate::diag::apo_state_string(&ep.state),
                        ep.id
                    );
                }
                if matched == 0 {
                    println!("no endpoint matches `{}`", endpoint.unwrap_or(""));
                }
                match tine_engine::apo_admin::find_virtual_cable_sink() {
                    Ok(Some(sink)) => {
                        println!(
                            "virtual cable: {} ({})",
                            sink.name,
                            if sink.is_available { "available" } else { "unavailable" }
                        );
                        exit_code::OK
                    }
                    Ok(None) => {
                        println!("virtual cable: not installed");
                        exit_code::FAILURE
                    }
                    Err(e) => {
                        println!("error: {e}");
                        exit_code::FAILURE
                    }
                }
            }
            Err(e) => {
                println!("error: {e}");
                exit_code::FAILURE
            }
        },
        ApoSub::Install | ApoSub::Uninstall | ApoSub::Repair => {
            println!("Nothing to do: Tine v0.1 routes through a virtual audio cable (D-018), so no");
            println!("system APO is installed and no elevation is ever required:");
            println!("  1. install \"VB-Audio Virtual Cable\" (or an equivalent virtual cable),");
            println!("  2. select your real microphone in Tine and set the cable input as the");
            println!("     microphone in your apps (`tine apo status` shows the current state).");
            exit_code::OK
        }
    }
}

/// Usage text printed by `tine --help` (and on parse errors).
#[must_use]
pub fn usage() -> String {
    format!(
        "Tine {version} — native microphone DSP\n\
         \n\
         usage:\n\
         \x20 tine [--background] [--metrics]  start the tray app (second launch opens the window)\n\
         \x20 tine status                live state of the running instance\n\
         \x20 tine diag [--out FILE]     privacy-safe diagnostics bundle (default tine-diag.zip)\n\
         \x20 tine set KEY VALUE         set one Pro parameter (SPECS key, pins it)\n\
         \x20 tine macro NAME VALUE      move a macro (purity|body|presence|leveler, 0.0..=1.0)\n\
         \x20 tine archetype NAME        dynamic|condenser|headset|laptop\n\
         \x20 tine bypass on|off         global A/B bypass\n\
         \x20 tine monitor on|off [--device ID]   \"Hear Myself\"\n\
         \x20 tine calibrate             run the 8-second Auto-Tune\n\
         \x20 tine meters                live telemetry + spectrum\n\
         \x20 tine apo status|install|uninstall|repair [--endpoint ID]\n\
         \x20 tine soak --minutes N --report FILE soak the running instance\n",
        version = crate::version::tine_version()
    )
}

#[cfg(test)]
mod tests {
    use super::{ApoSub, CliCommand, Mode, archetype_from_name, param_id_by_key};
    use std::ffi::OsString;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    fn app() -> Mode {
        Mode::App { background: false, metrics: false }
    }

    #[test]
    fn parses_app_mode_and_flags() {
        assert_eq!(super::parse(argv(&[])), Ok(app()));
        assert_eq!(
            super::parse(argv(&["--background"])),
            Ok(Mode::App { background: true, metrics: false })
        );
        assert_eq!(
            super::parse(argv(&["--background", "--metrics"])),
            Ok(Mode::App { background: true, metrics: true })
        );
    }

    #[test]
    fn parses_macro_set_bypass_archetype() {
        match super::parse(argv(&["macro", "body", "0.7"])) {
            Ok(Mode::Cli(CliCommand::Macro { name, value })) => {
                assert_eq!((name.as_str(), value), ("body", 0.7));
            }
            other => panic!("{other:?}"),
        }
        match super::parse(argv(&["set", "input_gain_db", "3.5"])) {
            Ok(Mode::Cli(CliCommand::Set { key, value })) => {
                assert_eq!(key, "input_gain_db");
                assert_eq!(value, 3.5);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            super::parse(argv(&["bypass", "off"])),
            Ok(Mode::Cli(CliCommand::Bypass { on: false }))
        );
        assert_eq!(
            super::parse(argv(&["archetype", "laptop"])),
            Ok(Mode::Cli(CliCommand::Archetype { name: "laptop".to_owned() }))
        );
    }

    #[test]
    fn parses_monitor_with_device() {
        match super::parse(argv(&["monitor", "on", "--device", "out-9"])) {
            Ok(Mode::Cli(CliCommand::Monitor { on, device })) => {
                assert!(on);
                assert_eq!(device.as_deref(), Some("out-9"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_diag_apo_soak() {
        assert_eq!(
            super::parse(argv(&["diag"])),
            Ok(Mode::Cli(CliCommand::Diag { out: "tine-diag.zip".into() }))
        );
        assert_eq!(
            super::parse(argv(&["apo", "status", "--endpoint", "mic0"])),
            Ok(Mode::Cli(CliCommand::Apo { sub: ApoSub::Status, endpoint: Some("mic0".to_owned()) }))
        );
        assert_eq!(
            super::parse(argv(&["soak", "--minutes", "1", "--report", "target/soak.json"])),
            Ok(Mode::Cli(CliCommand::Soak { minutes: 1, report: "target/soak.json".into() }))
        );
    }

    #[test]
    fn parses_ui_helper_version() {
        assert_eq!(super::parse(argv(&["--ui"])), Ok(Mode::UiChild));
        assert!(matches!(
            super::parse(argv(&["--elevated-helper", "noop"])),
            Ok(Mode::ElevatedHelper { .. })
        ));
        assert_eq!(super::parse(argv(&["--version"])), Ok(Mode::Version));
    }

    #[test]
    fn help_and_errors() {
        assert!(matches!(super::parse(argv(&["--help"])), Err(_)));
        assert!(super::parse(argv(&["frobnicate"])).is_err());
        assert!(super::parse(argv(&["soak", "--report", "x"])).is_err(), "missing --minutes");
        assert!(super::parse(argv(&["bypass", "maybe"])).is_err());
    }

    #[test]
    fn macros_and_archetypes_resolve() {
        use tine_engine::model::Macro;
        assert_eq!(Macro::from_name("body"), Some(Macro::Body));
        assert_eq!(
            archetype_from_name("DYNAMIC"),
            Some(tine_protocol::presets::Archetype::Dynamic)
        );
        assert_eq!(
            archetype_from_name("Gaming Headset / Boom"),
            Some(tine_protocol::presets::Archetype::Headset)
        );
        assert_eq!(archetype_from_name("nope"), None);
    }

    #[test]
    fn known_specs_keys_lookup() {
        assert!(param_id_by_key("input_gain_db").is_some());
        assert_eq!(param_id_by_key("does_not_exist"), None);
    }
}
