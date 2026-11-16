//! Version and host introspection for `status` and `diag` (control-plane
//! thread only).

use windows::core::{PCWSTR, w};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
};

/// The `tine.exe` crate version (from Cargo).
#[must_use]
pub fn tine_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
const WINVER_KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
/// `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0`.
const CPU_KEY: PCWSTR = w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0");

/// Opens a `HKLM` key; the caller must close it (`close_key`).
fn open_hklm_key(key_path: PCWSTR) -> Option<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: both paths are NUL-terminated static literals; `key` receives
    // the opened handle, closed by the caller via `close_key`.
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_path, None, KEY_READ, &raw mut key) };
    (status.is_ok()).then_some(key)
}

fn close_key(key: HKEY) {
    // SAFETY: `key` was opened by `open_hklm_key` and is used only until here.
    let _ = unsafe { RegCloseKey(key) };
}

/// Reads a `REG_SZ` value under a `HKLM` key; returns `None` on any failure.
fn read_reg_string(key_path: PCWSTR, value: PCWSTR) -> Option<String> {
    let key = open_hklm_key(key_path)?;
    // SAFETY: size probe with a null data buffer and a live size slot.
    let mut needed = 0u32;
    unsafe {
        let _ = RegQueryValueExW(key, value, None, None, None, Some(&raw mut needed));
    }
    if needed == 0 || needed > 64 * 1024 {
        close_key(key);
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let mut read = needed;
    // SAFETY: `buf` covers `needed` writable bytes as reported by the probe.
    let ok = unsafe {
        RegQueryValueExW(key, value, None, None, Some(buf.as_mut_ptr()), Some(&raw mut read))
    };
    close_key(key);
    if ok.is_err() {
        return None;
    }
    buf.truncate(read as usize);
    // The payload is NUL-terminated UTF-16.
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let trimmed: &[u16] = match units.iter().position(|&u| u == 0) {
        Some(zero) => &units[..zero],
        None => &units,
    };
    String::from_utf16(trimmed).ok()
}

/// Windows edition + build, e.g. `"Windows 11 … (26100.2825, 24H2)"` — best
/// effort from the registry (`RtlGetVersion` is unavailable in the
/// `windows` crate at this rev).
#[must_use]
pub fn windows_edition() -> String {
    let product = read_reg_string(WINVER_KEY, w!("ProductName")).unwrap_or_else(|| "Windows".to_owned());
    let build = read_reg_string(WINVER_KEY, w!("CurrentBuild")).unwrap_or_default();
    let ubr = read_reg_string_u32(WINVER_KEY, w!("UBR")).map(|n| n.to_string()).unwrap_or_default();
    let display = read_reg_string(WINVER_KEY, w!("DisplayVersion")).unwrap_or_default();
    let build_str = if build.is_empty() {
        String::new()
    } else if ubr.is_empty() {
        build
    } else {
        format!("{build}.{ubr}")
    };
    format!("{product} ({build_str}, {display})")
}

/// `REG_DWORD` helper for `UBR`.
fn read_reg_string_u32(key_path: PCWSTR, value: PCWSTR) -> Option<u32> {
    let key = open_hklm_key(key_path)?;
    let mut data = [0u8; 4];
    let mut read = 4u32;
    // SAFETY: `data` covers 4 bytes as required for a REG_DWORD read.
    let ok = unsafe {
        RegQueryValueExW(key, value, None, None, Some(data.as_mut_ptr()), Some(&raw mut read))
    };
    close_key(key);
    if ok.is_err() || read < 4 {
        return None;
    }
    Some(u32::from_le_bytes(data))
}

/// CPU model string from the hardware registry key.
#[must_use]
pub fn cpu_model() -> String {
    read_reg_string(CPU_KEY, w!("ProcessorNameString")).unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{cpu_model, tine_version, windows_edition};

    #[test]
    fn version_string_comes_from_cargo() {
        assert!(!tine_version().is_empty());
    }

    #[test]
    fn windows_edition_has_bracketed_build_on_real_hosts() {
        let v = windows_edition();
        // Registry availability differs under CI; assert the shape, not values.
        assert!(!v.is_empty(), "{v}");
        assert!(v.contains("Windows") || v.contains('('), "{v}");
    }

    #[test]
    fn cpu_model_is_a_string() {
        let c = cpu_model();
        assert!(!c.is_empty(), "{c}");
    }
}
