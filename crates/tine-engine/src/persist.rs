//! Atomic, versioned JSON persistence.
//!
//! All files are written with the same protocol: serialize to pretty JSON,
//! write to a sibling temp file, `FlushFileBuffers`, `CloseHandle`, then
//! `ReplaceFileW` (falling back to `MoveFileExW(REPLACE_EXISTING)` when the
//! target does not exist yet). A reader therefore always sees either the old
//! complete file or the new complete file — killing the process mid-write can
//! never truncate the target (T15 acceptance criterion).
//!
//! Loading always goes through a schema migration step: raw JSON is parsed
//! into a [`serde_json::Value`], migrated to the current schema
//! ([`SCHEMA_VERSION`]), only then deserialized into the typed struct and
//! validated. A file that cannot be parsed, migrated, or validated is moved
//! aside as `<file>.corrupt-<unix_ts>` and defaults are used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use tine_protocol::ids::SCHEMA_VERSION;
use tine_protocol::params::{DspParams, PARAM_COUNT};
use tine_protocol::presets::{EndpointProfile, ValidationError};
use tine_protocol::settings::AppSettings;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_MODE, FlushFileBuffers, MOVEFILE_REPLACE_EXISTING, MoveFileExW, REPLACE_FILE_FLAGS, ReplaceFileW,
    WriteFile,
};
use windows::core::PCWSTR;

use crate::paths::EnginePaths;

/// Errors returned while persisting or loading Tine's JSON files.
#[derive(Debug, Error)]
pub enum PersistError {
    /// A `std` I/O error.
    #[error("I/O error on {path:?}: {source}")]
    Io {
        /// File being read or written.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Serialization or deserialization failure.
    #[error("JSON error on {path:?}: {source}")]
    Json {
        /// File being read or written.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// A Win32 call failed during the atomic-write protocol.
    #[error("Win32 error in {context} on {path:?}: {source}")]
    Win {
        /// Win32 operation that failed.
        context: &'static str,
        /// File being written.
        path: PathBuf,
        /// Underlying Windows error.
        #[source]
        source: windows::core::Error,
    },
    /// The loaded file failed schema or range validation.
    #[error("validation error on {path:?}: {source}")]
    Validation {
        /// File being loaded.
        path: PathBuf,
        /// Underlying validation error.
        #[source]
        source: ValidationError,
    },
}

/// Outcome of loading a persisted file.
///
/// Every variant carries a usable value; `Missing`, `Restored`, and `Failed`
/// all fall back to defaults so callers never need to branch before use.
#[derive(Debug)]
pub enum Loaded<T> {
    /// The file existed, migrated cleanly, and validated.
    Fresh(T),
    /// The file does not exist; defaults are used.
    Missing(T),
    /// The file was corrupt (unparseable, unmigratable, or invalid); it was
    /// backed up as `<file>.corrupt-<ts>` and defaults are used.
    Restored(T),
    /// The file could not be read at all (e.g. I/O error); defaults are used
    /// and the error is reported for logging.
    Failed(T, PersistError),
}

impl<T> Loaded<T> {
    /// Returns the usable value regardless of origin.
    #[must_use]
    pub fn into_value(self) -> T {
        match self {
            Self::Fresh(v) | Self::Missing(v) | Self::Restored(v) | Self::Failed(v, _) => v,
        }
    }

    /// Returns `true` when the value came from a file that validated cleanly.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh(_))
    }
}

/// Encodes a path as a NUL-terminated UTF-16 buffer.
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    let mut buf: Vec<u16> = path.as_os_str().encode_wide().collect();
    buf.push(0);
    buf
}

/// Atomically replaces `path` with `bytes` (temp file + flush + `ReplaceFileW`).
///
/// The temp file is a sibling of `path` so the final rename stays on the same
/// volume and is atomic.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| PersistError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    let temp = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name()
            .map_or_else(|| "tine".to_owned(), |n| n.to_string_lossy().into_owned()),
        std::process::id()
    ));

    let io_err = |source: windows::core::Error| PersistError::Win {
        context: "write temp file",
        path: path.to_path_buf(),
        source,
    };

    // SAFETY: `wide()` produces a valid NUL-terminated path; the returned
    // handle is closed before `ReplaceFileW` and the handle is never leaked.
    let file = unsafe {
        CreateFileW(
            PCWSTR(wide(&temp).as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_MODE(0),
            None,
            FILE_CREATION_DISPOSITION(CREATE_ALWAYS.0),
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0),
            None,
        )
    }
    .map_err(io_err)?;

    // SAFETY: `file` is a valid handle and `bytes` outlives the call.
    unsafe { WriteFile(file, Some(bytes), None, None) }.map_err(io_err)?;

    // SAFETY: `file` is a valid handle; flushing ensures the data is on disk
    // before the rename, so even a power cut cannot produce a truncated target.
    unsafe { FlushFileBuffers(file) }.map_err(io_err)?;

    // SAFETY: `file` is a valid handle owned solely by this call.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(file) };

    // SAFETY: both paths are valid NUL-terminated strings.
    let replaced = unsafe {
        ReplaceFileW(
            PCWSTR(wide(path).as_ptr()),
            PCWSTR(wide(&temp).as_ptr()),
            PCWSTR::null(),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };
    match replaced {
        Ok(()) => Ok(()),
        Err(source) => {
            // `ReplaceFileW` never creates the target; fall back to a plain
            // rename when the target does not exist yet (first write).
            if !is_win32(&source, ERROR_FILE_NOT_FOUND) && !is_win32(&source, ERROR_PATH_NOT_FOUND) {
                return Err(PersistError::Win {
                    context: "ReplaceFileW",
                    path: path.to_path_buf(),
                    source,
                });
            }
            // SAFETY: both paths are valid NUL-terminated strings; `temp` exists.
            if let Err(source) = unsafe {
                MoveFileExW(
                    PCWSTR(wide(&temp).as_ptr()),
                    PCWSTR(wide(path).as_ptr()),
                    MOVEFILE_REPLACE_EXISTING,
                )
            } {
                return Err(PersistError::Win {
                    context: "MoveFileExW",
                    path: path.to_path_buf(),
                    source,
                });
            }
            Ok(())
        }
    }
}

/// Returns `true` when a Windows error wraps `code` via `HRESULT_FROM_WIN32`.
fn is_win32(source: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    source.code() == windows::core::HRESULT::from_win32(code.0)
}

/// Reads a file's raw bytes; `Ok(None)` when it does not exist. Parse errors
/// are deliberately *not* raised here: an unparseable file is "corrupt", and
/// corruption is handled by the caller (backup + defaults).
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, PersistError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PersistError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Parses raw bytes as JSON.
fn parse_bytes(path: &Path, bytes: &[u8]) -> Result<Value, PersistError> {
    serde_json::from_slice(bytes).map_err(|source| PersistError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Migrates a raw JSON document to the current schema in place.
///
/// Schema 0 is the pre-versioning format (no `schema` field); the migration
/// stamps it as schema 1, which is structurally identical to current files.
/// Newer schemas than [`SCHEMA_VERSION`] are rejected.
pub fn migrate_value(path: &Path, value: &mut Value) -> Result<(), PersistError> {
    let schema = value.get("schema").and_then(Value::as_u64).unwrap_or(0);
    if schema == 0 {
        // Document predates schema versioning; current layout == schema 1.
        if let Value::Object(map) = value {
            map.insert("schema".to_owned(), Value::from(SCHEMA_VERSION));
        }
        return Ok(());
    }
    if (1..=u64::from(SCHEMA_VERSION)).contains(&schema) {
        Ok(())
    } else {
        Err(PersistError::Validation {
            path: path.to_path_buf(),
            source: ValidationError::UnsupportedSchema {
                actual: schema.min(u64::from(u32::MAX)) as u32,
                max_supported: SCHEMA_VERSION,
            },
        })
    }
}

/// Moves a corrupt file aside as `<file>.corrupt-<unix_ts>`.
///
/// Failure to move the file is ignored: defaults are used either way, and the
/// backup is a best-effort diagnostic aid.
fn backup_corrupt(path: &Path) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let backup = path.with_extension(format!(
        "{}-corrupt-{ts}",
        path.extension()
            .map_or_else(String::new, |e| e.to_string_lossy().into_owned())
    ));
    let _ = std::fs::rename(path, backup);
}

/// Loads a typed, validated file with migration and corrupt-file recovery.
fn load_with<T, V>(path: &Path, default: T, validate: V) -> Loaded<T>
where
    T: DeserializeOwned,
    V: FnOnce(&T) -> Result<(), ValidationError>,
{
    let bytes = match read_bytes(path) {
        Ok(None) => return Loaded::Missing(default),
        Ok(Some(b)) => b,
        Err(source) => return Loaded::Failed(default, source),
    };

    let outcome: Result<T, PersistError> = (|| {
        let mut value = parse_bytes(path, &bytes)?;
        migrate_value(path, &mut value)?;
        let typed: T = serde_json::from_value(value).map_err(|source| PersistError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        if let Err(source) = validate(&typed) {
            return Err(PersistError::Validation {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(typed)
    })();

    match outcome {
        Ok(typed) => Loaded::Fresh(typed),
        Err(error) => {
            tracing::warn!(target: "tine::persist", path = ?path, error = ?error, "corrupt file; using defaults");
            // `path` exists (it was read successfully); move it aside.
            backup_corrupt(path);
            Loaded::Restored(default)
        }
    }
}

/// Serializes `value` and writes it atomically to `path`.
fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), PersistError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| PersistError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    atomic_write(path, &bytes)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Persists `%APPDATA%\Tine\settings.json`.
pub fn save_settings(settings: &AppSettings, paths: &EnginePaths) -> Result<(), PersistError> {
    atomic_write_json(&paths.settings_file(), settings)
}

/// Loads `%APPDATA%\Tine\settings.json` (defaults when missing or corrupt).
pub fn load_settings(paths: &EnginePaths) -> Loaded<AppSettings> {
    let path = paths.settings_file();
    load_with(&path, AppSettings::default(), AppSettings::validate)
}

// ---------------------------------------------------------------------------
// Endpoint profiles
// ---------------------------------------------------------------------------

/// Persists `%APPDATA%\Tine\profiles\<endpoint>.json`.
pub fn save_profile(profile: &EndpointProfile, paths: &EnginePaths) -> Result<(), PersistError> {
    atomic_write_json(&paths.profile_file(&profile.endpoint_id), profile)
}

/// Loads `%APPDATA%\Tine\profiles\<endpoint>.json` (defaults when missing or
/// corrupt).
pub fn load_profile(endpoint_id: &str, paths: &EnginePaths) -> Loaded<EndpointProfile> {
    let default = EndpointProfile::new(endpoint_id, tine_protocol::presets::Archetype::default());
    let path = paths.profile_file(endpoint_id);
    load_with(&path, default, EndpointProfile::validate)
}

// ---------------------------------------------------------------------------
// APO boot params
// ---------------------------------------------------------------------------

/// JSON document stored at `%ProgramData%\Tine\endpoints\<endpoint>.json`.
///
/// This is the format the APO reads at `LockForProcess` (D-004): the
/// last-written resolved parameters, so processing is correct even when
/// `tine.exe` is not running. `params` maps stable [`SPECS`] keys to values.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct BootParamsFile {
    /// Schema version (`1..=SCHEMA_VERSION`).
    pub schema: u32,
    /// Windows `MMDevice` endpoint ID this file belongs to.
    pub endpoint_id: String,
    /// Resolved parameters keyed by stable [`SPECS`] key.
    pub params: BTreeMap<String, f32>,
}

fn boot_params_file(endpoint_id: &str, params: &DspParams) -> BootParamsFile {
    let mut map = BTreeMap::new();
    for i in 0..PARAM_COUNT {
        if let Some(id) = tine_protocol::params::ParamId::from_index(i) {
            map.insert(id.spec().key.to_owned(), params.get(id));
        }
    }
    BootParamsFile {
        schema: tine_protocol::ids::SCHEMA_VERSION,
        endpoint_id: endpoint_id.to_owned(),
        params: map,
    }
}

/// Persists the APO boot params for `endpoint_id`.
pub fn save_boot_params(endpoint_id: &str, params: &DspParams, paths: &EnginePaths) -> Result<(), PersistError> {
    atomic_write_json(
        &paths.boot_params_file(endpoint_id),
        &boot_params_file(endpoint_id, params),
    )
}

/// Loads the APO boot params for `endpoint_id` ([`DspParams::DEFAULT`] when
/// missing or corrupt).
pub fn load_boot_params(endpoint_id: &str, paths: &EnginePaths) -> Loaded<DspParams> {
    let path = paths.boot_params_file(endpoint_id);
    match load_with(&path, BootParamsFile::default(), boot_params_is_well_formed) {
        Loaded::Fresh(file) => Loaded::Fresh(boot_params_into(&file)),
        Loaded::Missing(_) => Loaded::Missing(DspParams::DEFAULT),
        Loaded::Restored(_) => Loaded::Restored(DspParams::DEFAULT),
        Loaded::Failed(_, error) => Loaded::Failed(DspParams::DEFAULT, error),
    }
}

impl Default for BootParamsFile {
    fn default() -> Self {
        Self {
            schema: tine_protocol::ids::SCHEMA_VERSION,
            endpoint_id: String::new(),
            params: BTreeMap::new(),
        }
    }
}

fn boot_params_is_well_formed(file: &BootParamsFile) -> Result<(), ValidationError> {
    if file.schema == 0 || file.schema > SCHEMA_VERSION {
        return Err(ValidationError::UnsupportedSchema {
            actual: file.schema,
            max_supported: SCHEMA_VERSION,
        });
    }
    if file.endpoint_id.trim().is_empty() {
        return Err(ValidationError::EmptyField { field: "endpoint_id" });
    }
    Ok(())
}

/// Applies a boot-params document onto a [`DspParams`] snapshot.
///
/// Unknown keys are ignored; known keys are clamped by `DspParams::set`, so a
/// hand-edited file can never push a parameter out of range.
fn boot_params_into(file: &BootParamsFile) -> DspParams {
    let mut params = DspParams::DEFAULT;
    for i in 0..PARAM_COUNT {
        if let Some(id) = tine_protocol::params::ParamId::from_index(i)
            && let Some(v) = file.params.get(id.spec().key)
        {
            let raw = *v;
            params.set(id, raw);
        }
    }
    params
}
