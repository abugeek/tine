//! Windows `%APPDATA%`, `%LOCALAPPDATA%`, and `%ProgramData%` paths.
//!
//! All known-folder look-ups go through `SHGetKnownFolderPath` (no `dirs`
//! crate, D-013). File names embed the sanitized endpoint ID from
//! `tine_protocol::ids::SanitizedId` so the same identifier is safe for both
//! named file mappings and file names.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tine_protocol::ids::SanitizedId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_RoamingAppData, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
};
use windows::core::PWSTR;

/// Errors returned when resolving known folders or composing Tine's paths.
#[derive(Debug, Error)]
pub enum PathsError {
    /// `SHGetKnownFolderPath` failed for a known folder.
    #[error("failed to resolve known folder {folder}")]
    KnownFolder {
        /// Human-readable folder name.
        folder: &'static str,
        /// Underlying Windows error.
        #[source]
        source: windows::core::Error,
    },
    /// The known folder resolved to an empty path.
    #[error("known folder {folder} resolved to an empty path")]
    Empty {
        /// Human-readable folder name.
        folder: &'static str,
    },
    /// The known folder resolved to a path that is not valid UTF-16.
    #[error("known folder {folder} is not valid UTF-16: {source}")]
    Encoding {
        /// Human-readable folder name.
        folder: &'static str,
        /// Underlying conversion error.
        #[source]
        source: std::string::FromUtf16Error,
    },
}

/// RAII guard for the `CoTaskMem`-allocated wide string returned by
/// `SHGetKnownFolderPath`.
struct KnownPath(PWSTR);

impl KnownPath {
    /// Copies the NUL-terminated wide string into a [`PathBuf`]; the buffer is
    /// freed on drop.
    fn to_path_buf(&self, folder: &'static str) -> Result<PathBuf, PathsError> {
        // SAFETY: `SHGetKnownFolderPath` returned a valid NUL-terminated PWSTR.
        let len = unsafe { self.0.len() };
        if len == 0 {
            return Err(PathsError::Empty { folder });
        }
        // SAFETY: The pointer is valid for `len` UTF-16 code units.
        match unsafe { self.0.to_string() } {
            Ok(s) => Ok(PathBuf::from(s)),
            Err(source) => Err(PathsError::Encoding { folder, source }),
        }
    }
}

impl Drop for KnownPath {
    fn drop(&mut self) {
        // SAFETY: the pointer was allocated by `SHGetKnownFolderPath` with
        // `CoTaskMemAlloc` and is freed exactly once here.
        unsafe { CoTaskMemFree(Some(self.0.as_ptr() as *const c_void)) };
    }
}

/// Resolves one known folder to a [`PathBuf`].
fn known_folder(guid: &windows::core::GUID, folder: &'static str) -> Result<PathBuf, PathsError> {
    // SAFETY: `guid` points to a valid constant GUID; the returned PWSTR is
    // wrapped in `KnownPath` which frees it with `CoTaskMemFree`.
    let path = unsafe { SHGetKnownFolderPath(guid, KNOWN_FOLDER_FLAG(0), None) }
        .map_err(|source| PathsError::KnownFolder { folder, source })?;
    let guard = KnownPath(path);
    guard.to_path_buf(folder)
}

/// Root directories holding every file Tine persists.
///
/// In production this is built from [`EnginePaths::system`]; tests and
/// `devhost` inject temporary roots with [`EnginePaths::from_roots`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnginePaths {
    /// Roaming user data root (`%APPDATA%`); holds `Tine\settings.json`.
    pub app_data: PathBuf,
    /// Machine-wide data root (`%ProgramData%`); holds `Tine\endpoints\<id>.json`.
    pub program_data: PathBuf,
    /// Local (non-roaming) user data root (`%LOCALAPPDATA%`); holds logs.
    pub local_data: PathBuf,
}

impl EnginePaths {
    /// Resolves the real Windows known folders for roaming app data, local app
    /// data, and program data.
    pub fn system() -> Result<Self, PathsError> {
        Ok(Self {
            app_data: known_folder(&FOLDERID_RoamingAppData, "FOLDERID_RoamingAppData")?,
            local_data: known_folder(&FOLDERID_LocalAppData, "FOLDERID_LocalAppData")?,
            program_data: known_folder(&FOLDERID_ProgramData, "FOLDERID_ProgramData")?,
        })
    }

    /// Builds explicit roots (tests, `devhost`, diagnostics).
    #[must_use]
    pub fn from_roots(
        app_data: impl Into<PathBuf>,
        program_data: impl Into<PathBuf>,
        local_data: impl Into<PathBuf>,
    ) -> Self {
        Self {
            app_data: app_data.into(),
            program_data: program_data.into(),
            local_data: local_data.into(),
        }
    }

    /// `$app_data\Tine\settings.json`.
    #[must_use]
    pub fn settings_file(&self) -> PathBuf {
        self.app_data.join("Tine").join("settings.json")
    }

    /// `$app_data\Tine\profiles\<sanitized endpoint>.json`.
    #[must_use]
    pub fn profile_file(&self, endpoint_id: &str) -> PathBuf {
        self.profiles_dir()
            .join(format!("{}.json", SanitizedId::from_raw(endpoint_id)))
    }

    /// `$app_data\Tine\profiles`.
    #[must_use]
    pub fn profiles_dir(&self) -> PathBuf {
        self.app_data.join("Tine").join("profiles")
    }

    /// `$program_data\Tine\endpoints\<sanitized endpoint>.json` (APO boot params).
    #[must_use]
    pub fn boot_params_file(&self, endpoint_id: &str) -> PathBuf {
        self.boot_params_dir()
            .join(format!("{}.json", SanitizedId::from_raw(endpoint_id)))
    }

    /// `$program_data\Tine\endpoints`.
    #[must_use]
    pub fn boot_params_dir(&self) -> PathBuf {
        self.program_data.join("Tine").join("endpoints")
    }

    /// `$local_data\Tine\logs`.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.local_data.join("Tine").join("logs")
    }
}

/// Creates every parent directory of `file` that does not exist yet.
pub fn ensure_parent_dir(file: &Path) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::EnginePaths;
    use std::path::Path;

    #[test]
    fn config_paths_are_composed_and_sanitized() {
        let paths = EnginePaths::from_roots("/app", "/prog", "/local");
        assert_eq!(paths.settings_file(), Path::new("/app/Tine/settings.json"));
        assert_eq!(
            paths.profile_file("Wave:{bad chars}"),
            Path::new("/app/Tine/profiles/Wave_{bad_chars}.json")
        );
        assert_eq!(
            paths.boot_params_file("Wave:{bad chars}"),
            Path::new("/prog/Tine/endpoints/Wave_{bad_chars}.json")
        );
        assert_eq!(paths.logs_dir(), Path::new("/local/Tine/logs"));
    }

    #[test]
    fn system_paths_resolve_real_directories() {
        let paths = EnginePaths::system().ok();
        assert!(paths.is_some(), "known folders must resolve on Windows");
        if let Some(paths) = paths {
            let _ = std::fs::create_dir_all(paths.profiles_dir());
            assert!(paths.settings_file().starts_with(&paths.app_data));
        }
    }
}
