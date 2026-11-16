//! Shared-memory client for `Global\Tine.v1.<endpoint>`.
//!
//! Opens the APO's named file mapping (`OpenFileMappingW` + `MapViewOfFile`,
//! read-write), validates it with [`tine_protocol::shm::layout_from_ptr`], and
//! exposes the three channels of [`tine_protocol::shm::ShmLayout`]:
//! [`ParamBlock`] writes (app → APO), telemetry reads and tap draining
//! (APO → app), plus heartbeat age.
//!
//! Win32 surface is small and local to this file (the `win/` module belongs
//! to T14 and is not shared yet; fold these wrappers in at T21 if worthwhile).

use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tine_protocol::params::DspParams;
use tine_protocol::shm::{ShmError, ShmLayout, layout_from_ptr, shm_name};
use tine_protocol::sync::ParamBlock;
use tine_protocol::telemetry::AudioTelemetryFrame;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE, CloseHandle};
use windows::Win32::System::Memory::{FILE_MAP_ALL_ACCESS, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile};
use windows::core::PCWSTR;

use crate::model::EndpointId;

/// A heartbeat older than this marks the link stale.
pub const STALE_AFTER_MS: u64 = 2_000;

/// Reconnect-attempt period while the mapping is absent or has a version
/// mismatch (never while linked: an idle controller costs 0 % CPU).
pub const RECONNECT_POLL: Duration = Duration::from_secs(1);

/// Errors returned when opening or validating the APO's shared memory.
#[derive(Debug, Error)]
pub enum ShmOpenError {
    /// No mapping with this name exists: the APO is not installed on (or not
    /// currently processing) this endpoint.
    #[error("no shared-memory mapping (APO not installed or inactive on this endpoint)")]
    NotFound,
    /// A Win32 call failed.
    #[error("Win32 error in {context}: {source}")]
    Win {
        /// Operation that failed.
        context: &'static str,
        /// Underlying Windows error.
        #[source]
        source: windows::core::Error,
    },
    /// The mapping exists but its content failed validation.
    #[error("invalid shared-memory mapping: {0}")]
    Invalid(#[from] ShmError),
}

/// Live connection state of the APO shared memory for the selected endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// No mapping open (APO absent, or open failed).
    Unlinked,
    /// The mapping exists but its `layout_version` does not match ours.
    VersionMismatch {
        /// Version found in shared memory.
        actual: u32,
        /// Version compiled into this binary.
        expected: u32,
    },
    /// Connected; the APO heartbeat is fresh.
    Linked {
        /// Age of the most recent APO heartbeat in milliseconds.
        heartbeat_age_ms: u64,
    },
    /// Connected, but the APO heartbeat has not advanced recently.
    Stale {
        /// Age of the most recent APO heartbeat in milliseconds.
        heartbeat_age_ms: u64,
    },
}

impl LinkState {
    /// Returns `true` when parameters flow to the APO.
    #[must_use]
    pub const fn is_linked(&self) -> bool {
        matches!(self, Self::Linked { .. })
    }
}

impl fmt::Display for LinkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unlinked => f.write_str("unlinked"),
            Self::VersionMismatch { actual, expected } => {
                write!(f, "version mismatch ({actual}, expected {expected})")
            }
            Self::Linked { heartbeat_age_ms } => write!(f, "linked (heartbeat {heartbeat_age_ms} ms old)"),
            Self::Stale { heartbeat_age_ms } => write!(f, "stale (heartbeat {heartbeat_age_ms} ms old)"),
        }
    }
}

/// Encodes a `&str` as a NUL-terminated UTF-16 buffer.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Current wall-clock time in epoch milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Client handle to the APO's shared-memory mapping for one endpoint.
///
/// Cheap to clone behind an [`Arc`]; dropping the last handle unmaps the view
/// and closes the file-mapping handle. All access is lock-free, so cloning
/// across threads (UI reader, controller writer) is sound.
pub struct ShmClient {
    /// File-mapping handle; closed on drop.
    mapping: HANDLE,
    /// Mapped view; unmapped on drop.
    view: *mut c_void,
    /// Number of bytes mapped (== `size_of::<ShmLayout>()`).
    len: usize,
    /// Validated layout pointer; valid while `self` is alive.
    layout: *const ShmLayout,
}

// SAFETY: All access goes through `ShmLayout`'s lock-free atomics
// (`ParamBlock`, `Triple`, `SpscRing`), which are `Send + Sync` and support
// cross-process use; there are no interior pointers or thread-local state.
unsafe impl Send for ShmClient {}
// SAFETY: Same as `Send`: concurrent readers/writers on the lock-free
// primitives are the intended shared-memory usage pattern.
unsafe impl Sync for ShmClient {}

impl fmt::Debug for ShmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShmClient")
            .field("mapping", &self.mapping)
            .field("view", &self.view)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl Drop for ShmClient {
    fn drop(&mut self) {
        if !self.view.is_null() {
            // SAFETY: `view` was returned by a successful `MapViewOfFile` on
            // the mapping stored in `self.mapping` and mapped exactly once.
            unsafe {
                let addr = windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view };
                let _ = UnmapViewOfFile(addr);
            }
        }
        if !self.mapping.is_invalid() {
            // SAFETY: `mapping` is a valid owned handle; never used after this.
            let _ = unsafe { CloseHandle(self.mapping) };
        }
    }
}

impl ShmClient {
    /// Opens the mapping for `endpoint_id`
    /// (`Global\Tine.v1.<sanitized endpoint>`).
    pub fn open(endpoint_id: &str) -> Result<Self, ShmOpenError> {
        Self::open_name(shm_name(endpoint_id).as_str())
    }

    /// Opens an arbitrary named file mapping.
    ///
    /// Used by [`ShmClient::open`] and by tests, which use the session-local
    /// `Local\` namespace (creating `Global\` objects requires
    /// `SeCreateGlobalPrivilege`).
    pub fn open_name(name: &str) -> Result<Self, ShmOpenError> {
        let name_wide = wide(name);
        // SAFETY: `name_wide` is a valid NUL-terminated string for the call's
        // lifetime; the returned handle is owned by `self` and closed on drop.
        let mapping = match unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(name_wide.as_ptr())) } {
            Ok(h) => h,
            Err(source) => {
                return Err(match from_win32(&source) {
                    ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => ShmOpenError::NotFound,
                    _ => ShmOpenError::Win {
                        context: "OpenFileMappingW",
                        source,
                    },
                })
            }
        };

        let len = std::mem::size_of::<ShmLayout>();
        // SAFETY: `mapping` is a valid handle just obtained; the requested
        // access matches `OpenFileMappingW(FILE_MAP_ALL_ACCESS)`.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) };
        if view.Value.is_null() {
            // SAFETY: `mapping` was obtained from `OpenFileMappingW` and is
            // not used after this call.
            let _ = unsafe { CloseHandle(mapping) };
            return Err(ShmOpenError::Invalid(ShmError::NullPointer));
        }

        // SAFETY: The view was just mapped with RW access for `len` bytes and
        // outlives `self`; `layout_from_ptr` validates size, alignment, magic,
        // and version against it.
        let validated = unsafe { layout_from_ptr(view.Value.cast::<u8>(), len) };
        match validated {
            Ok(layout) => Ok(Self {
                mapping,
                view: view.Value,
                len,
                layout,
            }),
            Err(err) => {
                // SAFETY: mapped above, unmapped/closed exactly once.
                let _ = unsafe {
                    let _unmapped = UnmapViewOfFile(view);
                    CloseHandle(mapping)
                };
                Err(err.into())
            }
        }
    }

    /// Returns the validated shared-memory layout.
    ///
    /// The reference borrows `self`, so it cannot outlive the client that
    /// owns the mapping.
    #[must_use]
    pub fn layout(&self) -> &ShmLayout {
        // SAFETY: `self.layout` was produced by `layout_from_ptr` for a view
        // owned by `self`; the borrow ties its lifetime to `self` being alive.
        unsafe { &*self.layout }
    }

    /// Number of bytes mapped (always `size_of::<ShmLayout>()`).
    #[must_use]
    pub fn mapped_len(&self) -> usize {
        self.len
    }

    /// Publishes a fully resolved parameter snapshot (generation-stamped).
    pub fn write_params(&self, params: &DspParams) {
        self.layout().params.store(params);
    }

    /// Copies the latest telemetry frame into `out` if a new one is available.
    ///
    /// Single-reader by precondition (the [`crate::controller::ControllerHandle`]
    /// exposes this to exactly one consumer thread).
    pub fn read_telemetry(&self, out: &mut AudioTelemetryFrame) -> bool {
        self.layout().telemetry.latest(out)
    }

    /// Returns the latest telemetry frame, or a default snapshot.
    #[must_use]
    pub fn telemetry(&self) -> AudioTelemetryFrame {
        let mut frame = AudioTelemetryFrame::default();
        let _ = self.read_telemetry(&mut frame);
        frame
    }

    /// Drains up to `buf.len()` post-DSP tap samples; returns the count read.
    pub fn drain_tap(&self, buf: &mut [f32]) -> usize {
        self.layout().tap.pop_slice(buf)
    }

    /// Cumulative count of tap samples dropped because the ring was full.
    #[must_use]
    pub fn tap_overruns(&self) -> u64 {
        self.layout().tap.overruns()
    }

    /// Remaining live endpoint format (sample rate, channels) written by the
    /// APO at `LockForProcess`.
    #[must_use]
    pub fn sink_format(&self) -> (u32, u32) {
        let h = &self.layout().header;
        (
            h.sample_rate.load(std::sync::atomic::Ordering::Relaxed),
            h.channels.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// APO state bitflags ([`tine_protocol::telemetry::flags`]).
    #[must_use]
    pub fn state_flags(&self) -> u32 {
        self.layout().header.state.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Age of the most recent APO heartbeat in milliseconds (`None` if the APO
    /// has never published one).
    #[must_use]
    pub fn heartbeat_age_ms(&self) -> Option<u64> {
        let hb = self.layout().header.heartbeat.load(std::sync::atomic::Ordering::Relaxed);
        if hb == 0 {
            None
        } else {
            Some(now_ms().saturating_sub(hb))
        }
    }

    /// Live link state, computed from the current heartbeat age.
    #[must_use]
    pub fn state(&self) -> LinkState {
        match self.heartbeat_age_ms() {
            Some(age) if age <= STALE_AFTER_MS => LinkState::Linked { heartbeat_age_ms: age },
            Some(age) => LinkState::Stale { heartbeat_age_ms: age },
            None => LinkState::Stale {
                heartbeat_age_ms: u64::MAX,
            },
        }
    }
}

/// Matches a Windows error against a plain Win32 code (any facility).
fn from_win32(source: &windows::core::Error) -> windows::Win32::Foundation::WIN32_ERROR {
    let code = (source.code().0 as u32) & 0xFFFF;
    windows::Win32::Foundation::WIN32_ERROR(code)
}

// ---------------------------------------------------------------------------
// ParamTarget
// ---------------------------------------------------------------------------

/// Where the controller publishes resolved [`DspParams`]: the APO's shared
/// memory, or an in-process [`ParamBlock`] for tests and `devhost`.
pub trait ParamTarget {
    /// Publishes a fully resolved parameter snapshot.
    fn write(&self, params: &DspParams);
}

impl ParamTarget for ShmClient {
    fn write(&self, params: &DspParams) {
        self.write_params(params);
    }
}

impl ParamTarget for ParamBlock {
    fn write(&self, params: &DspParams) {
        ParamBlock::store(self, params);
    }
}

impl ParamTarget for Arc<ParamBlock> {
    fn write(&self, params: &DspParams) {
        (**self).write(params);
    }
}

// ---------------------------------------------------------------------------
// Target: managed, reconnecting sink owned by the controller
// ---------------------------------------------------------------------------

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Shm(_) => "Target::Shm",
            Self::Block(_) => "Target::Block",
        })
    }
}

/// Where a controller publishes resolved parameters, and how it tracks the
/// APO link lifetime.
///
/// `Shm` retries the mapping every [`RECONNECT_POLL`] **only while not
/// linked** (idle cost: zero wakeups); `Block` always succeeds and is used by
/// tests and `devhost`.
pub enum Target {
    /// The APO's shared memory (production).
    Shm(ShmTarget),
    /// An in-process [`ParamBlock`] (tests, `devhost`).
    Block(Arc<ParamBlock>),
}

impl Target {
    /// Builds an unlinked [`Target::Shm`] for `endpoint`.
    #[must_use]
    pub fn shm(endpoint: &EndpointId) -> Self {
        Self::Shm(ShmTarget::new(Some(endpoint.clone())))
    }

    /// Builds an in-process target around an existing parameter block.
    #[must_use]
    pub fn block(block: Arc<ParamBlock>) -> Self {
        Self::Block(block)
    }

    /// Publishes a fully resolved snapshot (a no-op while unlinked).
    pub fn write(&self, params: &DspParams) {
        match self {
            Self::Shm(t) => t.write(params),
            Self::Block(b) => b.store(params),
        }
    }

    /// Retries the mapping when due (only while not linked) and returns the
    /// new [`LinkState`] on a *transition*, plus the live client when one just
    /// connected (so callers can publish the current params over it).
    pub fn poll(&mut self) -> Option<(LinkState, Option<Arc<ShmClient>>)> {
        match self {
            Self::Shm(t) => t.poll(),
            Self::Block(_) => None,
        }
    }

    /// Retargets the mapping after an endpoint change (drops any live client).
    pub fn set_endpoint(&mut self, endpoint: Option<EndpointId>) {
        if let Self::Shm(t) = self {
            t.set_endpoint(endpoint);
        }
    }

    /// Returns the current link state (never initiates I/O while linked).
    #[must_use]
    pub fn state(&self) -> LinkState {
        match self {
            Self::Shm(t) => t.state(),
            Self::Block(_) => LinkState::Linked { heartbeat_age_ms: 0 },
        }
    }

    /// Returns the live client, if linked over shared memory.
    #[must_use]
    pub fn client(&self) -> Option<Arc<ShmClient>> {
        match self {
            Self::Shm(t) => t.client(),
            Self::Block(_) => None,
        }
    }

    /// Reconnect-attempt period the controller should wait while unlinked.
    #[must_use]
    pub fn poll_period(&self) -> Duration {
        poll_period()
    }
}

/// Reconnect-attempt period (shortened in unit tests, which poll in a loop).
#[cfg(not(test))]
fn poll_period() -> Duration {
    RECONNECT_POLL
}

#[cfg(test)]
fn poll_period() -> Duration {
    Duration::from_millis(10)
}

impl ParamTarget for Target {
    fn write(&self, params: &DspParams) {
        Target::write(self, params);
    }
}

/// Manages the APO mapping lifetime for one endpoint: connect, watch state,
/// reconnect (only while not linked).
pub struct ShmTarget {
    /// Endpoint whose `Global\Tine.v1.<id>` mapping we open.
    endpoint: Option<EndpointId>,
    /// Live client while the mapping is open.
    client: Option<Arc<ShmClient>>,
    /// Current link state (last transition).
    state: LinkState,
    /// Next permitted reconnect attempt (`None` = immediately).
    next_attempt: Option<Instant>,
    /// Mapping-name override (test hook: session-local `Local\` namespace;
    /// `None` in production).
    name_override: Option<String>,
}

impl ShmTarget {
    /// Creates an unlinked target that will attempt `endpoint` immediately.
    #[must_use]
    pub fn new(endpoint: Option<EndpointId>) -> Self {
        Self {
            endpoint,
            client: None,
            state: LinkState::Unlinked,
            next_attempt: None,
            name_override: None,
        }
    }

    /// Overrides the mapping name (test hook: `Local\` namespace).
    #[must_use]
    pub fn with_name(mut self, name: &str) -> Self {
        self.name_override = Some(name.to_owned());
        self
    }

    /// Mapping name to open (production: `Global\Tine.v1.<sanitized id>`).
    fn mapping_name(&self, endpoint: &EndpointId) -> String {
        self.name_override
            .clone()
            .unwrap_or_else(|| tine_protocol::shm::shm_name(endpoint.as_str()).as_str().to_owned())
    }

    /// Switches endpoint; disconnects and retries immediately.
    pub fn set_endpoint(&mut self, endpoint: Option<EndpointId>) {
        self.endpoint = endpoint;
        self.client = None;
        self.state = LinkState::Unlinked;
        self.next_attempt = None;
    }

    /// Current link state.
    #[must_use]
    pub fn state(&self) -> LinkState {
        if let Some(client) = &self.client {
            client.state()
        } else {
            self.state.clone()
        }
    }

    /// Live client, if connected.
    #[must_use]
    pub fn client(&self) -> Option<Arc<ShmClient>> {
        self.client.as_ref().map(Arc::clone)
    }

    /// Writes through the live client (no-op while unlinked; the controller
    /// re-publishes the current params as soon as a link is established).
    pub fn write(&self, params: &DspParams) {
        if let Some(client) = &self.client {
            client.write_params(params);
        }
    }

    /// Attempts reconnection when due. Returns `Some((state, client))` on a
    /// state transition; `client` is `Some` exactly when the transition is to
    /// linked.
    pub fn poll(&mut self) -> Option<(LinkState, Option<Arc<ShmClient>>)> {
        match &self.state {
            // Linked: never poll (0 % idle CPU). `Stale` is only reachable
            // pre-link here; see `state()` for the live computation.
            LinkState::Linked { .. } | LinkState::Stale { .. } => return None,
            _ => {}
        }
        if let Some(next) = self.next_attempt
            && Instant::now() < next {
                return None;
            }
        self.next_attempt = Some(Instant::now() + poll_period());

        let Some(endpoint) = self.endpoint.clone() else {
            if self.state != LinkState::Unlinked {
                self.state = LinkState::Unlinked;
                return Some((LinkState::Unlinked, None));
            }
            return None;
        };

        let name = self.mapping_name(&endpoint);
        let result = ShmClient::open_name(&name);
        let previous = self.state.clone();
        match result {
            Ok(client) => {
                let client = Arc::new(client);
                let new_state = client.state();
                self.client = Some(Arc::clone(&client));
                self.state = new_state.clone();
                if new_state == previous {
                    None
                } else {
                    Some((new_state, Some(client)))
                }
            }
            Err(ShmOpenError::Invalid(ShmError::VersionMismatch { actual, expected })) => {
                self.client = None;
                let new_state = LinkState::VersionMismatch { actual, expected };
                self.state = new_state.clone();
                if new_state == previous {
                    None
                } else {
                    Some((new_state, None))
                }
            }
            Err(_) => {
                self.client = None;
                let new_state = LinkState::Unlinked;
                self.state = new_state.clone();
                if new_state == previous {
                    None
                } else {
                    Some((new_state, None))
                }
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::{LinkState, ShmClient, ShmOpenError, ShmTarget, Target};
    use crate::model::EndpointId;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tine_protocol::params::DspParams;
    use tine_protocol::shm::{ShmError, ShmLayout, SHM_LAYOUT_VERSION, init_layout_in_place, layout_from_ptr};
    use tine_protocol::sync::ParamBlock;
    use tine_protocol::telemetry::AudioTelemetryFrame;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS,
        PAGE_READWRITE,
    };

    /// APO-side stand-in for one test: creates a local (session) mapping,
    /// initializes the `ShmLayout` in place, and lets the test act as the APO.
    struct TestMapping {
        mapping: HANDLE,
        base: *mut c_void,
        len: usize,
    }

    impl TestMapping {
        fn create(name: &str, layout_version: u32) -> Self {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let len = std::mem::size_of::<ShmLayout>();
            // SAFETY: `wide` is NUL-terminated; `HANDLE(-1)` = INVALID_HANDLE_VALUE
            // selects the system page file; size matches the layout exactly.
            let mapping = match unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    0,
                    len as u32,
                    windows::core::PCWSTR(wide.as_ptr()),
                )
            } {
                Ok(h) => h,
                Err(e) => panic!("CreateFileMappingW failed: {e}"),
            };
            // SAFETY: handle from CreateFileMappingW with matching size.
            let view = match unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) } {
                v if v.Value.is_null() => panic!("MapViewOfFile returned null"),
                v => v,
            };
            // SAFETY: fresh RW view of exactly `len` bytes, exclusively owned here.
            let initialized = unsafe {
                init_layout_in_place(
                    view.Value.cast::<u8>(),
                    len,
                    *b"test\0\0\0\0\0\0\0\0\0\0\0\0",
                    48_000,
                    1,
                    &DspParams::DEFAULT,
                )
            };
            assert!(initialized.is_ok(), "init_layout_in_place failed");
            if layout_version != SHM_LAYOUT_VERSION {
                // SAFETY: exclusive access; plain field write before any reader maps.
                let hdr = unsafe { &mut *(view.Value.cast::<ShmLayout>()) };
                hdr.header.layout_version = layout_version;
            }
            Self {
                mapping,
                base: view.Value,
                len,
            }
        }

        /// The APO-side layout. `layout_from_ptr` is used so the same
        /// validation path as production is exercised.
        fn layout(&self) -> &ShmLayout {
            // SAFETY: `base`/`len` describe a valid mapping owned by `self`.
            let validated = unsafe { layout_from_ptr(self.base.cast::<u8>(), self.len) };
            match validated {
                Ok(l) => l,
                Err(e) => panic!("test mapping failed validation: {e}"),
            }
        }

        /// Patches the header `layout_version` without re-validating (used to
        /// create and later repair a deliberately mismatched mapping).
        fn set_layout_version(&mut self, version: u32) {
            // SAFETY: `base` is a mapped `ShmLayout` created by `create`;
            // this field write races with no reader (test-only).
            let layout_ref = unsafe { &mut *self.base.cast::<ShmLayout>() };
            layout_ref.header.layout_version = version;
        }

        fn publish_telemetry(&self, seq: u64) {
            let frame = AudioTelemetryFrame {
                sequence: seq,
                input_rms_db: -20.0,
                ..AudioTelemetryFrame::default()
            };
            self.layout().telemetry.publish(&frame);
        }

        fn publish_heartbeat(&self, age_ms: u64) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            self.layout()
                .header
                .heartbeat
                .store(now.saturating_sub(age_ms), Ordering::Relaxed);
        }
    }

    impl Drop for TestMapping {
        fn drop(&mut self) {
            let view = MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base };
            // SAFETY: view from `MapViewOfFile`, unmapped exactly once.
            let _ = unsafe { UnmapViewOfFile(view) };
            if !self.mapping.is_invalid() {
                // SAFETY: handle from `CreateFileMappingW`; closed exactly once.
                let _ = unsafe { CloseHandle(self.mapping) };
            }
        }
    }

    fn unique_name(tag: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("Local\\Tine.v1.test-{tag}-{n}-{}", std::process::id())
    }

    #[test]
    fn open_missing_mapping_reports_not_found() {
        let result = ShmClient::open_name(&unique_name("absent"));
        assert!(
            matches!(result, Err(ShmOpenError::NotFound)),
            "absent mapping must map to NotFound, got {result:?}"
        );
    }

    #[test]
    fn client_round_trips_params_tap_and_telemetry() {
        let name = unique_name("roundtrip");
        let mapping = TestMapping::create(&name, SHM_LAYOUT_VERSION);
        mapping.publish_heartbeat(100);

        let client = match ShmClient::open_name(&name) {
            Ok(c) => c,
            Err(e) => panic!("client must open the fresh local mapping: {e}"),
        };
        assert_eq!(client.sink_format(), (48_000, 1));

        let mut p = DspParams::DEFAULT;
        p.input_gain_db = 12.0;
        client.write_params(&p);
        let mut loaded = DspParams::DEFAULT;
        let mut last_gen = 0u64;
        assert!(mapping.layout().params.load_if_changed(&mut last_gen, &mut loaded));
        assert_eq!(loaded.input_gain_db, 12.0);

        mapping.publish_telemetry(7);
        let mut frame = AudioTelemetryFrame::default();
        assert!(client.read_telemetry(&mut frame));
        assert_eq!(frame.sequence, 7);
        assert!(!client.read_telemetry(&mut frame), "no new frame until APO publishes");

        let fresh = matches!(client.state(), LinkState::Linked { .. });
        assert!(fresh, "fresh heartbeat implies linked, got {:?}", client.state());
        mapping.publish_heartbeat(60_000);
        assert!(matches!(client.state(), LinkState::Stale { .. }));
        let age = client.heartbeat_age_ms().unwrap_or(0);
        assert!((50_000..=70_000).contains(&age), "stale heartbeat age ~60s, got {age}");

        let samples = [1.0_f32, 2.0, 3.0, 4.0];
        assert_eq!(mapping.layout().tap.push_slice(&samples), 4);
        let mut drained = [0.0_f32; 8];
        let n = client.drain_tap(&mut drained);
        assert_eq!(n, 4);
        assert_eq!(&drained[..4], &samples[..]);
        drop(client);
        drop(mapping);
    }

    #[test]
    fn version_mismatch_is_reported_and_retried_by_target() {
        let name = unique_name("version");
        let mut mapping = TestMapping::create(&name, SHM_LAYOUT_VERSION + 1);

        let result = ShmClient::open_name(&name);
        assert!(
            matches!(
                result,
                Err(ShmOpenError::Invalid(ShmError::VersionMismatch { .. }))
            ),
            "expected VersionMismatch, got {result:?}"
        );

        let mut target = ShmTarget::new(Some(EndpointId::new("ep"))).with_name(&name);
        match target.poll() {
            Some((LinkState::VersionMismatch { actual, expected }, None)) => {
                assert_eq!((actual, expected), (SHM_LAYOUT_VERSION + 1, SHM_LAYOUT_VERSION));
            }
            other => panic!("expected version-mismatch transition, got {other:?}"),
        }

        // APO recreated the mapping with the right version: reconnect.
        mapping.set_layout_version(SHM_LAYOUT_VERSION);
        mapping.publish_heartbeat(50);
        // Poll until the retry is due (test poll period: 10 ms).
        let mut linked = None;
        let start = std::time::Instant::now();
        while linked.is_none() && start.elapsed() < std::time::Duration::from_millis(500) {
            match target.poll() {
                Some((state, client)) => {
                    assert!(matches!(state, LinkState::Linked { .. }), "{state:?}");
                    assert!(client.is_some());
                    linked = Some(state);
                }
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
        assert!(linked.is_some(), "target must relink once the version is fixed");
        assert!(target.state().is_linked());
        assert!(target.client().is_some());
        // Linked targets never poll (0 % idle CPU).
        assert!(target.poll().is_none());
    }

    #[test]
    fn target_reconnects_after_mapping_disappears_and_reappears() {
        let name = unique_name("reconnect");
        let mut target = ShmTarget::new(Some(EndpointId::new("ep"))).with_name(&name);

        let mapping = TestMapping::create(&name, SHM_LAYOUT_VERSION);
        mapping.publish_heartbeat(10);
        // Poll until linked (test poll period is 10 ms).
        let mut linked = false;
        for _ in 0..50 {
            let transition = target.poll();
            if let Some((state, _)) = transition
                && matches!(state, LinkState::Linked { .. }) {
                    linked = true;
                    break;
                }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(linked, "target must link once the mapping exists");

        // Endpoint switch disconnects and retriggers polling with a new name.
        let other = unique_name("reconnect2");
        let mapping2 = TestMapping::create(&other, SHM_LAYOUT_VERSION);
        mapping2.publish_heartbeat(10);
        target.set_endpoint(Some(EndpointId::new("ep2")));
        target = target.with_name(&other);
        let mut linked2 = false;
        for _ in 0..50 {
            if let Some((state, client)) = target.poll()
                && matches!(state, LinkState::Linked { .. }) {
                    assert!(client.is_some());
                    linked2 = true;
                    break;
                }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(linked2, "target must relink on the new endpoint mapping");
    }

    #[test]
    fn block_target_writes_through_paramblock() {
        let block: std::sync::Arc<ParamBlock> = std::sync::Arc::new(ParamBlock::new());
        let mut target = Target::block(std::sync::Arc::clone(&block));
        let mut p = DspParams::DEFAULT;
        p.gate_ratio = 4.0;
        target.write(&p);
        let mut loaded = DspParams::DEFAULT;
        let mut last_gen = 0u64;
        assert!(block.load_if_changed(&mut last_gen, &mut loaded));
        assert_eq!(loaded.gate_ratio, 4.0);
        assert_eq!(target.state(), LinkState::Linked { heartbeat_age_ms: 0 });
        assert!(target.client().is_none());
        assert!(target.poll().is_none());
    }
}
