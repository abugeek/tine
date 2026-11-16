//! Single-instance guard via named mutex `Local\Tine.SingleInstance` and
//! named pipe `\\.\pipe\tine-<user-sid>`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR, w};

const MUTEX_NAME: PCWSTR = w!("Local\\Tine.SingleInstance");

pub(crate) struct InstanceGuard {
    mutex: HANDLE,
    pub(crate) pipe_name: String,
}

// SAFETY: `HANDLE` is an OS kernel handle that can be closed from the owning thread on drop.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.mutex.is_invalid() {
            // SAFETY: `self.mutex` is a valid handle returned by `CreateMutexW`.
            unsafe {
                let _ = CloseHandle(self.mutex);
            }
        }
    }
}

pub(crate) enum AcquireResult {
    Primary(InstanceGuard),
    SecondaryNotified,
}

fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn current_user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle; `token` receives the opened token handle.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).ok()?;
    }

    let mut needed = 0u32;
    // SAFETY: Querying required buffer size for `TokenUser` with a null buffer.
    unsafe {
        let _ = GetTokenInformation(token, TokenUser, None, 0, &raw mut needed);
    }
    if needed == 0 {
        // SAFETY: `token` is a valid handle opened above.
        unsafe {
            let _ = CloseHandle(token);
        }
        return None;
    }

    let mut buf = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` is 8-byte aligned and sized to at least `needed` bytes as reported by `GetTokenInformation`.
    let ok = unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &raw mut needed) };
    // SAFETY: `token` is no longer needed after `GetTokenInformation`.
    unsafe {
        let _ = CloseHandle(token);
    }
    ok.ok()?;

    // SAFETY: `buf` is 8-byte aligned and holds a valid `TOKEN_USER` structure populated by `GetTokenInformation`.
    let token_user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
    let mut sid_pwstr = PWSTR::null();
    // SAFETY: `token_user.User.Sid` is valid while `buf` is alive; `sid_pwstr` is freed via `LocalFree`.
    unsafe {
        ConvertSidToStringSidW(token_user.User.Sid, &raw mut sid_pwstr).ok()?;
    }
    // SAFETY: `ConvertSidToStringSidW` returns a NUL-terminated UTF-16 string allocated with `LocalAlloc`.
    let sid_str = unsafe { sid_pwstr.to_string().ok() };
    // SAFETY: `sid_pwstr` was allocated by `ConvertSidToStringSidW` and must be freed with `LocalFree`.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sid_pwstr.0.cast())));
    }
    sid_str
}

fn pipe_name_for_user() -> String {
    let sid = current_user_sid().unwrap_or_else(|| "default".to_owned());
    format!(r"\\.\pipe\tine-{sid}")
}

pub(crate) fn acquire_or_notify() -> AcquireResult {
    let pipe_name = pipe_name_for_user();
    // SAFETY: `MUTEX_NAME` is a valid NUL-terminated static UTF-16 string.
    let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME) };
    let Ok(mutex) = mutex else {
        // Fallback: proceed as primary if mutex creation unexpectedly fails.
        return AcquireResult::Primary(InstanceGuard {
            mutex: HANDLE::default(),
            pipe_name,
        });
    };

    // SAFETY: `GetLastError` reads the calling thread's last-error code set by `CreateMutexW`.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        // SAFETY: `mutex` is a valid handle returned by `CreateMutexW`.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        send_open_to_primary(&pipe_name);
        return AcquireResult::SecondaryNotified;
    }

    AcquireResult::Primary(InstanceGuard { mutex, pipe_name })
}

fn send_open_to_primary(pipe_name: &str) {
    let wide = to_wide_null(pipe_name);
    for attempt in 0..5 {
        // SAFETY: `wide` is NUL-terminated UTF-16.
        let opened = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        if let Ok(handle) = opened {
            let mut written = 0u32;
            // SAFETY: `handle` is a valid pipe client handle; `b"open\n"` is valid for reading.
            unsafe {
                let _ = WriteFile(handle, Some(b"open\n"), Some(&raw mut written), None);
                let _ = CloseHandle(handle);
            }
            return;
        }
        if attempt < 4 {
            // SAFETY: `wide` is NUL-terminated UTF-16.
            unsafe {
                let _ = WaitNamedPipeW(PCWSTR(wide.as_ptr()), 200);
            }
        }
    }
}

pub(crate) fn spawn_pipe_listener(pipe_name: String, on_open: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("tine-pipe-listener".to_owned())
        .spawn(move || {
            let wide = to_wide_null(&pipe_name);
            loop {
                // SAFETY: `wide` is NUL-terminated UTF-16; parameters match `CreateNamedPipeW` contract.
                let pipe = unsafe {
                    CreateNamedPipeW(
                        PCWSTR(wide.as_ptr()),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        1,
                        256,
                        256,
                        0,
                        None,
                    )
                };
                if pipe.is_invalid() || ptr::eq(pipe.0, ptr::null()) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }

                // SAFETY: `pipe` is a valid server pipe handle.
                let connected =
                    unsafe { ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED };
                if connected {
                    let mut buf = [0u8; 64];
                    let mut read = 0u32;
                    // SAFETY: `pipe` is connected and `buf` is valid for `buf.len()` bytes.
                    let ok = unsafe { ReadFile(pipe, Some(&mut buf), Some(&raw mut read), None).is_ok() };
                    if ok {
                        let cmd = std::str::from_utf8(&buf[..read as usize]).unwrap_or("").trim();
                        if cmd == "open" {
                            on_open();
                        }
                    }
                    // SAFETY: `pipe` is connected.
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe);
                    }
                }
                // SAFETY: `pipe` is a valid handle.
                unsafe {
                    let _ = CloseHandle(pipe);
                }
            }
        })
        .ok();
}
