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

/// Window/graphics-stack DLLs the background parent never calls.
const DELAY_LOAD: &[&str] = &[
    "dxgi.dll",
    "dwrite.dll",
    "uiautomationcore.dll",
    "dwmapi.dll",
    "imm32.dll",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !(windows && msvc) {
        return; // `/DELAYLOAD` is an MSVC linker feature (D-001 targets MSVC).
    }
    for dll in DELAY_LOAD {
        println!("cargo:rustc-link-arg-bin=tine=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bin=tine=delayimp.lib");
}
