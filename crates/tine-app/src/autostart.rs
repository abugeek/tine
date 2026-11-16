//! Autostart: the `HKCU\…\Run` value `Tine` = `"<exe>" --background`,
//! toggled from the tray (spec) and reported by `tine status`.
//!
//! The registry is the source of truth: the engine's `settings.json` sweep
//! (controller persistence) is not raced for this field by design; see the
//! task Result notes. `HKEY_CURRENT_USER` writes need no elevation.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::w;
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ,
};

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// Value name inside the `Run` key.
const VALUE_NAME: windows::core::PCWSTR = w!("Tine");

/// Reads the autostart state (`true` when the `Tine` value exists).
///
/// # Errors
/// Returns the Win32 error text on registry failures (a missing value is a
/// plain `Ok(false)`, not an error).
pub fn is_enabled() -> Result<bool, String> {
    let key = open_run_key(KEY_READ)?;
    // SAFETY: `key` is a valid open `Run` key handle; this is an existence
    // probe (null data pointer, live size slot, per the API contract).
    let mut size = 0u32;
    let status = unsafe { RegQueryValueExW(key, VALUE_NAME, None, None, None, Some(&raw mut size)) };
    close_key(key);
    Ok(status == ERROR_SUCCESS)
}

/// Enables or disables autostart.
///
/// # Errors
/// Returns the Win32 error text on registry failure.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if !on {
        let key = open_run_key(KEY_SET_VALUE)?;
        // SAFETY: `key` is a valid open `Run` key handle.
        let status = unsafe { RegDeleteValueW(key, VALUE_NAME) };
        close_key(key);
        return check(status, 2); // 2 = ERROR_FILE_NOT_FOUND is fine too.
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Quoted exe path + the background flag: no window at login, tray only.
    let data = format!("\"{}\" --background", exe.display());
    let wide: Vec<u16> = OsStr::new(&data).encode_wide().collect();
    // SAFETY: `wide` stays alive for the call; REG_SZ payload is its raw
    // little-endian UTF-16 bytes.
    let payload =
        unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * size_of::<u16>()) };
    let key = open_run_key(KEY_SET_VALUE)?;
    // SAFETY: `key` is valid and `payload` matches the REG_SZ contract.
    let status = unsafe { RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(payload)) };
    close_key(key);
    check(status, u32::MAX)
}

/// The command line the Run value would hold right now (display + tests).
#[must_use]
pub fn command_line_for_current_exe() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" --background", exe.display())
}

/// Opens `HKCU\…\Run` (closes never; callers use [`close_key`]).
fn open_run_key(access: REG_SAM_FLAGS) -> Result<HKEY, String> {
    let mut key = HKEY::default();
    // SAFETY: `RUN_KEY` is a NUL-terminated static string; `key` receives
    // the opened handle, closed by the callers via `close_key`.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &raw mut key) };
    if status != ERROR_SUCCESS {
        return Err(format!("RegOpenKeyExW failed: {}", status.0));
    }
    Ok(key)
}

fn close_key(key: HKEY) {
    // SAFETY: `key` was opened by `open_run_key` and is used only until here.
    let _ = unsafe { RegCloseKey(key) };
}

fn check(status: WIN32_ERROR, also_ok: u32) -> Result<(), String> {
    if status == ERROR_SUCCESS || status.0 == also_ok {
        Ok(())
    } else {
        Err(format!("registry write failed: {status:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::command_line_for_current_exe;

    #[test]
    fn command_line_quotes_the_exe() {
        let line = command_line_for_current_exe();
        assert!(line.starts_with('"'), "{line}");
        assert!(line.ends_with("\" --background"), "{line}");
    }

    #[test]
    #[ignore = "registry side effect; re-enable to verify by hand"]
    fn round_trip_enabled_and_disabled() {
        super::set_enabled(true).unwrap_or(());
        assert!(super::is_enabled().unwrap_or(false));
        super::set_enabled(false).unwrap_or(());
        assert!(!super::is_enabled().unwrap_or(true));
    }
}
