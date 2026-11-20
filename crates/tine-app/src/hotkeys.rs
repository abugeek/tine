//! Global Windows hotkeys (`Ctrl+Shift+M` mute, `Ctrl+Shift+B` bypass)
//! running on a dedicated blocking `GetMessageW` Win32 loop (zero polling,
//! clean shutdown on `Drop`).

use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    VK_DELETE, VK_END, VK_F1, VK_HOME, VK_INSERT, VK_NEXT, VK_PAUSE, VK_PRIOR, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

const HOTKEY_ID_MUTE: i32 = 1;
const HOTKEY_ID_BYPASS: i32 = 2;

/// Action emitted when a registered global hotkey is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Toggle click-free RT output mute.
    ToggleMute,
    /// Toggle global DSP A/B bypass.
    ToggleBypass,
}

/// Parsed Win32 modifier bitmask (with `MOD_NOREPEAT`) and virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeySpec {
    /// `RegisterHotKey` modifier flags (`MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT`, ...).
    pub modifiers: u32,
    /// Win32 virtual-key code (`VK_*`).
    pub vk: u32,
}

/// Parses a human-readable hotkey shortcut such as `"Ctrl+Shift+M"` or `"Alt+F9"`.
///
/// Empty / whitespace strings return `None` (hotkey disabled). Always sets
/// `MOD_NOREPEAT` so holding a key does not flood the shell loop.
#[must_use]
pub fn parse_hotkey(spec: &str) -> Option<HotkeySpec> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut modifiers = MOD_NOREPEAT.0;
    let mut vk: Option<u32> = None;

    for raw_token in trimmed.split('+') {
        let token = raw_token.trim();
        if token.is_empty() {
            return None;
        }
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL.0,
            "shift" => modifiers |= MOD_SHIFT.0,
            "alt" => modifiers |= MOD_ALT.0,
            "win" | "super" => modifiers |= MOD_WIN.0,
            _ => {
                if vk.is_some() {
                    return None;
                }
                vk = parse_vk(&lower);
                vk?;
            }
        }
    }

    // Require a key and at least one modifier so bare letters don't steal typing.
    if modifiers == MOD_NOREPEAT.0 {
        return None;
    }
    Some(HotkeySpec { modifiers, vk: vk? })
}

fn parse_vk(lower: &str) -> Option<u32> {
    if lower.len() == 1 {
        let b = lower.as_bytes()[0];
        if b.is_ascii_lowercase() {
            return Some(u32::from(b.to_ascii_uppercase()));
        }
        if b.is_ascii_digit() {
            return Some(u32::from(b));
        }
    }
    if let Some(num) = lower.strip_prefix('f').and_then(|s| s.parse::<u32>().ok())
        && (1..=24).contains(&num)
    {
        return Some(u32::from(VK_F1.0) + (num - 1));
    }
    match lower {
        "space" => Some(u32::from(VK_SPACE.0)),
        "pause" => Some(u32::from(VK_PAUSE.0)),
        "insert" | "ins" => Some(u32::from(VK_INSERT.0)),
        "delete" | "del" => Some(u32::from(VK_DELETE.0)),
        "home" => Some(u32::from(VK_HOME.0)),
        "end" => Some(u32::from(VK_END.0)),
        "pageup" | "pgup" => Some(u32::from(VK_PRIOR.0)),
        "pagedown" | "pgdn" => Some(u32::from(VK_NEXT.0)),
        _ => None,
    }
}

/// Owns the background Win32 `RegisterHotKey` message thread and unregisters cleanly on drop.
pub struct HotkeyWatcher {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl HotkeyWatcher {
    /// Starts the global hotkey watcher for `mute_spec` and `bypass_spec`.
    ///
    /// If neither string parses to a valid shortcut (e.g., both disabled), returns `None`
    /// without spawning a thread. If another application already owns a hotkey, logs a
    /// warning and continues running cleanly.
    #[must_use]
    pub fn start(mute_spec: &str, bypass_spec: &str, tx: Sender<HotkeyAction>) -> Option<Self> {
        let mute = parse_hotkey(mute_spec);
        let bypass = parse_hotkey(bypass_spec);
        if mute.is_none() && bypass.is_none() {
            return None;
        }
        let mute_raw = mute_spec.to_owned();
        let bypass_raw = bypass_spec.to_owned();
        let (tid_tx, tid_rx) = std::sync::mpsc::sync_channel::<u32>(1);

        let Ok(join) = thread::Builder::new()
            .name("tine-hotkeys".to_owned())
            .spawn(move || hotkey_thread(mute, &mute_raw, bypass, &bypass_raw, &tx, &tid_tx))
        else {
            tracing::warn!(target: "tine::hotkeys", "failed to spawn global hotkey thread");
            return None;
        };

        let Ok(thread_id) = tid_rx.recv() else {
            let _ = join.join();
            return None;
        };

        Some(Self {
            thread_id,
            join: Some(join),
        })
    }
}

impl Drop for HotkeyWatcher {
    fn drop(&mut self) {
        if self.thread_id != 0 {
            // SAFETY: `self.thread_id` belongs to our `tine-hotkeys` Win32 thread
            // which already initialized its message queue via `PeekMessageW`.
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn hotkey_thread(
    mute: Option<HotkeySpec>,
    mute_raw: &str,
    bypass: Option<HotkeySpec>,
    bypass_raw: &str,
    tx: &Sender<HotkeyAction>,
    tid_tx: &std::sync::mpsc::SyncSender<u32>,
) {
    let mut msg = MSG::default();
    // Force Win32 message-queue creation on this thread before publishing its TID.
    // SAFETY: `msg` is valid writable stack memory; `PM_NOREMOVE` is non-destructive.
    let _ = unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_NOREMOVE) };

    let mut mute_registered = false;
    let mut bypass_registered = false;

    if let Some(spec) = mute {
        // SAFETY: Thread-associated hotkey (`hwnd = None`); unregistered before thread exit.
        match unsafe { RegisterHotKey(None, HOTKEY_ID_MUTE, HOT_KEY_MODIFIERS(spec.modifiers), spec.vk) } {
            Ok(()) => {
                mute_registered = true;
                tracing::info!(target: "tine::hotkeys", shortcut = mute_raw, "registered mute hotkey");
            }
            Err(e) => {
                tracing::warn!(target: "tine::hotkeys", shortcut = mute_raw, error = %e, "could not register mute hotkey");
            }
        }
    }

    if let Some(spec) = bypass {
        // SAFETY: Thread-associated hotkey (`hwnd = None`); unregistered before thread exit.
        match unsafe { RegisterHotKey(None, HOTKEY_ID_BYPASS, HOT_KEY_MODIFIERS(spec.modifiers), spec.vk) } {
            Ok(()) => {
                bypass_registered = true;
                tracing::info!(target: "tine::hotkeys", shortcut = bypass_raw, "registered bypass hotkey");
            }
            Err(e) => {
                tracing::warn!(target: "tine::hotkeys", shortcut = bypass_raw, error = %e, "could not register bypass hotkey");
            }
        }
    }

    // SAFETY: `GetCurrentThreadId` takes no pointers and always returns this thread's Win32 TID.
    let tid = unsafe { GetCurrentThreadId() };
    let _ = tid_tx.send(tid);

    if !mute_registered && !bypass_registered {
        return;
    }

    loop {
        // SAFETY: Blocks in `GetMessageW` until a `WM_HOTKEY` or `WM_QUIT` message arrives.
        let ret = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        if msg.message == WM_HOTKEY {
            let action = match i32::try_from(msg.wParam.0).ok() {
                Some(HOTKEY_ID_MUTE) => Some(HotkeyAction::ToggleMute),
                Some(HOTKEY_ID_BYPASS) => Some(HotkeyAction::ToggleBypass),
                _ => None,
            };
            if let Some(act) = action
                && tx.send(act).is_err()
            {
                break;
            }
        }
    }

    if mute_registered {
        // SAFETY: Registered on this thread above.
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID_MUTE) };
    }
    if bypass_registered {
        // SAFETY: Registered on this thread above.
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID_BYPASS) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hotkey_accepts_modifiers_letters_digits_and_fkeys() {
        let mute = parse_hotkey("Ctrl+Shift+M").expect("Ctrl+Shift+M must parse");
        assert_eq!(mute.modifiers, MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0);
        assert_eq!(mute.vk, u32::from(b'M'));

        let fkey = parse_hotkey("  alt + f12 ").expect("Alt+F12 must parse case-insensitively");
        assert_eq!(fkey.modifiers, MOD_ALT.0 | MOD_NOREPEAT.0);
        assert_eq!(fkey.vk, u32::from(VK_F1.0) + 11);

        let digit = parse_hotkey("Ctrl+Win+0").expect("Ctrl+Win+0 must parse");
        assert_eq!(digit.modifiers, MOD_CONTROL.0 | MOD_WIN.0 | MOD_NOREPEAT.0);
        assert_eq!(digit.vk, u32::from(b'0'));

        let named = parse_hotkey("Shift+Pause").expect("Shift+Pause must parse");
        assert_eq!(named.modifiers, MOD_SHIFT.0 | MOD_NOREPEAT.0);
        assert_eq!(named.vk, u32::from(VK_PAUSE.0));
    }

    #[test]
    fn parse_hotkey_rejects_empty_bare_key_or_malformed_specs() {
        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("   "), None);
        assert_eq!(parse_hotkey("M"), None, "bare key without modifiers must be rejected");
        assert_eq!(parse_hotkey("Ctrl+Shift"), None, "missing key");
        assert_eq!(parse_hotkey("Ctrl++M"), None, "empty token");
        assert_eq!(parse_hotkey("Ctrl+F25"), None, "out of range F-key");
        assert_eq!(parse_hotkey("Ctrl+A+B"), None, "multiple non-modifier keys");
    }
}
