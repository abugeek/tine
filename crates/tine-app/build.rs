//! Link settings for `tine.exe`.
//!
//! One binary hosts both the always-on tray/router parent and the `--ui`
//! window child (D-017), so it imports the window stack's DLLs. Loaded
//! eagerly, `dxgi.dll` makes the *parent* look like a 3D application:
//! overlay/OSD hook DLLs (`RivaTuner` and similar) then set up per-process
//! state at the first window (the tray), which measured as a 51 MB private
//! commit, and the extra images add to the parent's working set (T24).
//!
//! Delay-loading the graphics/UI-only DLLs means the parent never maps them;
//! the `--ui` child loads each one on its first call. Imports with data or
//! TLS, and everything the parent uses on its hot path, stay eager.
//!
//! The app icon (`assets/brand/tine.ico`) is linked in as icon group
//! [`APP_ICON_ID`]: Explorer, the taskbar and the `--ui` window
//! (`rune::set_app_icon_resource`) all read it from there. The `.res` file is
//! written here directly (no `rc.exe`, no extra build dependency); MSVC
//! `link.exe` accepts `.res` inputs as they are.

use std::path::{Path, PathBuf};

/// Window/graphics-stack DLLs the background parent never calls.
const DELAY_LOAD: &[&str] = &[
    "dxgi.dll",
    "dwrite.dll",
    "uiautomationcore.dll",
    "dwmapi.dll",
    "imm32.dll",
];

/// Resource id of the app's icon group (must match `tine_app::ui_bridge::APP_ICON_ID`).
const APP_ICON_ID: u16 = 1;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !(windows && msvc) {
        return; // `/DELAYLOAD` and `.res` inputs are MSVC linker features (D-001 targets MSVC).
    }
    for dll in DELAY_LOAD {
        println!("cargo:rustc-link-arg-bin=tine=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bin=tine=delayimp.lib");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let ico = manifest_dir.join("../../assets/brand/tine.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    match write_icon_res(&ico, &out_dir.join("tine-icon.res")) {
        Ok(res) => println!("cargo:rustc-link-arg-bin=tine={}", res.display()),
        Err(e) => println!("cargo:warning=app icon not embedded: {e}"),
    }
}

/// Converts an `.ico` into a `.res` with one `RT_ICON` per frame and an
/// `RT_GROUP_ICON` directory naming them.
fn write_icon_res(ico: &Path, res: &Path) -> Result<PathBuf, String> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let bytes = std::fs::read(ico).map_err(|e| format!("{}: {e}", ico.display()))?;
    let u16_at = |at: usize| -> Result<u16, String> {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "truncated .ico".to_owned())
    };
    let u32_at = |at: usize| -> Result<u32, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "truncated .ico".to_owned())
    };
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return Err("not an .ico file".to_owned());
    }
    let count = u16_at(4)?;

    let mut out = Vec::new();
    // A .res file starts with an empty 32-byte entry.
    push_entry(&mut out, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&count.to_le_bytes());
    for i in 0..count {
        let dir = 6 + usize::from(i) * 16;
        let size = u32_at(dir + 8)? as usize;
        let offset = u32_at(dir + 12)? as usize;
        let data = bytes
            .get(offset..offset + size)
            .ok_or_else(|| "icon frame past end of file".to_owned())?;
        let id = i + 1;
        push_entry(&mut out, RT_ICON, id, 0x1010, data);
        // GRPICONDIRENTRY: the ICONDIRENTRY's first 12 bytes, then the frame's id.
        group.extend_from_slice(bytes.get(dir..dir + 12).ok_or("truncated .ico")?);
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_entry(&mut out, RT_GROUP_ICON, APP_ICON_ID, 0x1030, &group);
    std::fs::write(res, out).map_err(|e| format!("{}: {e}", res.display()))?;
    Ok(res.to_path_buf())
}

/// One `RESOURCEHEADER` with numeric type and name, then its data padded to 4 bytes.
fn push_entry(out: &mut Vec<u8>, ty: u16, name: u16, flags: u16, data: &[u8]) {
    let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&size.to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    if ty == 0 {
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF]);
    } else {
        out.extend_from_slice(&[0xFF, 0xFF]);
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&[0xFF, 0xFF]);
        out.extend_from_slice(&name.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId (en-US)
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}
