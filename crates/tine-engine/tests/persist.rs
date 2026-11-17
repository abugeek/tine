//! Persistence tests: round-trips, corruption recovery, migration, and the
//! kill-loop proof that a killed writer never leaves a truncated file.

use std::path::Path;
use std::time::Duration;

use tine_engine::paths::EnginePaths;
use tine_engine::persist::{Loaded, atomic_write, load_profile, load_settings, save_profile, save_settings};
use tine_protocol::params::ParamId;
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, EndpointProfile, MacroPositions, PinSet};
use tine_protocol::settings::AppSettings;

fn temp_paths(tag: &str) -> EnginePaths {
    let base = tempfile::tempdir().ok().map_or_else(
        || std::env::temp_dir().join(format!("tine-persist-{tag}")),
        tempfile::TempDir::keep,
    );
    paths_under(&base.join(tag))
}

fn paths_under(dir: &Path) -> EnginePaths {
    EnginePaths::from_roots(dir.join("app"), dir.join("prog"), dir.join("local"))
}

fn calibration_fixture() -> Calibration {
    Calibration {
        noise_rms_db: -61.0,
        noise_peak_db: -55.5,
        speech_rms_db: -27.5,
        crest_db: 11.0,
        tilt_db: 2.5,
        derived: CalibrationOverrides {
            input_gain_db: 9.5,
            gate_threshold_db: -55.0,
            denoise_mix: 0.85,
            comp_thresh_db: -16.0,
            comp_makeup_db: 2.5,
        },
    }
}

fn profile_fixture() -> EndpointProfile {
    let mut profile = EndpointProfile::new("0.0.1.00000000.{persist-test}", Archetype::Headset);
    profile.macros = MacroPositions {
        purity: 0.3,
        body: 0.7,
        presence: 0.55,
        leveler: 0.8,
    };
    profile.pins = PinSet::from_bits(1 << ParamId::InputGainDb.index() as u64);
    profile.calibration = Some(calibration_fixture());
    profile
}

#[test]
fn settings_round_trip() {
    let paths = temp_paths("settings-rt");
    let settings = AppSettings {
        selected_endpoint_id: Some("ep-1".to_owned()),
        monitor_device_id: Some("out-1".to_owned()),
        monitor_enabled: true,
        autostart: true,
        window_width: 1600,
        ..AppSettings::default()
    };

    let saved = save_settings(&settings, &paths);
    assert!(saved.is_ok(), "save_settings failed: {saved:?}");
    let loaded = load_settings(&paths);
    assert!(loaded.is_fresh(), "expected Fresh, got {loaded:?}");
    assert_eq!(loaded.into_value(), settings);
}

#[test]
fn profile_round_trip() {
    let paths = temp_paths("profile-rt");
    let profile = profile_fixture();
    let saved = save_profile(&profile, &paths);
    assert!(saved.is_ok(), "save_profile failed: {saved:?}");

    let loaded = load_profile(profile.endpoint_id.as_str(), &paths);
    assert!(loaded.is_fresh(), "expected Fresh, got {loaded:?}");
    assert_eq!(loaded.into_value(), profile);
}

#[test]
fn missing_files_yield_defaults() {
    let paths = temp_paths("missing");
    match load_settings(&paths) {
        Loaded::Missing(s) => assert_eq!(s, AppSettings::default()),
        other => panic!("expected Missing, got {other:?}"),
    }
    match load_profile("never-saved", &paths) {
        Loaded::Missing(p) => {
            assert_eq!(p.archetype, Archetype::default());
            assert!(p.calibration.is_none());
        }
        other => panic!("expected Missing, got {other:?}"),
    }
}

#[test]
fn corrupt_settings_is_backed_up_and_defaults_used() {
    let paths = temp_paths("corrupt-settings");
    let settings = AppSettings::default();
    let saved = save_settings(&settings, &paths);
    assert!(saved.is_ok());
    // Truncate mid-JSON to simulate a torn write.
    std::fs::write(paths.settings_file(), b"{\"schema\":1,\"selected_endpoi").ok();

    match load_settings(&paths) {
        Loaded::Restored(s) => assert_eq!(s, AppSettings::default()),
        other => panic!("expected Restored, got {other:?}"),
    }
    // The corrupt file must be kept as settings.json-corrupt-<ts>.
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(paths.app_data.join("Tine")) {
        for entry in entries.filter_map(Result::ok) {
            if let Some(name) = entry.file_name().to_str() {
                found |= name.starts_with("settings.json-corrupt-");
            }
        }
    } else {
        panic!("cannot list settings dir");
    }
    assert!(found, "corrupt settings.json must be backed up");
    assert!(!paths.settings_file().exists(), "target must be moved aside");
}

#[test]
fn corrupt_profile_is_backed_up_and_defaults_used() {
    let paths = temp_paths("corrupt-profile");
    let mut profile = profile_fixture();
    profile.endpoint_id = "corrupt-ep".to_owned();
    let saved = save_profile(&profile, &paths);
    assert!(saved.is_ok());
    std::fs::write(paths.profile_file("corrupt-ep"), b"{ nope").ok();

    match load_profile("corrupt-ep", &paths) {
        Loaded::Restored(p) => assert_eq!(p.archetype, Archetype::default()),
        other => panic!("expected Restored, got {other:?}"),
    }
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(paths.profiles_dir()) {
        for entry in entries.filter_map(Result::ok) {
            if let Some(name) = entry.file_name().to_str() {
                found |= name.starts_with("corrupt-ep.json-corrupt-");
            }
        }
    } else {
        panic!("cannot list profiles dir");
    }
    assert!(found, "corrupt profile must be backed up");
}

#[test]
fn out_of_range_values_are_rejected_as_corrupt() {
    let paths = temp_paths("out-of-range");
    // Valid JSON, but a value far outside the SPECS range for hpf_freq_hz.
    std::fs::create_dir_all(paths.app_data.join("Tine")).ok();
    std::fs::write(
        paths.settings_file(),
        r#"{"schema":1,"window_width":999999,"window_height":800}"#,
    )
    .ok();
    match load_settings(&paths) {
        Loaded::Restored(s) => assert_eq!(s, AppSettings::default()),
        other => panic!("expected Restored (validation failure), got {other:?}"),
    }
}

#[test]
fn schema_0_files_are_migrated_to_schema_1() {
    let paths = temp_paths("migrate");

    // Schema-0 settings fixture: identical layout, no `schema` field.
    std::fs::create_dir_all(paths.app_data.join("Tine")).ok();
    std::fs::write(
        paths.settings_file(),
        r#"{"selected_endpoint_id":"ep-old","monitor_enabled":true,"autostart":true}"#,
    )
    .ok();
    match load_settings(&paths) {
        Loaded::Fresh(s) => {
            assert_eq!(s.schema, 1, "schema-0 file migrates to 1");
            assert_eq!(s.selected_endpoint_id, Some("ep-old".to_owned()));
            assert!(s.monitor_enabled);
            assert!(s.autostart);
        }
        other => panic!("expected Fresh after migration, got {other:?}"),
    }

    // Schema-0 profile fixture.
    std::fs::create_dir_all(paths.profiles_dir()).ok();
    std::fs::write(
        paths.profile_file("old-ep"),
        r#"{"endpoint_id":"old-ep","archetype":"condenser","macros":{"purity":0.25,"body":0.5,"presence":0.5,"leveler":0.5}}"#,
    )
    .ok();
    match load_profile("old-ep", &paths) {
        Loaded::Fresh(p) => {
            assert_eq!(p.schema, 1);
            assert_eq!(p.archetype, Archetype::Condenser);
            assert_eq!(p.macros.purity, 0.25);
        }
        other => panic!("expected Fresh after migration, got {other:?}"),
    }
}

#[test]
fn future_schema_is_rejected_as_corrupt() {
    let paths = temp_paths("future-schema");
    std::fs::create_dir_all(paths.app_data.join("Tine")).ok();
    std::fs::write(paths.settings_file(), r#"{"schema":99}"#).ok();
    match load_settings(&paths) {
        Loaded::Restored(s) => assert_eq!(s, AppSettings::default()),
        other => panic!("expected Restored for unsupported schema, got {other:?}"),
    }
}

#[test]
fn atomic_write_replaces_existing_file() {
    let paths = temp_paths("atomic");
    let target = paths.app_data.join("Tine").join("atomic.json");
    let first = atomic_write(&target, b"first");
    assert!(first.is_ok(), "first write: {first:?}");
    let second = atomic_write(&target, b"second");
    assert!(second.is_ok(), "second write: {second:?}");
    let content = std::fs::read_to_string(&target).ok();
    assert_eq!(content.as_deref(), Some("second"), "target must be replaced");
    // No temp litter.
    if let Ok(entries) = std::fs::read_dir(paths.app_data.join("Tine")) {
        for entry in entries.filter_map(Result::ok) {
            if let Some(name) = entry.file_name().to_str() {
                assert!(!name.contains(".tmp-"), "temp file left behind: {name}");
            }
        }
    } else {
        panic!("cannot list dir");
    }
}

// ---------------------------------------------------------------------------
// Kill-loop: killing the writer mid-write must never truncate the target.
// ---------------------------------------------------------------------------

/// Child-process companion: when `TINE_PERSIST_CHILD` is set, hammer the
/// settings file with atomic writes until killed (or 10k writes), then exit.
/// The parent (below) spawns this test binary with the env var set and kills
/// it at pseudo-random moments.
#[test]
fn kill_loop_writer_child() {
    if std::env::var("TINE_PERSIST_CHILD").ok().is_none() {
        return; // normal run: this test is a no-op (the parent spawns us)
    }
    let root = std::path::PathBuf::from(std::env::var("TINE_PERSIST_ROOT").ok().unwrap_or_default());
    let paths = paths_under(&root);
    for i in 0..100_000_u32 {
        let settings = AppSettings {
            selected_endpoint_id: Some(format!("ep-{i}")),
            ..AppSettings::default()
        };
        let _ = save_settings(&settings, &paths);
    }
}

/// 200 iterations: spawn the writer child, kill it mid-flight, and require
/// the settings file to be either absent (first write never completed) or a
/// *complete* valid document. A truncated file fails the test.
const KILL_LOOP_DIR: &str = "kill-loop-shared";

#[test]
fn kill_loop_never_leaves_a_truncated_settings_file() {
    const ITERATIONS: u32 = 200;
    // A fixed directory shared with the spawned writer children through
    // `TINE_PERSIST_ROOT`.
    let root = std::env::temp_dir().join(format!("tine-persist-{}-{KILL_LOOP_DIR}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = paths_under(&root);

    let exe = std::env::current_exe().ok();
    let Some(exe) = exe else {
        panic!("current_exe unavailable");
    };

    let mut killed_with_writes_landed = 0_u32;
    let mut killed_before_first_write = 0_u32;
    for i in 0..ITERATIONS {
        // Periodically start from a fresh slate to also exercise the
        // "target absent, first atomic write" path under process death.
        if i % 11 == 0 {
            let _ = std::fs::remove_file(paths.settings_file());
        }
        let child = std::process::Command::new(&exe)
            .args(["kill_loop_writer_child", "--exact"])
            .env("TINE_PERSIST_CHILD", "1")
            .env("TINE_PERSIST_ROOT", &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        let Some(mut child) = child else {
            panic!("failed to spawn writer child");
        };

        // Deterministic pseudo-random kill window spanning process startup
        // (~10 ms) and the write burst, so kills land inside writes too.
        let sleep_us = 5_000 + (i * 13_613) % 45_000;
        std::thread::sleep(Duration::from_micros(u64::from(sleep_us)));

        let had_file = paths.settings_file().exists();
        if had_file {
            killed_with_writes_landed = killed_with_writes_landed.saturating_add(1);
        } else {
            killed_before_first_write = killed_before_first_write.saturating_add(1);
        }
        let _ = child.kill();
        let _ = child.wait();

        // The invariant: never a truncated file.
        validate_not_truncated(&paths.settings_file());
        let _ = std::fs::remove_file(paths.settings_file());
    }
    assert!(
        killed_with_writes_landed > 0,
        "kill window never landed after a completed write; widen the window"
    );
    let _ = killed_before_first_write;
    println!(
        "kill-loop: {ITERATIONS} iterations, {killed_with_writes_landed} with writes landed,          {killed_before_first_write} before the first write"
    );
}

/// A settings file must be either absent or a complete, migratable, valid
/// document — never truncated.
fn validate_not_truncated(file: &Path) {
    let bytes = match std::fs::read(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => panic!("unreadable settings file {}: {e}", file.display()),
        Ok(b) => b,
    };
    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&bytes);
    match parsed {
        Ok(value) => {
            assert!(
                value.get("selected_endpoint_id").is_some(),
                "complete document expected in {}",
                file.display()
            );
        }
        Err(e) => panic!("truncated/corrupt settings file at {}: {e}", file.display()),
    }
}
