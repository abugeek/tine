//! Single-instance guard and the named-pipe transport used by the CLI, the
//! pipe server, and secondary launches.
//!
//! - App mode holds the mutex `Local\Tine.SingleInstance` for its process
//!   lifetime.
//! - A second launch connects to `\\.\pipe\tine-<user-sid>` and sends its
//!   command, then exits.
//! - The pipe server (`spawn_server`) accepts newline-delimited JSON
//!   requests and answers with one JSON line. The pipe's DACL grants access
//!   to the current user only (see [`crate::security`]).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, w};

use crate::proto::{PIPE_UNLIMITED_INSTANCES, PipeRequest, PipeResponse};

/// Named-mutex name; `Local\` scope = this user's session only.
const MUTEX_NAME: PCWSTR = w!("Local\\Tine.SingleInstance");

/// Owns the single-instance mutex until dropped.
pub struct InstanceGuard {
    mutex: HANDLE,
    /// Named pipe bound to the current user's SID.
    pub pipe_name: String,
}

// SAFETY: `HANDLE` is closed from the owning thread on drop; the guard never
// crosses into an audio thread.
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

fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Buffers holding the current process's `TOKEN_USER` payload.
struct UserToken {
    /// 8-byte-aligned buffer returned by `GetTokenInformation`.
    buf: Vec<u64>,
}

impl UserToken {
    fn query() -> Option<Self> {
        let mut token = HANDLE::default();
        // SAFETY: `GetCurrentProcess()` returns a pseudo-handle; `token`
        // receives a real handle closed below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).ok()? };
        let mut needed = 0u32;
        // SAFETY: size query with a null buffer.
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
        // SAFETY: `buf` is 8-byte aligned and covers at least `needed` bytes
        // as reported by the size query.
        let ok =
            unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &raw mut needed) };
        // SAFETY: the token handle is no longer needed.
        unsafe {
            let _ = CloseHandle(token);
        }
        if ok.is_err() {
            return None;
        }
        Some(Self { buf })
    }

    /// The SID string (`S-1-…`) for the process user.
    fn sid_string(&self) -> Option<String> {
        // SAFETY: `buf` holds a valid `TOKEN_USER` populated by `GetTokenInformation`.
        let token_user = unsafe { &*(self.buf.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_pwstr = windows::core::PWSTR::null();
        // SAFETY: the SID lives inside `buf`, alive for the whole call; the
        // returned string is freed with `LocalFree` below.
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &raw mut sid_pwstr) }.ok()?;
        // SAFETY: `ConvertSidToStringSidW` allocated a NUL-terminated UTF-16
        // string with `LocalAlloc`.
        let sid_str = unsafe { sid_pwstr.to_string().ok() };
        // SAFETY: `sid_pwstr` was allocated by `ConvertSidToStringSidW`.
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(sid_pwstr.0.cast())));
        }
        sid_str
    }

    /// The SID bytes (for building the pipe DACL).
    fn sid_bytes(&self) -> Vec<u8> {
        // SAFETY: `buf` holds a valid `TOKEN_USER`.
        let token_user = unsafe { &*(self.buf.as_ptr().cast::<TOKEN_USER>()) };
        // SAFETY: the SID pointer stays valid while `buf` is alive; its
        // length comes from `GetLengthSid`.
        let len = unsafe { windows::Win32::Security::GetLengthSid(token_user.User.Sid) } as usize;
        // SAFETY: reading `len` bytes from the SID pointer inside `buf`.
        let slice = unsafe { std::slice::from_raw_parts(token_user.User.Sid.0.cast::<u8>(), len) };
        slice.to_vec()
    }
}

/// The current user's SID string, or `None` on failure.
#[must_use]
pub fn current_user_sid() -> Option<String> {
    UserToken::query().and_then(|t| t.sid_string())
}

/// The current user's SID bytes, or `None` on failure.
fn current_user_sid_bytes() -> Option<Vec<u8>> {
    UserToken::query().map(|t| t.sid_bytes())
}

/// `\\.\pipe\tine-<user SID>` (falls back to `\\.\pipe\tine-default`).
#[must_use]
pub fn pipe_name_for_user() -> String {
    let sid = current_user_sid().unwrap_or_else(|| "default".to_owned());
    format!(r"\\.\pipe\tine-{sid}")
}

/// Creates the mutex, reporting whether we are the primary instance.
fn acquire_locked() -> (Option<InstanceGuard>, bool) {
    let pipe_name = pipe_name_for_user();
    // SAFETY: `MUTEX_NAME` is a NUL-terminated static UTF-16 string.
    let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME) };
    let Ok(mutex) = mutex else {
        // Fallback: proceed as primary if mutex creation unexpectedly fails.
        return (
            Some(InstanceGuard {
                mutex: HANDLE::default(),
                pipe_name,
            }),
            true,
        );
    };
    // SAFETY: `GetLastError` reads the thread's last-error value set by
    // `CreateMutexW`.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        // SAFETY: `mutex` is a valid handle returned by `CreateMutexW` that
        // we intentionally abandon (the primary owns the mutex).
        unsafe {
            let _ = CloseHandle(mutex);
        }
        return (None, false);
    }
    (Some(InstanceGuard { mutex, pipe_name }), true)
}

/// Acquires the single-instance mutex, or forwards `open` to the running
/// instance (spec: "second launch signals the first").
pub fn acquire_or_forward_open() -> (Option<InstanceGuard>, bool) {
    let (guard, primary) = acquire_locked();
    if primary {
        return (guard, true);
    }
    // Secondary: no guard; tell the primary to show the window.
    let _ = send_request(&pipe_name_for_user(), &PipeRequest::Open);
    (None, false)
}

/// `--background` (autostart) variant: a secondary exits without forwarding,
/// so login never pops a window over the user's work.
pub fn acquire_quiet() -> (Option<InstanceGuard>, bool) {
    acquire_locked()
}

/// Sends one JSON request and waits for the one-line JSON reply.
///
/// # Errors
/// Returns a human-readable error when no instance answers or the reply is
/// malformed.
pub fn send_request(pipe_name: &str, req: &PipeRequest) -> Result<PipeResponse, String> {
    let wide = to_wide_null(pipe_name);
    let mut handle = None;
    for _attempt in 0..25 {
        // SAFETY: `wide` is NUL-terminated UTF-16; client-side open of an
        // existing pipe.
        let opened = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match opened {
            Ok(h) => {
                handle = Some(h);
                break;
            }
            Err(err) => {
                if err.code() == ERROR_FILE_NOT_FOUND.to_hresult() {
                    // The pipe does not exist yet (tine.exe may just be
                    // starting): pace the retries instead of spinning.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                } else {
                    // SAFETY: bounded wait for the server to post a free
                    // instance.
                    let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 100) };
                }
            }
        }
    }
    let Some(handle) = handle else {
        return Err("tine.exe is not running".to_owned());
    };
    let result = request_on_handle(handle, req);
    // SAFETY: the conversation is done; close the client handle.
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

fn request_on_handle(handle: HANDLE, req: &PipeRequest) -> Result<PipeResponse, String> {
    let payload = crate::proto::encode_line(req);
    let mut written = 0u32;
    // SAFETY: `handle` is a connected pipe handle; `payload` is a valid slice.
    if unsafe { WriteFile(handle, Some(&payload), Some(&raw mut written), None) }.is_err() {
        return Err("failed to send the command to tine.exe".to_owned());
    }

    let mut buf: Vec<u8> = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        let mut read = 0u32;
        // SAFETY: `handle` is connected; `chunk` is a valid buffer.
        let ok = unsafe { ReadFile(handle, Some(&mut chunk), Some(&raw mut read), None) };
        if ok.is_err() || read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read as usize]);
        // Newline terminates the message; the cap only guards against a
        // runaway peer, so it is far above any reply we produce.
        if buf.contains(&b'\n') || buf.len() > 512 * 1024 {
            break;
        }
    }
    let line: Vec<u8> = buf.iter().copied().take_while(|&b| b != b'\n').collect();
    let text = String::from_utf8_lossy(&line).trim().to_owned();
    if text.is_empty() {
        return Err("tine.exe closed the connection before answering".to_owned());
    }
    serde_json::from_str::<PipeResponse>(&text).map_err(|e| format!("malformed reply from tine.exe: {e}"))
}

/// One connection's parsed request with its reply channel.
pub struct PipeJob {
    /// Client command.
    pub request: PipeRequest,
    /// Server answer (sent back over the same pipe connection).
    pub reply: Sender<PipeResponse>,
}

/// Spawns the pipe server thread; every request becomes a [`PipeJob`] on
/// `jobs`. The thread runs until the process exits.
///
/// The pipe is created with a DACL granting access to the current user only.
pub fn spawn_server(pipe_name: String, jobs: Sender<PipeJob>) {
    let sid = current_user_sid_bytes();
    std::thread::Builder::new()
        .name("tine-pipe-server".to_owned())
        .spawn(move || server_loop(&pipe_name, &jobs, sid.as_deref()))
        .ok();
}

fn server_loop(pipe_name: &str, jobs: &Sender<PipeJob>, sid: Option<&[u8]>) {
    let wide = to_wide_null(pipe_name);
    loop {
        let Some(pipe) = create_user_pipe(&wide, sid) else {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        // SAFETY: `pipe` is a valid server-side pipe handle.
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok()
            // SAFETY: last-error read after `ConnectNamedPipe` (a client may
            // have connected between create and connect).
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            serve_connection(pipe, jobs);
        }
        // SAFETY: the connection is done; close before creating the next
        // instance.
        unsafe {
            let _ = CloseHandle(pipe);
        }
    }
}

/// Creates a named-pipe instance with a user-only DACL.
fn create_user_pipe(wide: &[u16], sid: Option<&[u8]>) -> Option<HANDLE> {
    let sec = crate::security::user_only_security_attributes(sid)?;
    // SAFETY: `wide` is NUL-terminated; `sec` holds a valid
    // `SECURITY_ATTRIBUTES`; parameters match the `CreateNamedPipeW` contract.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            Some(&raw const sec.attrs),
        )
    };
    (!pipe.is_invalid()).then_some(pipe)
}

fn serve_connection(pipe: HANDLE, jobs: &Sender<PipeJob>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    // Read until a newline arrives or the client stops writing.
    loop {
        let mut read = 0u32;
        // SAFETY: `pipe` is connected; `chunk` is a valid read buffer.
        let ok = unsafe { ReadFile(pipe, Some(&mut chunk), Some(&raw mut read), None) };
        if ok.is_err() || read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read as usize]);
        // Newline terminates the message; the cap only guards against a
        // runaway peer, so it is far above any request we produce.
        if buf.contains(&b'\n') || buf.len() > 512 * 1024 {
            break;
        }
    }
    // Newline-delimited JSON: serve exactly the first complete request per
    // connection (the protocol is one request, one response).
    let Some(line) = crate::proto::take_line(&mut buf) else {
        return; // no complete request line on this connection
    };
    let text = String::from_utf8_lossy(&line)
        .trim()
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let parsed: Option<PipeRequest> = serde_json::from_str(&text).ok();
    if let Some(request) = parsed {
        // Dedicated reply channel per request: the app loop answers exactly
        // one response (fast commands immediately, `soak` when it finishes).
        let (tx_reply, rx_reply) = std::sync::mpsc::channel::<PipeResponse>();
        if jobs
            .send(PipeJob {
                request,
                reply: tx_reply,
            })
            .is_ok()
        {
            let response = rx_reply
                .recv_timeout(std::time::Duration::from_secs(600))
                .unwrap_or_else(|_| PipeResponse::err(1, "tine.exe did not answer in time"));
            write_reply(pipe, &response);
        }
    } else if !text.is_empty() {
        write_reply(pipe, &PipeResponse::err(2, "unrecognized pipe command"));
    }
    // SAFETY: `pipe` is still connected; disconnect for the next client.
    unsafe {
        let _ = DisconnectNamedPipe(pipe);
    }
}

fn write_reply(pipe: HANDLE, response: &PipeResponse) {
    let mut payload = serde_json::to_vec(response).unwrap_or_default();
    payload.push(b'\n');
    let mut written = 0u32;
    // SAFETY: `pipe` is connected; `payload` is a valid slice.
    if unsafe { WriteFile(pipe, Some(&payload), Some(&raw mut written), None) }.is_ok() {
        // `DisconnectNamedPipe` discards bytes the client has not read yet.
        // The client reads the reply in slices; blocking until it has
        // drained everything keeps replies larger than one slice from
        // being truncated. Our clients always read through the trailing
        // newline, so this cannot block indefinitely against them; a
        // crashed client releases its handle and unblocks the flush too.
        // SAFETY: flushing a blocking pipe handle waits for the peer to
        // consume all written bytes.
        let _ = unsafe { FlushFileBuffers(pipe) };
    }
}

#[cfg(test)]
mod tests {
    use super::{PipeRequest, PipeResponse, pipe_name_for_user, send_request, spawn_server};
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn pipe_name_carries_user_or_fallback() {
        let name = pipe_name_for_user();
        assert!(name.starts_with(r"\\.\pipe\tine-"), "{name}");
    }

    /// Regression: a reply larger than the client's 512-byte read slice used
    /// to lose its tail — `DisconnectNamedPipe` discards bytes the client
    /// has not read, and the server disconnected right after writing.
    /// `write_reply` now drains (`FlushFileBuffers`) before the disconnect.
    #[test]
    fn large_reply_round_trips_intact() {
        let name = format!(r"\\.\pipe\tine-test-{}", std::process::id());
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_server(name.clone(), tx);

        // Echo a ~8 KiB payload back: `payload` mirrors it in the response.
        let filler = "x".repeat(8 * 1024);
        let request = PipeRequest::Status;
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv_timeout(Duration::from_secs(30)) {
                let _ = job.reply.send(PipeResponse::ok(json!({
                    "payload": filler.clone(),
                })));
            }
        });

        // Give the server thread a moment to create its first instance, then
        // ask (send_request retries for a while, so no strict timing needed).
        let resp = send_request(&name, &request).expect("server should answer");
        let err = resp.error.clone().unwrap_or_default();
        assert!(resp.ok, "expected ok, got {err}");
        let payload = resp.data.expect("payload present");
        let text = payload
            .get("payload")
            .and_then(|v| v.as_str())
            .expect("payload is a string");
        assert_eq!(text.len(), 8 * 1024, "payload must arrive untruncated");
        assert!(text.chars().all(|c| c == 'x'));
    }
}
