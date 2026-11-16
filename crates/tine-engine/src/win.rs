//! Win32, COM, and WASAPI (`MMDevice`) helpers.
//!
//! All `unsafe` Win32/COM FFI in `tine-engine` is confined to this module behind
//! RAII wrappers and safe Rust APIs. Non-RT control-plane code only.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use thiserror::Error;
use tine_protocol::presets::Archetype;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, E_NOINTERFACE, E_POINTER, HANDLE, RPC_E_CHANGED_MODE, S_FALSE, S_OK};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_UNPLUGGED, EDataFlow, ERole, IMMDevice,
    IMMDeviceCollection, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Vtbl, MMDeviceEnumerator,
    PKEY_AudioEndpoint_FormFactor, eCapture, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, PWSTR};

/// Errors returned by Win32 / COM / WASAPI wrappers.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WinError {
    /// A Windows COM or Win32 API call failed.
    #[error("Win32/COM error in {context}: {source}")]
    Com {
        /// Human-readable operation context.
        context: &'static str,
        /// Underlying Windows error.
        #[source]
        source: windows::core::Error,
    },
    /// Failed to spawn the background device-watcher thread.
    #[error("failed to spawn device watcher thread: {0}")]
    ThreadSpawn(String),
}

impl WinError {
    fn com(context: &'static str, source: windows::core::Error) -> Self {
        Self::Com { context, source }
    }
}

/// RAII guard for COM initialization on the current thread.
///
/// Calls `CoInitializeEx(COINIT_MULTITHREADED)` on construction and
/// `CoUninitialize()` on drop if this guard initialized COM. If the thread was
/// already initialized with a different apartment model (`RPC_E_CHANGED_MODE`),
/// construction succeeds without taking ownership of `CoUninitialize`.
pub struct ComInit {
    should_uninit: bool,
    _not_send_sync: PhantomData<*const ()>,
}

impl ComInit {
    /// Initializes COM in the multithreaded apartment (MTA) for the current thread.
    pub fn new_mta() -> Result<Self, WinError> {
        // SAFETY: `CoInitializeEx` with `None` reserved pointer is valid on any thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == S_OK || hr == S_FALSE {
            Ok(Self {
                should_uninit: true,
                _not_send_sync: PhantomData,
            })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self {
                should_uninit: false,
                _not_send_sync: PhantomData,
            })
        } else {
            Err(WinError::com("CoInitializeEx", windows::core::Error::from(hr)))
        }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.should_uninit {
            // SAFETY: Paired with `CoInitializeEx` returning `S_OK` or `S_FALSE` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// RAII wrapper for COM task-memory strings (`PWSTR`) allocated by `CoTaskMemAlloc`.
pub struct CoTaskMemPwstr(PWSTR);

impl CoTaskMemPwstr {
    /// Wraps a `PWSTR` returned by a COM API.
    ///
    /// # Safety
    /// `ptr` must be null or a valid pointer allocated by the COM task allocator
    /// (`CoTaskMemAlloc`) that has not yet been freed.
    #[must_use]
    pub unsafe fn from_raw(ptr: PWSTR) -> Self {
        Self(ptr)
    }

    /// Converts the UTF-16 string to a Rust `String`, returning `None` if null.
    #[must_use]
    pub fn to_string_lossy(&self) -> Option<String> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: `self.0` is non-null and points to a null-terminated UTF-16 string
        // owned by `self` until drop.
        unsafe { self.0.to_string().ok() }
    }
}

impl Drop for CoTaskMemPwstr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was allocated via COM task allocator and has not been freed.
            unsafe { CoTaskMemFree(Some(self.0.0.cast::<c_void>())) };
            self.0 = PWSTR::null();
        }
    }
}

/// RAII wrapper for a Win32 `PROPVARIANT`, calling `PropVariantClear` on drop.
pub struct PropVariant(PROPVARIANT);

impl PropVariant {
    /// Creates an empty `PROPVARIANT`.
    #[must_use]
    pub fn new() -> Self {
        Self(PROPVARIANT::default())
    }

    /// Returns a mutable pointer to the inner `PROPVARIANT` for COM out-parameters.
    pub fn as_mut_ptr(&mut self) -> *mut PROPVARIANT {
        &raw mut self.0
    }

    /// Extracts a string value if possible.
    #[must_use]
    pub fn as_string(&self) -> Option<String> {
        // SAFETY: `self.0` is an initialized `PROPVARIANT`; `PropVariantToStringAlloc`
        // allocates a `PWSTR` via `CoTaskMemAlloc` on success.
        let raw = unsafe { PropVariantToStringAlloc(&raw const self.0) }.ok()?;
        // SAFETY: `raw` was just allocated by `PropVariantToStringAlloc`.
        let guarded = unsafe { CoTaskMemPwstr::from_raw(raw) };
        guarded.to_string_lossy()
    }

    /// Extracts a `u32` value if possible.
    #[must_use]
    pub fn as_u32(&self) -> Option<u32> {
        // SAFETY: `self.0` is an initialized `PROPVARIANT`.
        unsafe { PropVariantToUInt32(&raw const self.0) }.ok()
    }
}

impl Default for PropVariant {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PropVariant {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid `PROPVARIANT` (either zero-initialized or populated by COM).
        let _ = unsafe { PropVariantClear(&raw mut self.0) };
    }
}

/// RAII wrapper for a Win32 `HANDLE`, closing it on drop when valid.
pub struct Handle(HANDLE);

impl Handle {
    /// Wraps a raw Win32 `HANDLE`, returning `None` if invalid.
    #[must_use]
    pub fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_invalid() { None } else { Some(Self(handle)) }
    }

    /// Returns the raw `HANDLE`.
    #[must_use]
    pub fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid open handle owned by this wrapper.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Physical form factor of a Windows audio endpoint (`PKEY_AudioEndpoint_FormFactor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FormFactor {
    /// Remote network audio device.
    RemoteNetworkDevice,
    /// Desktop or external speakers.
    Speakers,
    /// Line-level input/output (e.g. audio interface line in).
    LineLevel,
    /// Headphones without integrated mic.
    Headphones,
    /// Dedicated microphone.
    Microphone,
    /// Headset (headphones with boom/inline mic).
    Headset,
    /// Telephone handset.
    Handset,
    /// Unknown digital passthrough.
    UnknownDigitalPassthrough,
    /// S/PDIF optical or coax.
    Spdif,
    /// HDMI / `DisplayPort` audio device.
    DigitalAudioDisplayDevice,
    /// Unrecognized or unspecified form factor.
    #[default]
    Unknown,
}

impl FormFactor {
    /// Converts a raw Windows `EndpointFormFactor` `u32` value.
    #[must_use]
    pub const fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::RemoteNetworkDevice,
            1 => Self::Speakers,
            2 => Self::LineLevel,
            3 => Self::Headphones,
            4 => Self::Microphone,
            5 => Self::Headset,
            6 => Self::Handset,
            7 => Self::UnknownDigitalPassthrough,
            8 => Self::Spdif,
            9 => Self::DigitalAudioDisplayDevice,
            _ => Self::Unknown,
        }
    }
}

/// Suggests a Tine [`Archetype`] from an endpoint's [`FormFactor`] and friendly name.
#[must_use]
pub fn suggest_archetype(form_factor: FormFactor, friendly_name: &str) -> Archetype {
    let lower = friendly_name.to_ascii_lowercase();

    match form_factor {
        FormFactor::Headset | FormFactor::Headphones | FormFactor::Handset => Archetype::Headset,
        FormFactor::LineLevel => Archetype::Dynamic,
        _ => suggest_from_mic_name(&lower),
    }
}

fn suggest_from_mic_name(lower_name: &str) -> Archetype {
    const HEADSET_HINTS: &[&str] = &["headset", "earphone", "earbud", "airpods", "buds", "boom"];
    const LAPTOP_HINTS: &[&str] = &[
        "array",
        "internal",
        "built-in",
        "integrated",
        "webcam",
        "camera",
        "laptop",
        "smart sound",
        "intel",
    ];
    const CONDENSER_HINTS: &[&str] = &[
        "condenser",
        "yeti",
        "quadcast",
        "solocast",
        "duocast",
        "at2020",
        "snowball",
        "seiren",
        "nt-usb",
        "wave:1",
        "wave:3",
    ];

    if HEADSET_HINTS.iter().any(|h| lower_name.contains(h)) {
        return Archetype::Headset;
    }
    if LAPTOP_HINTS.iter().any(|h| lower_name.contains(h)) {
        return Archetype::Laptop;
    }
    if CONDENSER_HINTS.iter().any(|h| lower_name.contains(h)) {
        return Archetype::Condenser;
    }
    Archetype::Dynamic
}

/// Direction of a Windows audio endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataFlow {
    /// Capture (microphone / input) endpoints.
    Capture,
    /// Render (speaker / virtual-cable input) endpoints.
    Render,
}

impl DataFlow {
    const fn to_win(self) -> EDataFlow {
        match self {
            Self::Capture => eCapture,
            Self::Render => eRender,
        }
    }
}

/// Raw endpoint metadata read from `IMMDeviceEnumerator`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRecord {
    /// Windows `MMDevice` endpoint ID string.
    pub id: String,
    /// Friendly name (`PKEY_Device_FriendlyName`).
    pub name: String,
    /// Endpoint form factor (`PKEY_AudioEndpoint_FormFactor`).
    pub form_factor: FormFactor,
    /// `true` when the endpoint's device state is `DEVICE_STATE_ACTIVE`.
    pub is_active: bool,
}

/// Enumerates Windows audio endpoints for the given [`DataFlow`].
///
/// When `active_only` is `true`, only `DEVICE_STATE_ACTIVE` endpoints are returned.
/// When `false`, active, disabled, and unplugged endpoints are included.
pub fn enumerate_endpoints(flow: DataFlow, active_only: bool) -> Result<Vec<EndpointRecord>, WinError> {
    let _com = ComInit::new_mta()?;

    // SAFETY: COM is initialized on this thread; `MMDeviceEnumerator` is a standard in-proc/MTA class.
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| WinError::com("CoCreateInstance(MMDeviceEnumerator)", e))?;

    let state_mask = if active_only {
        DEVICE_STATE_ACTIVE
    } else {
        DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_DISABLED.0 | DEVICE_STATE_UNPLUGGED.0)
    };

    // SAFETY: `enumerator` is valid and `state_mask` is a valid bitmask of `DEVICE_STATE_*` flags.
    let collection: IMMDeviceCollection = unsafe { enumerator.EnumAudioEndpoints(flow.to_win(), state_mask) }
        .map_err(|e| WinError::com("EnumAudioEndpoints", e))?;

    // SAFETY: `collection` is a valid `IMMDeviceCollection`.
    let count = unsafe { collection.GetCount() }.map_err(|e| WinError::com("IMMDeviceCollection::GetCount", e))?;

    let mut records = Vec::with_capacity(count as usize);
    for idx in 0..count {
        // SAFETY: `idx < count` and `collection` is valid.
        let device: IMMDevice =
            unsafe { collection.Item(idx) }.map_err(|e| WinError::com("IMMDeviceCollection::Item", e))?;

        if let Some(record) = read_device_record(&device)? {
            records.push(record);
        }
    }

    Ok(records)
}

fn read_device_record(device: &IMMDevice) -> Result<Option<EndpointRecord>, WinError> {
    // SAFETY: `device` is a valid `IMMDevice`; `GetId` allocates a `PWSTR` via `CoTaskMemAlloc`.
    let raw_id = unsafe { device.GetId() }.map_err(|e| WinError::com("IMMDevice::GetId", e))?;
    // SAFETY: `raw_id` was allocated by `IMMDevice::GetId`.
    let id_guard = unsafe { CoTaskMemPwstr::from_raw(raw_id) };
    let Some(id) = id_guard.to_string_lossy() else {
        return Ok(None);
    };

    // SAFETY: `device` is a valid `IMMDevice`.
    let state = unsafe { device.GetState() }.map_err(|e| WinError::com("IMMDevice::GetState", e))?;
    let is_active = state == DEVICE_STATE_ACTIVE;

    // SAFETY: `device` is a valid `IMMDevice`; `STGM_READ` opens a read-only property store.
    let props: Option<IPropertyStore> = unsafe { device.OpenPropertyStore(STGM_READ) }.ok();

    let (name, form_factor) = match props {
        Some(ref store) => (
            read_string_prop(store, &PKEY_Device_FriendlyName).unwrap_or_else(|| id.clone()),
            read_u32_prop(store, &PKEY_AudioEndpoint_FormFactor).map_or(FormFactor::Unknown, FormFactor::from_raw),
        ),
        None => (id.clone(), FormFactor::Unknown),
    };

    Ok(Some(EndpointRecord {
        id,
        name,
        form_factor,
        is_active,
    }))
}

fn read_string_prop(store: &IPropertyStore, key: &windows::Win32::Foundation::PROPERTYKEY) -> Option<String> {
    // SAFETY: `store` and `key` are valid; `GetValue` initializes the returned `PROPVARIANT`.
    let raw = unsafe { store.GetValue(key) }.ok()?;
    let mut pv = PropVariant::new();
    // SAFETY: Transferring ownership of `raw` into our `PropVariant` drop guard.
    unsafe {
        std::ptr::write(pv.as_mut_ptr(), raw);
    }
    pv.as_string()
}

fn read_u32_prop(store: &IPropertyStore, key: &windows::Win32::Foundation::PROPERTYKEY) -> Option<u32> {
    // SAFETY: `store` and `key` are valid; `GetValue` initializes the returned `PROPVARIANT`.
    let raw = unsafe { store.GetValue(key) }.ok()?;
    let mut pv = PropVariant::new();
    // SAFETY: Transferring ownership of `raw` into our `PropVariant` drop guard.
    unsafe {
        std::ptr::write(pv.as_mut_ptr(), raw);
    }
    pv.as_u32()
}

/// Hot-plug and property change events emitted by [`ComDeviceWatcher`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    /// A new audio endpoint was added.
    Added(String),
    /// An audio endpoint was removed.
    Removed(String),
    /// The default audio endpoint changed (carries the new default endpoint ID, if any).
    DefaultChanged(String),
    /// An endpoint's state or properties changed.
    PropertyChanged(String),
}

#[repr(C)]
struct NotificationClient {
    vtable: *const IMMNotificationClient_Vtbl,
    ref_count: AtomicU32,
    tx: Sender<DeviceEvent>,
}

impl NotificationClient {
    const VTABLE: IMMNotificationClient_Vtbl = IMMNotificationClient_Vtbl {
        base__: IUnknown_Vtbl {
            QueryInterface: Self::query_interface,
            AddRef: Self::add_ref,
            Release: Self::release,
        },
        OnDeviceStateChanged: Self::on_device_state_changed,
        OnDeviceAdded: Self::on_device_added,
        OnDeviceRemoved: Self::on_device_removed,
        OnDefaultDeviceChanged: Self::on_default_device_changed,
        OnPropertyValueChanged: Self::on_property_value_changed,
    };

    fn into_com(tx: Sender<DeviceEvent>) -> IMMNotificationClient {
        let boxed = Box::new(Self {
            vtable: &Self::VTABLE,
            ref_count: AtomicU32::new(1),
            tx,
        });
        let raw = Box::into_raw(boxed).cast::<c_void>();
        // SAFETY: `raw` points to a valid `#[repr(C)]` `NotificationClient` with
        // `IMMNotificationClient_Vtbl` as its first field and initial ref_count == 1.
        unsafe { IMMNotificationClient::from_raw(raw) }
    }

    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> HRESULT {
        if interface.is_null() {
            return E_POINTER;
        }
        // SAFETY: `iid` and `interface` are valid pointers per COM `QueryInterface` contract.
        unsafe {
            *interface = std::ptr::null_mut();
            if iid.is_null() {
                return E_POINTER;
            }
            let iid_ref = &*iid;
            if *iid_ref == <IUnknown as Interface>::IID || *iid_ref == <IMMNotificationClient as Interface>::IID {
                let _ = Self::add_ref(this);
                *interface = this;
                return S_OK;
            }
        }
        E_NOINTERFACE
    }

    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        // SAFETY: `this` is a valid `NotificationClient` pointer.
        let obj = unsafe { &*this.cast::<Self>() };
        // Relaxed increment is sufficient for ref-count increments; Release synchronizes drop.
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        // SAFETY: `this` is a valid `NotificationClient` pointer.
        let obj = unsafe { &*this.cast::<Self>() };
        // Release pairs with Acquire fence before freeing the last reference.
        let prev = obj.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            // SAFETY: Last reference dropped; `this` was allocated via `Box::into_raw`.
            unsafe { drop(Box::from_raw(this.cast::<Self>())) };
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn on_device_state_changed(
        this: *mut c_void,
        pwstrdeviceid: PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> HRESULT {
        // SAFETY: `this` is a valid `NotificationClient` pointer for the duration of the callback.
        let obj = unsafe { &*this.cast::<Self>() };
        let _ = obj
            .tx
            .send(DeviceEvent::PropertyChanged(pcwstr_to_string(pwstrdeviceid)));
        S_OK
    }

    unsafe extern "system" fn on_device_added(this: *mut c_void, pwstrdeviceid: PCWSTR) -> HRESULT {
        // SAFETY: `this` is a valid `NotificationClient` pointer for the duration of the callback.
        let obj = unsafe { &*this.cast::<Self>() };
        let _ = obj.tx.send(DeviceEvent::Added(pcwstr_to_string(pwstrdeviceid)));
        S_OK
    }

    unsafe extern "system" fn on_device_removed(this: *mut c_void, pwstrdeviceid: PCWSTR) -> HRESULT {
        // SAFETY: `this` is a valid `NotificationClient` pointer for the duration of the callback.
        let obj = unsafe { &*this.cast::<Self>() };
        let _ = obj.tx.send(DeviceEvent::Removed(pcwstr_to_string(pwstrdeviceid)));
        S_OK
    }

    unsafe extern "system" fn on_default_device_changed(
        this: *mut c_void,
        _flow: EDataFlow,
        _role: ERole,
        pwstrdefaultdeviceid: PCWSTR,
    ) -> HRESULT {
        // SAFETY: `this` is a valid `NotificationClient` pointer for the duration of the callback.
        let obj = unsafe { &*this.cast::<Self>() };
        let _ = obj
            .tx
            .send(DeviceEvent::DefaultChanged(pcwstr_to_string(pwstrdefaultdeviceid)));
        S_OK
    }

    unsafe extern "system" fn on_property_value_changed(
        this: *mut c_void,
        pwstrdeviceid: PCWSTR,
        _key: windows::Win32::Foundation::PROPERTYKEY,
    ) -> HRESULT {
        // SAFETY: `this` is a valid `NotificationClient` pointer for the duration of the callback.
        let obj = unsafe { &*this.cast::<Self>() };
        let _ = obj
            .tx
            .send(DeviceEvent::PropertyChanged(pcwstr_to_string(pwstrdeviceid)));
        S_OK
    }
}

fn pcwstr_to_string(ptr: PCWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: `ptr` is non-null and points to a null-terminated UTF-16 string for the duration of the callback.
    unsafe { ptr.to_string() }.unwrap_or_default()
}

/// Safe RAII handle to a registered `IMMNotificationClient` running on a dedicated MTA thread.
///
/// Dropping `ComDeviceWatcher` unregisters the callback and joins the watcher thread.
pub struct ComDeviceWatcher {
    stop_tx: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ComDeviceWatcher {
    /// Starts watching Windows audio endpoint changes, sending events to `event_tx`.
    pub fn start(event_tx: Sender<DeviceEvent>) -> Result<Self, WinError> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), WinError>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let thread = thread::Builder::new()
            .name("tine-device-watcher".to_owned())
            .spawn(move || {
                let com = match ComInit::new_mta() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };

                // SAFETY: COM is initialized on this thread.
                let enumerator: IMMDeviceEnumerator =
                    match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
                        Ok(e) => e,
                        Err(e) => {
                            let _ = ready_tx.send(Err(WinError::com("CoCreateInstance(MMDeviceEnumerator)", e)));
                            return;
                        }
                    };

                let client = NotificationClient::into_com(event_tx);

                // SAFETY: `enumerator` and `client` are valid COM objects on this MTA thread.
                if let Err(e) = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) } {
                    let _ = ready_tx.send(Err(WinError::com("RegisterEndpointNotificationCallback", e)));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                // Wait until dropped; WASAPI delivers callbacks on its own RPC threads.
                let _ = stop_rx.recv();

                // SAFETY: `client` was registered with `enumerator` above.
                let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
                drop(client);
                drop(enumerator);
                drop(com);
            })
            .map_err(|e| WinError::ThreadSpawn(e.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_tx: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(WinError::ThreadSpawn(
                    "watcher thread exited before initialization completed".to_owned(),
                ))
            }
        }
    }
}

impl Drop for ComDeviceWatcher {
    fn drop(&mut self) {
        self.stop_tx.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
