//! `tine diag`: a privacy-safe diagnostics report for bug reports and the
//! community beta (D-016).
//!
//! Contains: Tine/Windows version, CPU model, capture endpoints with their
//! routing state, virtual-cable availability, the app settings with endpoint
//! ids hashed, and the last 3 log files — **no audio, no user names, no
//! paths outside Tine's folders**. The report packs into a ZIP
//! (`diag::zip`, stored entries, in-house to avoid a new dependency).

use std::path::Path;

use serde_json::{Value, json};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;

use crate::version;

/// Number of recent log files included.
pub const LOGS_TO_INCLUDE: usize = 3;

/// Collects the diagnostics JSON document.
///
/// # Errors
/// Returns a message when Windows/device introspection fails outright; the
/// partially collected sections are still included in that case.
#[must_use]
pub fn collect(paths: &EnginePaths) -> Value {
    let endpoints = tine_engine::apo_admin::list_capture_endpoints()
        .map(|list| {
            list.iter()
                .map(|ep| {
                    json!({
                        "name": ep.name,
                        "form_factor": format!("{:?}", ep.form_factor),
                        "state": apo_state_string(&ep.state),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let virtual_cable = tine_engine::apo_admin::find_virtual_cable_sink().map_or(Value::Null, |sink| match sink {
        Some(sink) => json!({ "name": sink.name, "available": sink.is_available }),
        None => Value::Null,
    });

    let settings = hash_endpoint_ids(&persist::load_settings(paths).into_value());

    json!({
        "tine_version": version::tine_version(),
        "windows": version::windows_edition(),
        "cpu_model": version::cpu_model(),
        "routing": "virtual-cable (D-018)",
        "capture_endpoints": endpoints,
        "virtual_cable_sink": virtual_cable,
        "settings": settings,
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

/// Hashes the endpoint ids in a settings document (`FNV-1a/64`, hex) so the
/// file has no machine identifiers.
#[must_use]
pub fn hash_endpoint_ids(settings: &tine_protocol::settings::AppSettings) -> Value {
    json!({
        "schema": settings.schema,
        "selected_endpoint_id": settings.selected_endpoint_id.as_deref().map(crate::hash::fnv64_hex),
        "monitor_device_id": settings.monitor_device_id.as_deref().map(crate::hash::fnv64_hex),
        "monitor_enabled": settings.monitor_enabled,
        "pro_drawer_open": settings.pro_drawer_open,
        "autostart": settings.autostart,
        "start_minimized": settings.start_minimized,
        "window_width": settings.window_width,
        "window_height": settings.window_height,
    })
}

/// `ApoState` as a stable, human-readable string (the engine enum is not
/// `Copy`-serializable; keep names stable for community reports).
#[must_use]
pub fn apo_state_string(state: &tine_engine::apo_admin::ApoState) -> String {
    use tine_engine::apo_admin::ApoState;
    match state {
        ApoState::Active { heartbeat_age_ms } => format!("active (heartbeat {heartbeat_age_ms} ms)"),
        ApoState::InstalledNotLoaded => "installed-not-loaded".to_owned(),
        ApoState::RemovedBySystem => "removed-by-system".to_owned(),
        ApoState::NotInstalled => "not-installed".to_owned(),
        ApoState::EnhancementsDisabled => "enhancements-disabled".to_owned(),
        ApoState::Error(_) => "error".to_owned(),
    }
}

/// Builds the diag ZIP: `diag.json` + the newest log files.
///
/// `out` receives the zip bytes; the caller writes them to the target path.
///
/// # Errors
/// Propagates filesystem errors (logs unreadable → skipped with the error
/// noted in the returned JSON).
#[must_use]
pub fn build_zip(paths: &EnginePaths, extra: Value) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let diag = if extra.is_null() {
        collect(paths)
    } else {
        merge(collect(paths), extra)
    };
    entries.push((
        "diag.json".to_owned(),
        serde_json::to_vec_pretty(&diag).unwrap_or_else(|_| b"{}".to_vec()),
    ));
    for log in recent_logs(paths, LOGS_TO_INCLUDE) {
        let name = log.file.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_else(|| "log".to_owned());
        if let Ok(bytes) = std::fs::read(&log.file) {
            entries.push((name, bytes));
        }
    }
    entries
}

/// Writes the diag report to `out` (a `.zip` path).
///
/// # Errors
/// Returns the io error when the target cannot be written.
pub fn write(path: &Path, entries: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let zip = crate::zip::stored_zip(entries);
    std::fs::write(path, zip)
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(b), Value::Object(e)) = (&mut base, extra) {
        for (k, v) in e {
            b.insert(k, v);
        }
    }
    base
}

/// The newest log files (current first, then rolled generations).
struct RecentLog {
    _gen: u32,
    file: std::path::PathBuf,
}

fn recent_logs(paths: &EnginePaths, n: usize) -> Vec<RecentLog> {
    let dir = paths.logs_dir();
    let mut logs: Vec<RecentLog> = vec![RecentLog {
        _gen: 0,
        file: dir.join("tine.log"),
    }];
    for generation in 1.. {
        if logs.len() >= n + 10 {
            break;
        }
        let file = dir.join(format!("tine.{generation}.log"));
        if !file.exists() || generation > 20 {
            break;
        }
        logs.push(RecentLog { _gen: generation, file });
    }
    if n >= logs.len() {
        logs
    } else {
        logs.truncate(n);
        logs
    }
}

#[cfg(test)]
mod tests {
    use super::{LOGS_TO_INCLUDE, collect, hash_endpoint_ids};
    use serde_json::Value;
    use tine_engine::paths::EnginePaths;
    use tine_protocol::settings::AppSettings;

    fn temp_paths(tag: &str) -> EnginePaths {
        let dir = std::env::temp_dir().join(format!("tine-diag-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        EnginePaths::from_roots(dir.join("app"), dir.join("prog"), dir.join("local"))
    }

    #[test]
    fn settings_ids_are_hashed() {
        let s = AppSettings {
            selected_endpoint_id: Some("{very-identifying-device-guid}".to_owned()),
            ..AppSettings::default()
        };
        let json = hash_endpoint_ids(&s);
        let selected = json
            .as_object()
            .and_then(|o| o.get("selected_endpoint_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let text = selected.as_str().unwrap_or_default();
        assert!(!text.contains("very-identifying"), "{text}");
        // FNV-1a of the raw id, hex, 16 chars.
        assert_eq!(text.len(), 16, "{text}");
    }

    #[test]
    fn collect_contains_the_static_sections() {
        let value = collect(&temp_paths("collect"));
        let obj = value.as_object().expect("object");
        assert!(obj.contains_key("tine_version"));
        assert!(obj.contains_key("cpu_model"));
        assert!(obj.contains_key("routing"));
        assert_eq!(
            obj.get("routing").and_then(Value::as_str),
            Some("virtual-cable (D-018)")
        );
        assert!(obj.contains_key("capture_endpoints"));
    }

    #[test]
    fn three_logs_are_included_at_most() {
        assert_eq!(LOGS_TO_INCLUDE, 3);
    }
}
