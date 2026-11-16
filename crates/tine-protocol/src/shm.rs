//! Versioned `#[repr(C)]` shared-memory layout (`Global\Tine.v1.<endpoint>`).
//!
//! Used across processes between `tine_apo.dll` (inside `audiodg.exe`) and `tine.exe`.

#[cfg(not(all(test, loom)))]
use core::mem::offset_of;
use core::mem::{align_of, size_of};
use core::sync::atomic::Ordering;

#[cfg(not(all(test, loom)))]
use core::sync::atomic::{AtomicU32, AtomicU64};
#[cfg(all(test, loom))]
use loom::sync::atomic::{AtomicU32, AtomicU64};

use crate::ids::SanitizedId;
use crate::params::DspParams;
use crate::sync::{ParamBlock, SpscRing, Triple};
use crate::telemetry::AudioTelemetryFrame;

/// Magic number identifying a valid Tine shared-memory mapping (`"TINEAPO1"`).
pub const SHM_MAGIC: u64 = u64::from_le_bytes(*b"TINEAPO1");

/// Shared-memory layout version. Bump whenever any field, offset, or size changes.
pub const SHM_LAYOUT_VERSION: u32 = 1;

/// Post-DSP audio tap ring capacity in samples (~0.68 s at 48 kHz; power of two).
pub const TAP_LEN: usize = 32768;

/// Prefix for Windows named file mappings.
const SHM_PREFIX: &str = "Global\\Tine.v1.";

/// Stack-allocated Windows named file mapping name (`"Global\Tine.v1.<sanitized id>"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShmName {
    buf: [u8; 128],
    len: usize,
}

impl ShmName {
    /// Returns the name as a `&str` (without trailing NUL).
    #[must_use]
    pub fn as_str(&self) -> &str {
        let slice = self.buf.get(..self.len).unwrap_or(&[]);
        // SAFETY: `shm_name` only writes ASCII bytes.
        unsafe { core::str::from_utf8_unchecked(slice) }
    }

    /// Returns the name as a NUL-terminated ASCII byte slice for Win32 ANSI APIs.
    #[must_use]
    pub fn as_bytes_with_nul(&self) -> &[u8] {
        let end = (self.len + 1).min(self.buf.len());
        self.buf.get(..end).unwrap_or(&[0])
    }
}

impl core::fmt::Display for ShmName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Formats `"Global\Tine.v1.<sanitized endpoint id>"` into a fixed stack buffer.
#[must_use]
pub fn shm_name(endpoint_id: &str) -> ShmName {
    let sanitized = SanitizedId::from_raw(endpoint_id);
    let mut buf = [0u8; 128];
    let mut len = 0usize;

    for b in SHM_PREFIX.bytes().chain(sanitized.as_str().bytes()) {
        if len + 1 >= buf.len() {
            break;
        }
        if let Some(slot) = buf.get_mut(len) {
            *slot = b;
            len += 1;
        }
    }
    // `buf[len]` is already 0 (NUL terminator).
    ShmName { buf, len }
}

/// Header at offset 0 of [`ShmLayout`].
#[repr(C)]
pub struct ShmHeader {
    /// Must equal [`SHM_MAGIC`].
    pub magic: u64,
    /// Must equal [`SHM_LAYOUT_VERSION`].
    pub layout_version: u32,
    /// Short ASCII build tag of the loaded `tine_apo.dll` (NUL-padded).
    pub apo_build: [u8; 16],
    /// Endpoint sample rate in Hz (written by APO at `LockForProcess`).
    pub sample_rate: AtomicU32,
    /// Endpoint channel count (written by APO at `LockForProcess`).
    pub channels: AtomicU32,
    /// Reserved for alignment (must be 0).
    reserved: u32,
    /// Monotonic APO heartbeat timestamp / counter (updated by APO).
    pub heartbeat: AtomicU64,
    /// State bitflags (see [`crate::telemetry::flags`]).
    pub state: AtomicU32,
    /// Telemetry writer claim token (for multi-instance APO arbitration).
    pub writer_claim: AtomicU32,
    /// Epoch milliseconds until which an Auto-Tune bypass lease is valid.
    pub bypass_lease_until_ms: AtomicU64,
}

/// Complete shared-memory region shared between `tine_apo.dll` and `tine.exe`.
#[repr(C)]
pub struct ShmLayout {
    /// Header metadata and heartbeats.
    pub header: ShmHeader,
    /// App -> APO parameter block.
    pub params: ParamBlock,
    /// APO -> App latest telemetry frame.
    pub telemetry: Triple<AudioTelemetryFrame>,
    /// APO -> App post-DSP mono audio tap for spectrum analysis.
    pub tap: SpscRing<TAP_LEN>,
}

/// Errors returned when validating a mapped [`ShmLayout`] pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShmError {
    /// Mapped pointer was null.
    NullPointer,
    /// Mapped pointer did not satisfy `align_of::<ShmLayout>()`.
    Unaligned,
    /// Mapped region is smaller than `size_of::<ShmLayout>()`.
    TooSmall {
        /// Actual mapped byte length.
        actual: usize,
        /// Required byte length.
        expected: usize,
    },
    /// Header magic did not match [`SHM_MAGIC`].
    BadMagic(u64),
    /// Header `layout_version` did not match [`SHM_LAYOUT_VERSION`].
    VersionMismatch {
        /// Version found in shared memory.
        actual: u32,
        /// Version compiled into this binary.
        expected: u32,
    },
}

impl core::fmt::Display for ShmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NullPointer => f.write_str("shared memory pointer is null"),
            Self::Unaligned => f.write_str("shared memory pointer is unaligned"),
            Self::TooSmall { actual, expected } => {
                write!(f, "shared memory too small: {actual} bytes (expected >= {expected})")
            }
            Self::BadMagic(magic) => write!(f, "invalid shared memory magic: {magic:#018x}"),
            Self::VersionMismatch { actual, expected } => {
                write!(f, "shared memory version mismatch: {actual} (expected {expected})")
            }
        }
    }
}

impl core::error::Error for ShmError {}

/// Validates a mapped shared-memory region and returns a shared reference to [`ShmLayout`].
///
/// # Safety
/// `p` must point to a readable/writable shared-memory mapping of at least `len` bytes
/// that remains mapped for `'a` and is only mutated through the atomic/lock-free fields of [`ShmLayout`].
pub unsafe fn layout_from_ptr<'a>(p: *mut u8, len: usize) -> Result<&'a ShmLayout, ShmError> {
    if p.is_null() {
        return Err(ShmError::NullPointer);
    }
    if !(p as usize).is_multiple_of(align_of::<ShmLayout>()) {
        return Err(ShmError::Unaligned);
    }
    let expected = size_of::<ShmLayout>();
    if len < expected {
        return Err(ShmError::TooSmall { actual: len, expected });
    }

    // SAFETY: Non-null, aligned, and `len >= size_of::<ShmLayout>() >= size_of::<ShmHeader>()`.
    #[allow(clippy::cast_ptr_alignment)]
    let hdr = unsafe { &*p.cast::<ShmHeader>() };
    if hdr.magic != SHM_MAGIC {
        return Err(ShmError::BadMagic(hdr.magic));
    }
    if hdr.layout_version != SHM_LAYOUT_VERSION {
        return Err(ShmError::VersionMismatch {
            actual: hdr.layout_version,
            expected: SHM_LAYOUT_VERSION,
        });
    }

    // SAFETY: Magic, version, size, and alignment are all validated.
    #[allow(clippy::cast_ptr_alignment)]
    Ok(unsafe { &*p.cast::<ShmLayout>() })
}

/// Initializes a freshly created shared-memory mapping in-place without stack-allocating [`ShmLayout`].
///
/// # Safety
/// `p` must be non-null, aligned to `align_of::<ShmLayout>()`, and point to at least
/// `size_of::<ShmLayout>()` writable bytes with exclusive initialization access.
pub unsafe fn init_layout_in_place<'a>(
    p: *mut u8,
    len: usize,
    apo_build: [u8; 16],
    sample_rate: u32,
    channels: u32,
    initial_params: &DspParams,
) -> Result<&'a ShmLayout, ShmError> {
    if p.is_null() {
        return Err(ShmError::NullPointer);
    }
    if !(p as usize).is_multiple_of(align_of::<ShmLayout>()) {
        return Err(ShmError::Unaligned);
    }
    let expected = size_of::<ShmLayout>();
    if len < expected {
        return Err(ShmError::TooSmall { actual: len, expected });
    }

    // SAFETY: `p` is non-null, aligned, and `len >= size_of::<ShmLayout>()`.
    unsafe {
        core::ptr::write_bytes(p, 0, expected);
        #[allow(clippy::cast_ptr_alignment)]
        let layout_ptr = p.cast::<ShmLayout>();

        ParamBlock::init_in_place(core::ptr::addr_of_mut!((*layout_ptr).params), initial_params);
        Triple::init_in_place(core::ptr::addr_of_mut!((*layout_ptr).telemetry));
        SpscRing::init_in_place(core::ptr::addr_of_mut!((*layout_ptr).tap));

        let hdr = &mut (*layout_ptr).header;
        hdr.apo_build = apo_build;
        hdr.sample_rate.store(sample_rate, Ordering::Relaxed);
        hdr.channels.store(channels, Ordering::Relaxed);
        hdr.layout_version = SHM_LAYOUT_VERSION;
        // Release fence ensures all prior initialization is visible before `magic` is published.
        core::sync::atomic::fence(Ordering::Release);
        hdr.magic = SHM_MAGIC;

        Ok(&*layout_ptr)
    }
}

// Compile-time layout invariants: any accidental change to field sizes or offsets is a
// compile error until `SHM_LAYOUT_VERSION` and these assertions are updated together.
#[cfg(not(all(test, loom)))]
const _: () = {
    assert!(size_of::<ShmHeader>() == 64);
    assert!(align_of::<ShmHeader>() == 8);
    assert!(offset_of!(ShmLayout, header) == 0);
    assert!(offset_of!(ShmLayout, params) == 64);
    assert!(offset_of!(ShmLayout, telemetry) == 256);
    assert!(offset_of!(ShmLayout, tap) == 448);
    assert!(size_of::<ShmLayout>() == 131_712);
    assert!(align_of::<ShmLayout>() == 64);
};
