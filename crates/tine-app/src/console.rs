//! Console plumbing for CLI mode.
//!
//! Release builds use `windows_subsystem = "windows"`, so every CLI command
//! attaches to the parent's console (`AttachConsole(ATTACH_PARENT_PROCESS)`)
//! before anything is printed, then rebinds the process std handles to
//! `CONOUT$` so Rust's `println!` reaches the terminal. When the process was
//! launched with redirected std pipes (mintty, CI, `> file`), the inherited
//! handles are already valid and are left untouched.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    SetStdHandle,
};
use windows::core::{PCWSTR, w};

/// Set while the console Ctrl handler saw an interrupt (checked by long CLI
/// operations like `soak` so they can exit cleanly).
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// `CTRL_C_EVENT` / `CTRL_BREAK_EVENT` / `CTRL_CLOSE_EVENT` etc.
const CTRL_C_EVENT: u32 = 0;
const CTRL_BREAK_EVENT: u32 = 1;
const CTRL_CLOSE_EVENT: u32 = 2;
const CTRL_LOGOFF_EVENT: u32 = 5;
const CTRL_SHUTDOWN_EVENT: u32 = 6;

/// True once the user pressed Ctrl+C (or Break/close) in CLI mode.
#[must_use]
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

// SAFETY: the handler is a static function with the exact `PHANDLER_ROUTINE`
// signature; it only flips an atomic and claims the event so the CLI loop
// can exit on its own terms.
unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> windows::core::BOOL {
    if matches!(
        ctrl_type,
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // Claim the event: `soak` exits on its own terms; other fast commands
    // finish before the user can press Ctrl+C a second time.
    true.into()
}

/// Attaches to the parent console and installs the Ctrl handler.
///
/// Call once at CLI entry, before any stdout/stderr use. When the process
/// already has a console (debug builds) or inherited pipe std handles, the
/// std handles are left untouched.
pub fn attach_for_cli() {
    // Ctrl+C handling: cleanly interruptible CLI mode (T17 spec).
    // SAFETY: `ctrl_handler` is a valid `PHANDLER_ROUTINE` (static, C ABI).
    let _ = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) };

    if std::io::stdout().is_terminal() || std::io::stderr().is_terminal() {
        return; // debug build / inherited console / mintty pty: keep handles.
    }

    // GUI-subsystem process: attach to the invoker's console so `tine status`
    // prints where the user can see it.
    // SAFETY: `ATTACH_PARENT_PROCESS` is a constant process id sentinel.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok() {
        rebind_std(STD_OUTPUT_HANDLE, w!("CONOUT$"));
        rebind_std(STD_ERROR_HANDLE, w!("CONOUT$"));
    }
}

/// Replaces one process std handle with the given console pseudo-file.
fn rebind_std(which: STD_HANDLE, name: PCWSTR) {
    // SAFETY: `name` points to a NUL-terminated static string; the call only
    // opens a console pseudo-file. Ownership passes to the process std slot.
    let opened = unsafe {
        CreateFileW(
            name,
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
            None,
        )
    };
    if let Ok(handle) = opened {
        // SAFETY: `handle` is a valid console handle and `SetStdHandle` only
        // stores it into the process PEB std-handle slot.
        let _ = unsafe { SetStdHandle(which, handle) };
    }
}
