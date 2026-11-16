use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::{PCWSTR, w};

pub const SHM_NAME: PCWSTR = w!("Global\\TineSpike.v1");
pub const SHM_MAGIC: u32 = 0x5449_4E45; // "TINE"

#[repr(C)]
pub struct SpikeShm {
    pub magic: u32,
    pub _pad: u32,
    pub heartbeat: AtomicU64,
    pub panic_count: AtomicU32,
    pub faulted: AtomicU32,
    pub sample_rate: AtomicU32,
    pub channels: AtomicU32,
    pub last_frame_count: AtomicU32,
    pub instance_count: AtomicU32,
}

pub struct ShmMapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// SAFETY: ShmMapping owns a process-wide memory-mapped view of atomics.
unsafe impl Send for ShmMapping {}
// SAFETY: All fields accessed through ShmMapping::data() are atomics or immutable magic.
unsafe impl Sync for ShmMapping {}

impl ShmMapping {
    pub fn create_or_open() -> Result<Self, windows::core::Error> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // Grant Full/All to SYSTEM (SY) and LocalService (LS), Read/Write to Authenticated Users (AU)
        let sddl = w!("D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GRGW;;;AU)");
        // SAFETY: sddl is a valid null-terminated UTF-16 string; sd is initialized by Windows.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None)?;
        }

        let sa = SECURITY_ATTRIBUTES {
            nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };

        // SAFETY: sa points to a valid SECURITY_DESCRIPTOR; SHM_NAME is a valid PCWSTR.
        let handle_res = unsafe {
            CreateFileMappingW(
                HANDLE(-1isize as *mut c_void),
                Some(&sa),
                PAGE_READWRITE,
                0,
                core::mem::size_of::<SpikeShm>() as u32,
                SHM_NAME,
            )
        };

        // SAFETY: sd.0 was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }

        let handle = handle_res?;
        // SAFETY: handle is a valid file mapping handle.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, core::mem::size_of::<SpikeShm>()) };
        if view.Value.is_null() {
            let err = windows::core::Error::from_thread();
            // SAFETY: handle is valid.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(err);
        }

        let shm = Self { handle, view };
        // SAFETY: view.Value points to at least size_of::<SpikeShm>() bytes.
        let data = unsafe { &*(shm.view.Value as *const SpikeShm) };
        if data.magic != SHM_MAGIC {
            // SAFETY: view.Value is writable and aligned.
            unsafe {
                let mut_data = &mut *(shm.view.Value as *mut SpikeShm);
                mut_data.magic = SHM_MAGIC;
            }
        }
        data.instance_count.fetch_add(1, Ordering::Relaxed);
        Ok(shm)
    }

    pub fn open_existing() -> Result<Self, windows::core::Error> {
        // SAFETY: SHM_NAME is a valid PCWSTR.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, SHM_NAME)? };
        // SAFETY: handle is a valid file mapping handle.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, core::mem::size_of::<SpikeShm>()) };
        if view.Value.is_null() {
            let err = windows::core::Error::from_thread();
            // SAFETY: handle is valid.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(err);
        }
        Ok(Self { handle, view })
    }

    #[inline(always)]
    pub fn data(&self) -> &SpikeShm {
        // SAFETY: view.Value is mapped for the lifetime of Self and has size >= SpikeShm.
        unsafe { &*(self.view.Value as *const SpikeShm) }
    }
}

impl Drop for ShmMapping {
    fn drop(&mut self) {
        // SAFETY: view and handle are valid and owned by Self.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}
