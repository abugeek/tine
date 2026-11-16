//! Throwaway APO feasibility spike DLL (T02).
#![allow(missing_docs, clippy::all, clippy::pedantic)]

pub mod shm;

use core::ffi::c_void;
use core::panic::AssertUnwindSafe;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::sync::Mutex;

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER, E_UNEXPECTED, HANDLE, HINSTANCE,
    S_FALSE, S_OK,
};
use windows::Win32::Media::Audio::Apo::{
    APO_BUFFER_FLAGS, APO_CONNECTION_DESCRIPTOR, APO_CONNECTION_PROPERTY, APO_FLAG, APO_FLAG_BITSPERSAMPLE_MUST_MATCH,
    APO_FLAG_FRAMESPERSECOND_MUST_MATCH, APO_FLAG_INPLACE, APO_FLAG_SAMPLESPERFRAME_MUST_MATCH, APO_REG_PROPERTIES,
    APOERR_ALREADY_INITIALIZED, APOERR_FORMAT_NOT_SUPPORTED, APOERR_NOT_INITIALIZED, AUDIO_SYSTEMEFFECT,
    AUDIO_SYSTEMEFFECT_STATE, IAudioMediaType, IAudioProcessingObject, IAudioProcessingObject_Impl,
    IAudioProcessingObjectConfiguration, IAudioProcessingObjectConfiguration_Impl, IAudioProcessingObjectRT,
    IAudioProcessingObjectRT_Impl, IAudioSystemEffects, IAudioSystemEffects_Impl, IAudioSystemEffects2_Impl,
    IAudioSystemEffects3, IAudioSystemEffects3_Impl, UNCOMPRESSEDAUDIOFORMAT,
};
use windows::Win32::System::Com::{CoTaskMemAlloc, IClassFactory, IClassFactory_Impl};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::core::{BOOL, Error, GUID, HRESULT, Interface, Ref, implement, w};

use shm::ShmMapping;

/// CLSID: {70355144-DE5B-4045-BC60-B8787CDC4077}
pub const CLSID_TINE_SPIKE: GUID = GUID::from_u128(0x70355144_de5b_4045_bc60_b8787cdc4077);

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: {00000003-0000-0010-8000-00aa00389b71}
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

static DLL_OBJECTS: AtomicU32 = AtomicU32::new(0);
static DLL_LOCKS: AtomicU32 = AtomicU32::new(0);
static NEXT_INSTANCE_ID: AtomicU32 = AtomicU32::new(1);

fn log_non_rt(msg: &str) {
    let _ = create_dir_all(r"C:\ProgramData\TineSpike");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"C:\ProgramData\TineSpike\spike.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

fn read_force_panic_flag() -> bool {
    let mut hkey = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: Valid registry key path and output HKEY.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\TineSpike"),
            Some(0),
            KEY_READ,
            &mut hkey,
        )
    };
    if status.is_err() {
        return false;
    }
    let mut val: u32 = 0;
    let mut size: u32 = core::mem::size_of::<u32>() as u32;
    let mut reg_type = REG_DWORD;
    // SAFETY: hkey is valid; val is properly sized.
    let q = unsafe {
        RegQueryValueExW(
            hkey,
            w!("ForcePanic"),
            None,
            Some(&mut reg_type),
            Some(&mut val as *mut u32 as *mut u8),
            Some(&mut size),
        )
    };
    // SAFETY: hkey is valid.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    q.is_ok() && val != 0
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        log_non_rt("DllMain: DLL_PROCESS_ATTACH");
    }
    BOOL(1)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if ppv.is_null() {
            return E_POINTER;
        }
        // SAFETY: ppv was checked for null.
        unsafe { *ppv = core::ptr::null_mut() };
        if rclsid.is_null() || riid.is_null() {
            return E_INVALIDARG;
        }
        // SAFETY: rclsid and riid are valid pointers per COM contract.
        let clsid = unsafe { *rclsid };
        if clsid != CLSID_TINE_SPIKE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = SpikeClassFactory.into();
        // SAFETY: riid and ppv are valid.
        unsafe { factory.query(riid, ppv) }
    }));
    res.unwrap_or(E_UNEXPECTED)
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if DLL_OBJECTS.load(Ordering::Acquire) == 0 && DLL_LOCKS.load(Ordering::Acquire) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[implement(IClassFactory)]
struct SpikeClassFactory;

impl IClassFactory_Impl for SpikeClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<windows::core::IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if ppvobject.is_null() {
                return Err(Error::from(E_POINTER));
            }
            // SAFETY: ppvobject checked for null.
            unsafe { *ppvobject = core::ptr::null_mut() };
            if !punkouter.is_null() {
                return Err(Error::from(CLASS_E_NOAGGREGATION));
            }
            if riid.is_null() {
                return Err(Error::from(E_INVALIDARG));
            }
            let apo: IAudioProcessingObject = TineSpikeApo::new().into();
            // SAFETY: riid and ppvobject are valid.
            let hr = unsafe { apo.query(riid, ppvobject) };
            hr.ok()
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    fn LockServer(&self, flock: BOOL) -> windows::core::Result<()> {
        if flock.as_bool() {
            DLL_LOCKS.fetch_add(1, Ordering::Relaxed);
        } else {
            DLL_LOCKS.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[implement(
    IAudioProcessingObject,
    IAudioProcessingObjectRT,
    IAudioProcessingObjectConfiguration,
    IAudioSystemEffects3
)]
struct TineSpikeApo {
    id: u32,
    initialized: AtomicBool,
    locked: AtomicBool,
    faulted: AtomicBool,
    force_panic: AtomicBool,
    channels: AtomicU32,
    sample_rate: AtomicU32,
    shm: Mutex<Option<ShmMapping>>,
    // Cached raw pointer for lock-free RT access while locked; valid as long as `shm` holds the mapping.
    shm_ptr: core::sync::atomic::AtomicPtr<shm::SpikeShm>,
}

impl TineSpikeApo {
    fn new() -> Self {
        let id = NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        DLL_OBJECTS.fetch_add(1, Ordering::Relaxed);
        log_non_rt(&format!("[inst {id}] TineSpikeApo::new"));
        Self {
            id,
            initialized: AtomicBool::new(false),
            locked: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
            force_panic: AtomicBool::new(false),
            channels: AtomicU32::new(1),
            sample_rate: AtomicU32::new(48000),
            shm: Mutex::new(None),
            shm_ptr: core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()),
        }
    }
}

impl Drop for TineSpikeApo {
    fn drop(&mut self) {
        self.shm_ptr.store(core::ptr::null_mut(), Ordering::Release);
        DLL_OBJECTS.fetch_sub(1, Ordering::Relaxed);
        log_non_rt(&format!("[inst {}] TineSpikeApo::drop", self.id));
    }
}

impl IAudioProcessingObject_Impl for TineSpikeApo_Impl {
    fn Reset(&self) -> windows::core::Result<()> {
        log_non_rt(&format!("[inst {}] Reset", self.id));
        Ok(())
    }

    fn GetLatency(&self) -> windows::core::Result<i64> {
        Ok(0)
    }

    fn GetRegistrationProperties(&self) -> windows::core::Result<*mut APO_REG_PROPERTIES> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let size = core::mem::size_of::<APO_REG_PROPERTIES>();
            // SAFETY: CoTaskMemAlloc allocates `size` bytes for Windows COM caller to free.
            let ptr = unsafe { CoTaskMemAlloc(size) as *mut APO_REG_PROPERTIES };
            if ptr.is_null() {
                return Err(Error::from(E_FAIL));
            }
            let mut props = APO_REG_PROPERTIES::default();
            props.clsid = CLSID_TINE_SPIKE;
            props.Flags = APO_FLAG(
                APO_FLAG_INPLACE.0
                    | APO_FLAG_SAMPLESPERFRAME_MUST_MATCH.0
                    | APO_FLAG_FRAMESPERSECOND_MUST_MATCH.0
                    | APO_FLAG_BITSPERSAMPLE_MUST_MATCH.0,
            );
            let name = "Tine APO Spike\0";
            for (i, ch) in name.encode_utf16().enumerate() {
                if i < props.szFriendlyName.len() {
                    props.szFriendlyName[i] = ch;
                }
            }
            let copy = "Tine Contributors\0";
            for (i, ch) in copy.encode_utf16().enumerate() {
                if i < props.szCopyrightInfo.len() {
                    props.szCopyrightInfo[i] = ch;
                }
            }
            props.u32MajorVersion = 1;
            props.u32MinorVersion = 0;
            props.u32MinInputConnections = 1;
            props.u32MaxInputConnections = 1;
            props.u32MinOutputConnections = 1;
            props.u32MaxOutputConnections = 1;
            props.u32MaxInstances = u32::MAX;
            props.u32NumAPOInterfaces = 1;
            props.iidAPOInterfaceList[0] = IAudioSystemEffects::IID;
            // SAFETY: ptr is non-null, properly aligned, and has `size` bytes.
            unsafe { ptr.write(props) };
            Ok(ptr)
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    fn Initialize(&self, cbdatasize: u32, _pbydata: *const u8) -> windows::core::Result<()> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if self.initialized.swap(true, Ordering::AcqRel) {
                return Err(Error::from(APOERR_ALREADY_INITIALIZED));
            }
            log_non_rt(&format!("[inst {}] Initialize (cbDataSize={cbdatasize})", self.id));
            match ShmMapping::create_or_open() {
                Ok(mapping) => {
                    let raw = mapping.data() as *const shm::SpikeShm as *mut shm::SpikeShm;
                    self.shm_ptr.store(raw, Ordering::Release);
                    if let Ok(mut guard) = self.shm.lock() {
                        *guard = Some(mapping);
                    }
                    log_non_rt(&format!("[inst {}] Global\\TineSpike.v1 created/opened OK", self.id));
                }
                Err(e) => {
                    log_non_rt(&format!("[inst {}] Global\\TineSpike.v1 FAILED: {e:?}", self.id));
                }
            }
            Ok(())
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    fn IsInputFormatSupported(
        &self,
        poppositeformat: Ref<IAudioMediaType>,
        prequestedinputformat: Ref<IAudioMediaType>,
    ) -> windows::core::Result<IAudioMediaType> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let req = prequestedinputformat.as_ref().ok_or_else(|| Error::from(E_POINTER))?;
            let mut fmt = UNCOMPRESSEDAUDIOFORMAT::default();
            // SAFETY: req is a valid IAudioMediaType.
            unsafe { req.GetUncompressedAudioFormat(&mut fmt)? };
            let is_f32 = fmt.guidFormatType == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                && fmt.dwBytesPerSampleContainer == 4
                && fmt.dwValidBitsPerSample == 32;
            if !is_f32 {
                log_non_rt(&format!(
                    "[inst {}] IsInputFormatSupported: REJECTED fmt={:?} bits={} rate={} ch={}",
                    self.id, fmt.guidFormatType, fmt.dwValidBitsPerSample, fmt.fFramesPerSecond, fmt.dwSamplesPerFrame
                ));
                return Err(Error::from(APOERR_FORMAT_NOT_SUPPORTED));
            }
            if let Some(opp) = poppositeformat.as_ref() {
                let mut opp_fmt = UNCOMPRESSEDAUDIOFORMAT::default();
                // SAFETY: opp is valid.
                if unsafe { opp.GetUncompressedAudioFormat(&mut opp_fmt) }.is_ok() {
                    if opp_fmt.dwSamplesPerFrame != fmt.dwSamplesPerFrame
                        || (opp_fmt.fFramesPerSecond - fmt.fFramesPerSecond).abs() > 0.5
                    {
                        return Err(Error::from(APOERR_FORMAT_NOT_SUPPORTED));
                    }
                }
            }
            Ok(req.clone())
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    fn IsOutputFormatSupported(
        &self,
        poppositeformat: Ref<IAudioMediaType>,
        prequestedoutputformat: Ref<IAudioMediaType>,
    ) -> windows::core::Result<IAudioMediaType> {
        self.IsInputFormatSupported(poppositeformat, prequestedoutputformat)
    }

    fn GetInputChannelCount(&self) -> windows::core::Result<u32> {
        Ok(self.channels.load(Ordering::Relaxed))
    }
}

impl IAudioProcessingObjectConfiguration_Impl for TineSpikeApo_Impl {
    fn LockForProcess(
        &self,
        u32numinputconnections: u32,
        ppinputconnections: *const *const APO_CONNECTION_DESCRIPTOR,
        u32numoutputconnections: u32,
        ppoutputconnections: *const *const APO_CONNECTION_DESCRIPTOR,
    ) -> windows::core::Result<()> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if !self.initialized.load(Ordering::Acquire) {
                return Err(Error::from(APOERR_NOT_INITIALIZED));
            }
            if u32numinputconnections != 1
                || u32numoutputconnections != 1
                || ppinputconnections.is_null()
                || ppoutputconnections.is_null()
            {
                return Err(Error::from(E_INVALIDARG));
            }
            // SAFETY: pointers checked for null and count == 1.
            let in_desc = unsafe { &**ppinputconnections };
            // SAFETY: pointers checked for null and count == 1.
            let out_desc = unsafe { &**ppoutputconnections };

            let mut fmt = UNCOMPRESSEDAUDIOFORMAT::default();
            if let Some(pfmt) = in_desc.pFormat.as_ref() {
                // SAFETY: pfmt is a valid IAudioMediaType.
                unsafe { pfmt.GetUncompressedAudioFormat(&mut fmt)? };
            }
            let rate = fmt.fFramesPerSecond as u32;
            let ch = fmt.dwSamplesPerFrame.max(1);
            self.sample_rate.store(rate, Ordering::Relaxed);
            self.channels.store(ch, Ordering::Relaxed);

            let force = read_force_panic_flag();
            self.force_panic.store(force, Ordering::Release);

            let shm_raw = self.shm_ptr.load(Ordering::Acquire);
            if !shm_raw.is_null() {
                // SAFETY: shm_raw is valid while self.shm holds the mapping.
                let shm = unsafe { &*shm_raw };
                shm.sample_rate.store(rate, Ordering::Relaxed);
                shm.channels.store(ch, Ordering::Relaxed);
            }

            let msg = format!(
                "[inst {}] LockForProcess: rate={}Hz ch={} bits={}/{} maxFrames={} inType={:?} outType={:?} inBuf={:#x} outBuf={:#x} forcePanic={}",
                self.id,
                rate,
                ch,
                fmt.dwValidBitsPerSample,
                fmt.dwBytesPerSampleContainer * 8,
                in_desc.u32MaxFrameCount,
                in_desc.Type,
                out_desc.Type,
                in_desc.pBuffer,
                out_desc.pBuffer,
                force,
            );
            log_non_rt(&msg);
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(r"C:\ProgramData\TineSpike\format.log")
            {
                let _ = writeln!(f, "{msg}");
            }

            self.locked.store(true, Ordering::Release);
            Ok(())
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    fn UnlockForProcess(&self) -> windows::core::Result<()> {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.locked.store(false, Ordering::Release);
            log_non_rt(&format!("[inst {}] UnlockForProcess", self.id));
            Ok(())
        }));
        res.unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }
}

impl IAudioProcessingObjectRT_Impl for TineSpikeApo_Impl {
    fn APOProcess(
        &self,
        u32numinputconnections: u32,
        ppinputconnections: *const *const APO_CONNECTION_PROPERTY,
        u32numoutputconnections: u32,
        ppoutputconnections: *mut *mut APO_CONNECTION_PROPERTY,
    ) {
        if u32numinputconnections == 0
            || u32numoutputconnections == 0
            || ppinputconnections.is_null()
            || ppoutputconnections.is_null()
        {
            return;
        }
        // SAFETY: checked non-null and count >= 1.
        let in_prop = unsafe { &**ppinputconnections };
        // SAFETY: checked non-null and count >= 1.
        let out_prop = unsafe { &mut **ppoutputconnections };

        let frames = in_prop.u32ValidFrameCount;
        out_prop.u32ValidFrameCount = frames;
        out_prop.u32BufferFlags = in_prop.u32BufferFlags;

        let ch = self.channels.load(Ordering::Relaxed) as usize;
        let sample_count = (frames as usize).saturating_mul(ch);
        let in_ptr = in_prop.pBuffer as *const f32;
        let out_ptr = out_prop.pBuffer as *mut f32;
        if in_ptr.is_null() || out_ptr.is_null() {
            return;
        }

        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if self.faulted.load(Ordering::Relaxed) {
                if in_ptr as usize != out_ptr as usize && sample_count > 0 {
                    // SAFETY: buffers are valid for `sample_count` f32s per APO contract.
                    unsafe {
                        core::ptr::copy_nonoverlapping(in_ptr, out_ptr, sample_count);
                    }
                }
                return;
            }

            if self.force_panic.swap(false, Ordering::AcqRel) {
                panic!("Forced panic in TineSpikeApo::APOProcess");
            }

            let shm_raw = self.shm_ptr.load(Ordering::Relaxed);
            if !shm_raw.is_null() {
                // SAFETY: shm_raw is valid while locked.
                let shm = unsafe { &*shm_raw };
                shm.heartbeat.fetch_add(1, Ordering::Relaxed);
                shm.last_frame_count.store(frames, Ordering::Relaxed);
            }

            // BUFFER_SILENT = 0, BUFFER_VALID = 1, BUFFER_INVALID = 2
            if in_prop.u32BufferFlags == APO_BUFFER_FLAGS(1) && sample_count > 0 {
                // SAFETY: in_ptr and out_ptr are valid for sample_count f32s.
                unsafe {
                    if in_ptr as usize == out_ptr as usize {
                        let buf = core::slice::from_raw_parts_mut(out_ptr, sample_count);
                        for s in buf {
                            *s *= 0.25; // -12.04 dB
                        }
                    } else {
                        let src = core::slice::from_raw_parts(in_ptr, sample_count);
                        let dst = core::slice::from_raw_parts_mut(out_ptr, sample_count);
                        for (d, s) in dst.iter_mut().zip(src.iter()) {
                            *d = *s * 0.25;
                        }
                    }
                }
            } else if in_ptr as usize != out_ptr as usize && sample_count > 0 {
                // SAFETY: buffers are valid for sample_count f32s.
                unsafe {
                    core::ptr::copy_nonoverlapping(in_ptr, out_ptr, sample_count);
                }
            }
        }));

        if res.is_err() {
            self.faulted.store(true, Ordering::Release);
            let shm_raw = self.shm_ptr.load(Ordering::Relaxed);
            if !shm_raw.is_null() {
                // SAFETY: shm_raw is valid.
                let shm = unsafe { &*shm_raw };
                shm.faulted.store(1, Ordering::Relaxed);
                shm.panic_count.fetch_add(1, Ordering::Relaxed);
            }
            if in_ptr as usize != out_ptr as usize && sample_count > 0 {
                // SAFETY: buffers are valid for sample_count f32s.
                unsafe {
                    core::ptr::copy_nonoverlapping(in_ptr, out_ptr, sample_count);
                }
            }
        }
    }

    fn CalcInputFrames(&self, u32outputframecount: u32) -> u32 {
        u32outputframecount
    }

    fn CalcOutputFrames(&self, u32inputframecount: u32) -> u32 {
        u32inputframecount
    }
}

impl IAudioSystemEffects_Impl for TineSpikeApo_Impl {}

impl IAudioSystemEffects2_Impl for TineSpikeApo_Impl {
    fn GetEffectsList(
        &self,
        ppeffectsids: *mut *mut GUID,
        pceffects: *mut u32,
        _event: HANDLE,
    ) -> windows::core::Result<()> {
        log_non_rt(&format!("[inst {}] IAudioSystemEffects2::GetEffectsList", self.id));
        if !ppeffectsids.is_null() {
            // SAFETY: checked non-null.
            unsafe { *ppeffectsids = core::ptr::null_mut() };
        }
        if !pceffects.is_null() {
            // SAFETY: checked non-null.
            unsafe { *pceffects = 0 };
        }
        Ok(())
    }
}

impl IAudioSystemEffects3_Impl for TineSpikeApo_Impl {
    fn GetControllableSystemEffectsList(
        &self,
        effects: *mut *mut AUDIO_SYSTEMEFFECT,
        numeffects: *mut u32,
        _event: HANDLE,
    ) -> windows::core::Result<()> {
        log_non_rt(&format!(
            "[inst {}] IAudioSystemEffects3::GetControllableSystemEffectsList",
            self.id
        ));
        if !effects.is_null() {
            // SAFETY: checked non-null.
            unsafe { *effects = core::ptr::null_mut() };
        }
        if !numeffects.is_null() {
            // SAFETY: checked non-null.
            unsafe { *numeffects = 0 };
        }
        Ok(())
    }

    fn SetAudioSystemEffectState(&self, effectid: &GUID, state: AUDIO_SYSTEMEFFECT_STATE) -> windows::core::Result<()> {
        log_non_rt(&format!(
            "[inst {}] IAudioSystemEffects3::SetAudioSystemEffectState id={effectid:?} state={state:?}",
            self.id
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::CoTaskMemFree;

    #[test]
    fn com_object_and_gain_processing_work() {
        let apo: IAudioProcessingObject = TineSpikeApo::new().into();
        // SAFETY: Test exercises in-process COM interface methods with valid stack buffers.
        unsafe {
            let props = apo.GetRegistrationProperties().expect("GetRegistrationProperties");
            assert_eq!((*props).clsid, CLSID_TINE_SPIKE);
            assert_eq!((*props).u32MinInputConnections, 1);
            CoTaskMemFree(Some(props as *const c_void));

            apo.Initialize(&[]).expect("Initialize");

            let rt: IAudioProcessingObjectRT = apo.cast().expect("cast IAudioProcessingObjectRT");
            assert_eq!(rt.CalcInputFrames(480), 480);
            assert_eq!(rt.CalcOutputFrames(480), 480);

            let mut in_buf = [0.8_f32; 480];
            let mut out_buf = [0.0_f32; 480];
            let in_prop = APO_CONNECTION_PROPERTY {
                pBuffer: in_buf.as_mut_ptr() as usize,
                u32ValidFrameCount: 480,
                u32BufferFlags: APO_BUFFER_FLAGS(1),
                u32Signature: 0,
            };
            let mut out_prop = APO_CONNECTION_PROPERTY {
                pBuffer: out_buf.as_mut_ptr() as usize,
                u32ValidFrameCount: 0,
                u32BufferFlags: APO_BUFFER_FLAGS(0),
                u32Signature: 0,
            };
            let in_ptr: *const APO_CONNECTION_PROPERTY = &in_prop;
            let mut out_ptr: *mut APO_CONNECTION_PROPERTY = &mut out_prop;
            rt.APOProcess(1, &in_ptr, 1, &mut out_ptr);

            assert_eq!(out_prop.u32ValidFrameCount, 480);
            assert_eq!(out_prop.u32BufferFlags, APO_BUFFER_FLAGS(1));
            assert!((out_buf[0] - 0.2).abs() < 1e-6);
            assert!((out_buf[479] - 0.2).abs() < 1e-6);
        }
    }
}
