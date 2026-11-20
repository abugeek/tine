//! Windows sleep / modern-standby suspend & resume notification watcher.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMECRITICAL, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

/// Power state transition reported by Windows power notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    /// System is entering sleep or modern standby.
    Suspending,
    /// System resumed from sleep or modern standby.
    Resumed,
}

/// RAII registration for Windows suspend/resume notifications.
pub struct PowerWatcher {
    handle: HPOWERNOTIFY,
    _params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
    ctx_raw: *const Sender<PowerEvent>,
}

// SAFETY: `PowerWatcher` owns the heap-pinned registration parameters and only
// calls `PowerUnregisterSuspendResumeNotification` on drop after which the
// thread-safe `Sender<PowerEvent>` Arc count is decremented.
unsafe impl Send for PowerWatcher {}

impl PowerWatcher {
    /// Registers a Windows suspend/resume callback forwarding [`PowerEvent`] to `tx`.
    ///
    /// Returns `None` if Windows refuses registration (non-fatal; health ticks still recover).
    #[must_use]
    pub fn start(tx: Sender<PowerEvent>) -> Option<Self> {
        let ctx_arc = Arc::new(tx);
        let ctx_raw = Arc::into_raw(ctx_arc);
        let params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_callback),
            Context: ctx_raw.cast::<c_void>().cast_mut(),
        });
        let mut reg_ptr: *mut c_void = std::ptr::null_mut();
        let recipient = HANDLE(std::ptr::from_ref(&*params).cast::<c_void>().cast_mut());
        // SAFETY: `params` is heap-allocated and remains pinned at the same address until
        // after `PowerUnregisterSuspendResumeNotification` completes in `Drop`.
        let err =
            unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, recipient, &raw mut reg_ptr) };
        if err.0 != 0 || reg_ptr.is_null() {
            // SAFETY: Registration failed; reclaim `ctx_raw` since callback will never run.
            unsafe { drop(Arc::from_raw(ctx_raw)) };
            tracing::warn!(target: "tine::power", win32 = err.0, "power notification registration failed");
            return None;
        }
        Some(Self {
            handle: HPOWERNOTIFY(reg_ptr as isize),
            _params: params,
            ctx_raw,
        })
    }
}

impl Drop for PowerWatcher {
    fn drop(&mut self) {
        // SAFETY: Unregister first so Windows guarantees no further callback invocations
        // before we reclaim `ctx_raw` and drop `_params`.
        unsafe {
            let _ = PowerUnregisterSuspendResumeNotification(self.handle);
            drop(Arc::from_raw(self.ctx_raw));
        }
    }
}

unsafe extern "system" fn power_callback(context: *const c_void, event_type: u32, _setting: *const c_void) -> u32 {
    if context.is_null() {
        return 0;
    }
    // SAFETY: `context` was created via `Arc::into_raw` in `PowerWatcher::start` and remains
    // live until after `PowerUnregisterSuspendResumeNotification` finishes in `Drop`.
    let tx = unsafe { &*context.cast::<Sender<PowerEvent>>() };
    let ev = match event_type {
        PBT_APMSUSPEND => Some(PowerEvent::Suspending),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => Some(PowerEvent::Resumed),
        _ => None,
    };
    if let Some(ev) = ev {
        let _ = tx.send(ev);
    }
    0
}
