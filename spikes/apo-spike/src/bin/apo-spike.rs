//! CLI for T02 APO feasibility spike.
#![allow(missing_docs, clippy::all, clippy::pedantic)]

use core::ffi::c_void;
use core::sync::atomic::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use apo_spike::shm::ShmMapping;
use serde::{Deserialize, Serialize};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, HANDLE, LUID};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATEMASK_ALL, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
};
use windows::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, SET_ACCESS, SE_REGISTRY_KEY, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, AllocateAndInitializeSid, DACL_SECURITY_INFORMATION, FreeSid,
    OWNER_SECURITY_INFORMATION, PSID, PSECURITY_DESCRIPTOR, SECURITY_NT_AUTHORITY,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_DWORD,
    REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::{GUID, HRESULT, Interface, PCWSTR, PWSTR, w};

const CLSID_STR: &str = "{70355144-DE5B-4045-BC60-B8787CDC4077}";
const I_AUDIO_SYSTEM_EFFECTS_STR: &str = "{FD7F2B29-24D0-4B5C-B177-592C39F9CA10}";
const MODE_DEFAULT_STR: &str = "{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}";

const PKEY_FX_ASSOCIATION: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},0";
const PKEY_SFX_CLSID: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5";
const PKEY_MFX_CLSID: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},6";
const PKEY_EFX_CLSID: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},7";
const PKEY_SFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},5";
const PKEY_MFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},6";
const PKEY_EFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},7";
const PKEY_DISABLE_SYSFX: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";

const INSTALL_DIR: &str = r"C:\Program Files\TineSpike";
const PROGRAM_DATA_DIR: &str = r"C:\ProgramData\TineSpike";

#[derive(Serialize, Deserialize)]
struct RegValueBackup {
    reg_type: u32,
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct FxBackup {
    endpoint_guid: String,
    values: BTreeMap<String, RegValueBackup>,
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

fn extract_guid(id: &str) -> String {
    if let Some(pos) = id.rfind('{') {
        id[pos..].to_string()
    } else {
        id.to_string()
    }
}

fn fx_reg_path(guid: &str) -> String {
    format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\FxProperties"
    )
}

fn fx_named_obj(guid: &str) -> String {
    format!(
        r"MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\FxProperties"
    )
}

fn enable_privilege(name: PCWSTR) -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| format!("OpenProcessToken: {e}"))?;

        let mut luid = LUID::default();
        let ok = windows::Win32::Security::LookupPrivilegeValueW(None, name, &mut luid);
        if let Err(e) = ok {
            let _ = CloseHandle(token);
            return Err(format!("LookupPrivilegeValueW: {e}"));
        }

        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            ..Default::default()
        };
        tp.Privileges[0].Luid = luid;
        tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;

        let res = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        let _ = CloseHandle(token);
        res.map_err(|e| format!("AdjustTokenPrivileges: {e}"))?;
        Ok(())
    }
}

fn ensure_fx_admin_access(guid: &str) -> Result<(), String> {
    let _ = enable_privilege(w!("SeTakeOwnershipPrivilege"));
    let _ = enable_privilege(w!("SeRestorePrivilege"));
    let _ = enable_privilege(w!("SeSecurityPrivilege"));

    let obj = to_wide(&fx_named_obj(guid));
    unsafe {
        let mut admin_sid = PSID::default();
        let nt_auth = SECURITY_NT_AUTHORITY;
        AllocateAndInitializeSid(
            &nt_auth,
            2,
            32,  // SECURITY_BUILTIN_DOMAIN_RID
            544, // DOMAIN_ALIAS_RID_ADMINS
            0,
            0,
            0,
            0,
            0,
            0,
            &mut admin_sid,
        )
        .map_err(|e| format!("AllocateAndInitializeSid: {e}"))?;

        // 1. Set owner to Administrators so we can edit DACL if needed.
        let r1 = SetNamedSecurityInfoW(
            PCWSTR(obj.as_ptr()),
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION,
            Some(admin_sid),
            None,
            None,
            None,
        );
        if r1.is_err() {
            FreeSid(admin_sid);
            return Err(format!("SetNamedSecurityInfoW(OWNER): {r1:?}"));
        }

        // 2. Grant Administrators Full Control on FxProperties.
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: KEY_ALL_ACCESS.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: windows::Win32::Security::NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: core::ptr::null_mut(),
                MultipleTrusteeOperation: Default::default(),
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: PWSTR(admin_sid.0 as *mut u16),
            },
        };
        let mut new_dacl = core::ptr::null_mut();
        // Read existing DACL first? Passing None replaces, so let's read existing DACL!
        let mut old_dacl = core::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let r_get = windows::Win32::Security::Authorization::GetNamedSecurityInfoW(
            PCWSTR(obj.as_ptr()),
            SE_REGISTRY_KEY,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            &mut sd,
        );
        if r_get.is_err() {
            FreeSid(admin_sid);
            return Err(format!("GetNamedSecurityInfoW: {r_get:?}"));
        }
        let r_acl = SetEntriesInAclW(Some(&[ea]), Some(old_dacl), &mut new_dacl);
        if r_acl.is_err() {
            let _ = windows::Win32::Foundation::LocalFree(Some(
                windows::Win32::Foundation::HLOCAL(sd.0),
            ));
            FreeSid(admin_sid);
            return Err(format!("SetEntriesInAclW: {r_acl:?}"));
        }
        let r_set = SetNamedSecurityInfoW(
            PCWSTR(obj.as_ptr()),
            SE_REGISTRY_KEY,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_dacl),
            None,
        );
        let _ = windows::Win32::Foundation::LocalFree(Some(
            windows::Win32::Foundation::HLOCAL(new_dacl as *mut c_void),
        ));
        let _ = windows::Win32::Foundation::LocalFree(Some(
            windows::Win32::Foundation::HLOCAL(sd.0),
        ));
        FreeSid(admin_sid);
        if r_set.is_err() {
            return Err(format!("SetNamedSecurityInfoW(DACL): {r_set:?}"));
        }
        let _ = SET_ACCESS;
        Ok(())
    }
}

fn reg_read_all(path: &str) -> Result<BTreeMap<String, RegValueBackup>, String> {
    let wide = to_wide(path);
    let mut hkey = HKEY::default();
    unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        )
        .ok()
        .map_err(|e| format!("RegOpenKeyExW({path}): {e}"))?;
    }

    let mut map = BTreeMap::new();
    let mut index = 0u32;
    loop {
        let mut name_buf = vec![0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let mut reg_type = 0u32;
        let mut data_buf = vec![0u8; 4096];
        let mut data_len = data_buf.len() as u32;

        let res = unsafe {
            RegEnumValueW(
                hkey,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut reg_type),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if res == ERROR_NO_MORE_ITEMS {
            break;
        }
        if res == ERROR_MORE_DATA {
            data_buf.resize(data_len as usize, 0);
            name_len = name_buf.len() as u32;
            let res2 = unsafe {
                RegEnumValueW(
                    hkey,
                    index,
                    Some(PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut reg_type),
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if res2.is_err() {
                unsafe {
                    let _ = RegCloseKey(hkey);
                }
                return Err(format!("RegEnumValueW retry failed: {res2:?}"));
            }
        } else if res.is_err() {
            unsafe {
                let _ = RegCloseKey(hkey);
            }
            return Err(format!("RegEnumValueW failed: {res:?}"));
        }

        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        data_buf.truncate(data_len as usize);
        map.insert(
            name,
            RegValueBackup {
                reg_type,
                data: data_buf,
            },
        );
        index += 1;
    }

    unsafe {
        let _ = RegCloseKey(hkey);
    }
    Ok(map)
}

fn reg_create_key(path: &str) -> Result<HKEY, String> {
    let wide = to_wide(path);
    let mut hkey = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide.as_ptr()),
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()
        .map_err(|e| format!("RegCreateKeyExW({path}): {e}"))?;
    }
    Ok(hkey)
}

fn reg_set_sz(hkey: HKEY, name: Option<&str>, val: &str) -> Result<(), String> {
    let name_w = name.map(to_wide);
    let val_w = to_wide(val);
    let bytes = unsafe {
        core::slice::from_raw_parts(val_w.as_ptr() as *const u8, val_w.len() * 2)
    };
    unsafe {
        RegSetValueExW(
            hkey,
            name_w
                .as_ref()
                .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
            Some(0),
            REG_SZ,
            Some(bytes),
        )
        .ok()
        .map_err(|e| format!("RegSetValueExW({name:?}): {e}"))
    }
}

fn reg_set_multi_sz(hkey: HKEY, name: &str, vals: &[&str]) -> Result<(), String> {
    let name_w = to_wide(name);
    let mut wide = Vec::new();
    for v in vals {
        wide.extend(v.encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let bytes = unsafe {
        core::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2)
    };
    unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            Some(0),
            REG_MULTI_SZ,
            Some(bytes),
        )
        .ok()
        .map_err(|e| format!("RegSetValueExW({name}): {e}"))
    }
}

fn reg_set_dword(hkey: HKEY, name: &str, val: u32) -> Result<(), String> {
    let name_w = to_wide(name);
    let bytes = val.to_le_bytes();
    unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            Some(0),
            REG_DWORD,
            Some(&bytes),
        )
        .ok()
        .map_err(|e| format!("RegSetValueExW({name}): {e}"))
    }
}

fn cmd_list() -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("CoInitializeEx: {e}"))?;
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator): {e}"))?;
        let collection = enumerator
            .EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL))
            .map_err(|e| format!("EnumAudioEndpoints: {e}"))?;
        let count = collection.GetCount().map_err(|e| format!("GetCount: {e}"))?;

        for i in 0..count {
            let dev = collection.Item(i).map_err(|e| format!("Item({i}): {e}"))?;
            let id_pwstr = dev.GetId().map_err(|e| format!("GetId: {e}"))?;
            let id = id_pwstr.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id_pwstr.0 as *const c_void));

            let state = dev.GetState().map(|s| s.0).unwrap_or(0);
            let props: IPropertyStore = dev
                .OpenPropertyStore(STGM_READ)
                .map_err(|e| format!("OpenPropertyStore: {e}"))?;
            let pv = props
                .GetValue(&PKEY_Device_FriendlyName)
                .unwrap_or_default();
            let name = PropVariantToStringAlloc(&pv)
                .map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const c_void));
                    s
                })
                .unwrap_or_else(|_| "<unknown>".to_string());

            let state_str = match state {
                1 => "ACTIVE",
                2 => "DISABLED",
                4 => "NOTPRESENT",
                8 => "UNPLUGGED",
                _ => "OTHER",
            };
            println!("[{state_str:10}] {id}  \"{name}\"");
            if state == 1 {
                let guid = extract_guid(&id);
                if let Ok(fx) = reg_read_all(&fx_reg_path(&guid)) {
                    for (k, v) in fx {
                        println!("    FX: {k} (type={}, len={})", v.reg_type, v.data.len());
                    }
                }
            }
        }
    }
    Ok(())
}

fn find_dll_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe.parent().ok_or("no parent dir")?;
    let candidate = dir.join("apo_spike.dll");
    if candidate.exists() {
        return Ok(candidate);
    }
    let fallback = PathBuf::from(r"target\release\apo_spike.dll");
    if fallback.exists() {
        return Ok(fallback);
    }
    Err(format!(
        "apo_spike.dll not found next to {} or at {}",
        exe.display(),
        fallback.display()
    ))
}

fn cmd_install(endpoint_id: &str, slot: &str) -> Result<(), String> {
    let guid = extract_guid(endpoint_id);
    let dll_src = find_dll_path()?;
    fs::create_dir_all(INSTALL_DIR).map_err(|e| format!("create_dir_all({INSTALL_DIR}): {e}"))?;
    fs::create_dir_all(PROGRAM_DATA_DIR)
        .map_err(|e| format!("create_dir_all({PROGRAM_DATA_DIR}): {e}"))?;

    let dll_dst = Path::new(INSTALL_DIR).join("apo_spike.dll");
    fs::copy(&dll_src, &dll_dst).map_err(|e| {
        format!(
            "copy {} -> {}: {e}",
            dll_src.display(),
            dll_dst.display()
        )
    })?;
    println!("Copied DLL to {}", dll_dst.display());

    // 1. Register CLSID
    let clsid_key_path = format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}\InprocServer32");
    let h_clsid = reg_create_key(&clsid_key_path)?;
    reg_set_sz(h_clsid, None, &dll_dst.to_string_lossy())?;
    reg_set_sz(h_clsid, Some("ThreadingModel"), "Both")?;
    unsafe {
        let _ = RegCloseKey(h_clsid);
    }
    println!("Registered CLSID {CLSID_STR}");

    // 2. Register AudioProcessingObjects entry
    let apo_key_path =
        format!(r"SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{CLSID_STR}");
    let h_apo = reg_create_key(&apo_key_path)?;
    reg_set_sz(h_apo, Some("FriendlyName"), "Tine APO Spike")?;
    reg_set_sz(h_apo, Some("Copyright"), "Tine Contributors")?;
    reg_set_dword(h_apo, "MajorVersion", 1)?;
    reg_set_dword(h_apo, "MinorVersion", 0)?;
    reg_set_dword(h_apo, "Flags", 15)?;
    reg_set_dword(h_apo, "MinInputConnections", 1)?;
    reg_set_dword(h_apo, "MaxInputConnections", 1)?;
    reg_set_dword(h_apo, "MinOutputConnections", 1)?;
    reg_set_dword(h_apo, "MaxOutputConnections", 1)?;
    reg_set_dword(h_apo, "MaxInstances", u32::MAX)?;
    reg_set_dword(h_apo, "NumAPOInterfaces", 1)?;
    reg_set_sz(h_apo, Some("APOInterface0"), I_AUDIO_SYSTEM_EFFECTS_STR)?;
    unsafe {
        let _ = RegCloseKey(h_apo);
    }
    println!("Registered AudioProcessingObjects entry");

    // 3. Back up original FxProperties
    let fx_path = fx_reg_path(&guid);
    let backup_path = Path::new(PROGRAM_DATA_DIR).join(format!("backup_{guid}.json"));
    if !backup_path.exists() {
        let existing = reg_read_all(&fx_path).unwrap_or_default();
        let backup = FxBackup {
            endpoint_guid: guid.clone(),
            values: existing,
        };
        let json = serde_json::to_string_pretty(&backup).map_err(|e| e.to_string())?;
        fs::write(&backup_path, json).map_err(|e| format!("write backup: {e}"))?;
        println!("Backed up original FxProperties to {}", backup_path.display());
    } else {
        println!(
            "Using existing FxProperties backup at {}",
            backup_path.display()
        );
    }

    // 4. Ensure admin write/delete access on FxProperties and write slot PKEYs
    ensure_fx_admin_access(&guid)?;
    let fx_w = to_wide(&fx_path);
    let mut h_fx = HKEY::default();
    unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(fx_w.as_ptr()),
            Some(0),
            KEY_SET_VALUE | KEY_WRITE,
            &mut h_fx,
        )
        .ok()
        .map_err(|e| format!("RegOpenKeyExW(FxProperties): {e}"))?;
    }

    reg_set_sz(h_fx, Some(PKEY_FX_ASSOCIATION), "{00000000-0000-0000-0000-000000000000}")?;
    match slot {
        "sfx" => {
            reg_set_sz(h_fx, Some(PKEY_SFX_CLSID), CLSID_STR)?;
            reg_set_multi_sz(h_fx, PKEY_SFX_MODES, &[MODE_DEFAULT_STR])?;
        }
        "mfx" => {
            reg_set_sz(h_fx, Some(PKEY_MFX_CLSID), CLSID_STR)?;
            reg_set_multi_sz(h_fx, PKEY_MFX_MODES, &[MODE_DEFAULT_STR])?;
        }
        "efx" => {
            reg_set_sz(h_fx, Some(PKEY_EFX_CLSID), CLSID_STR)?;
            reg_set_multi_sz(h_fx, PKEY_EFX_MODES, &[MODE_DEFAULT_STR])?;
        }
        other => {
            unsafe {
                let _ = RegCloseKey(h_fx);
            }
            return Err(format!("Unknown slot '{other}'; use sfx, mfx, or efx"));
        }
    }
    reg_set_dword(h_fx, PKEY_DISABLE_SYSFX, 0)?;
    unsafe {
        let _ = RegCloseKey(h_fx);
    }
    println!("Installed spike APO on {guid} (slot={slot}). Run `apo-spike reload {endpoint_id}` to activate.");
    Ok(())
}

fn cmd_uninstall(endpoint_id: &str) -> Result<(), String> {
    let guid = extract_guid(endpoint_id);
    let fx_path = fx_reg_path(&guid);
    let backup_path = Path::new(PROGRAM_DATA_DIR).join(format!("backup_{guid}.json"));

    if backup_path.exists() {
        let json = fs::read_to_string(&backup_path).map_err(|e| format!("read backup: {e}"))?;
        let backup: FxBackup = serde_json::from_str(&json).map_err(|e| format!("parse backup: {e}"))?;

        ensure_fx_admin_access(&guid)?;
        let current = reg_read_all(&fx_path).unwrap_or_default();

        let fx_w = to_wide(&fx_path);
        let mut h_fx = HKEY::default();
        unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(fx_w.as_ptr()),
                Some(0),
                KEY_SET_VALUE | KEY_WRITE,
                &mut h_fx,
            )
            .ok()
            .map_err(|e| format!("RegOpenKeyExW(FxProperties): {e}"))?;
        }

        // Delete values added after backup
        for key in current.keys() {
            if !backup.values.contains_key(key) {
                let kw = to_wide(key);
                unsafe {
                    let _ = RegDeleteValueW(h_fx, PCWSTR(kw.as_ptr()));
                }
            }
        }
        // Restore original values byte-for-byte
        for (key, val) in &backup.values {
            let kw = to_wide(key);
            unsafe {
                RegSetValueExW(
                    h_fx,
                    PCWSTR(kw.as_ptr()),
                    Some(0),
                    REG_VALUE_TYPE(val.reg_type),
                    Some(&val.data),
                )
                .ok()
                .map_err(|e| format!("restore {key}: {e}"))?;
            }
        }
        unsafe {
            let _ = RegCloseKey(h_fx);
        }
        fs::remove_file(&backup_path).map_err(|e| format!("remove backup: {e}"))?;
        println!("Restored original FxProperties for {guid}");
    } else {
        println!("No backup found at {}; skipping FxProperties restore", backup_path.display());
    }

    let clsid_key = to_wide(&format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}"));
    let apo_key = to_wide(&format!(
        r"SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{CLSID_STR}"
    ));
    let spike_key = to_wide(r"SOFTWARE\TineSpike");
    unsafe {
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(clsid_key.as_ptr()));
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(apo_key.as_ptr()));
        let _ = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(spike_key.as_ptr()));
    }
    println!("Removed spike CLSID and APO registrations.");
    Ok(())
}

fn cmd_heartbeat(count: Option<u32>) -> Result<(), String> {
    let shm = ShmMapping::open_existing()
        .map_err(|e| format!("OpenFileMappingW(Global\\TineSpike.v1): {e}"))?;
    let data = shm.data();
    let mut prev = data.heartbeat.load(Ordering::Relaxed);
    let mut prev_time = Instant::now();

    println!("Connected to Global\\TineSpike.v1 (magic={:#x}). Press Ctrl+C to stop.", data.magic);
    let mut i = 0u32;
    loop {
        if let Some(max) = count {
            if i >= max {
                return Ok(());
            }
        }
        i += 1;
        thread::sleep(Duration::from_millis(500));
        let now = Instant::now();
        let cur = data.heartbeat.load(Ordering::Relaxed);
        let dt = now.duration_since(prev_time).as_secs_f64().max(0.001);
        let rate = (cur.saturating_sub(prev)) as f64 / dt;
        prev = cur;
        prev_time = now;

        println!(
            "hb={cur} ({rate:.1}/s) | sr={}Hz ch={} frames={} | inst={} | panics={} faulted={}",
            data.sample_rate.load(Ordering::Relaxed),
            data.channels.load(Ordering::Relaxed),
            data.last_frame_count.load(Ordering::Relaxed),
            data.instance_count.load(Ordering::Relaxed),
            data.panic_count.load(Ordering::Relaxed),
            data.faulted.load(Ordering::Relaxed),
        );
    }
}

// Undocumented IPolicyConfig COM interface for endpoint disable/enable reload test.
// CLSID_PolicyConfigClient = {870AF99C-171D-4F9E-AF0D-E63DF40C2BC9}
// IID_IPolicyConfig        = {F8679F50-850A-41CF-9C72-430F290290C8}
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

#[repr(C)]
struct IPolicyConfigVtbl {
    base: windows::core::IUnknown_Vtbl,
    // 10 unused methods before SetEndpointVisibility (index 13 in vtable = 3 IUnknown + 10)
    _pad: [usize; 10],
    set_endpoint_visibility:
        unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
}

fn cmd_reload(endpoint_id: &str, use_service: bool) -> Result<(), String> {
    if use_service {
        println!("Restarting audiosrv (Restart-Service audiosrv -Force)...");
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Restart-Service audiosrv -Force"])
            .status()
            .map_err(|e| format!("Restart-Service audiosrv: {e}"))?;
        println!("Restart-Service audiosrv exited with {status}");
        return Ok(());
    }

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("CoInitializeEx: {e}"))?;
        let obj: windows::core::IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)
            .map_err(|e| format!("CoCreateInstance(PolicyConfig): {e}"))?;
        let mut raw: *mut c_void = core::ptr::null_mut();
        obj.query(&IID_IPOLICY_CONFIG, &mut raw)
            .ok()
            .map_err(|e| format!("QueryInterface(IPolicyConfig): {e}"))?;
        let vtbl = &*(*(raw as *const *const IPolicyConfigVtbl));
        let wide_id = to_wide(endpoint_id);
        let hr1 = (vtbl.set_endpoint_visibility)(raw, PCWSTR(wide_id.as_ptr()), 0);
        if hr1.is_err() {
            return Err(format!("SetEndpointVisibility(FALSE): {hr1:?}"));
        }
        thread::sleep(Duration::from_millis(250));
        let hr2 = (vtbl.set_endpoint_visibility)(raw, PCWSTR(wide_id.as_ptr()), 1);
        if hr2.is_err() {
            return Err(format!("SetEndpointVisibility(TRUE): {hr2:?}"));
        }
        println!("Toggled endpoint visibility for {endpoint_id}");
    }
    Ok(())
}

fn cmd_capture_test(endpoint_id: &str, raw_mode: bool, ms: u64) -> Result<(), String> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMOPTIONS_RAW, AudioClientProperties,
        IAudioCaptureClient, IAudioClient2, WAVEFORMATEXTENSIBLE,
    };

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("CoInitializeEx: {e}"))?;
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator): {e}"))?;
        let wide_id = to_wide(endpoint_id);
        let dev = enumerator
            .GetDevice(PCWSTR(wide_id.as_ptr()))
            .map_err(|e| format!("GetDevice: {e}"))?;
        let client: IAudioClient2 = dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate(IAudioClient2): {e}"))?;

        if raw_mode {
            let props = AudioClientProperties {
                cbSize: core::mem::size_of::<AudioClientProperties>() as u32,
                bIsOffload: false.into(),
                eCategory: windows::Win32::Media::Audio::AudioCategory_Communications,
                Options: AUDCLNT_STREAMOPTIONS_RAW,
            };
            client
                .SetClientProperties(&props)
                .map_err(|e| format!("SetClientProperties(RAW): {e}"))?;
        }

        let fmt_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat: {e}"))?;
        let fmt = &*fmt_ptr;
        let rate = fmt.nSamplesPerSec;
        let ch = fmt.nChannels as usize;
        let bits = fmt.wBitsPerSample;
        let tag = fmt.wFormatTag;
        let is_float = tag == 3
            || (tag == 0xFFFE
                && core::ptr::addr_of!((*(fmt_ptr as *const WAVEFORMATEXTENSIBLE)).SubFormat)
                    .read_unaligned()
                    == GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71));

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                1_000_000, // 100 ms buffer
                0,
                fmt_ptr,
                None,
            )
            .map_err(|e| format!("Initialize: {e}"))?;
        CoTaskMemFree(Some(fmt_ptr as *const c_void));

        let cap: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("GetService(IAudioCaptureClient): {e}"))?;
        client.Start().map_err(|e| format!("Start: {e}"))?;

        let start = Instant::now();
        let mut total_frames: u64 = 0;
        let mut sum_sq: f64 = 0.0;
        let mut peak: f32 = 0.0;

        while start.elapsed() < Duration::from_millis(ms) {
            thread::sleep(Duration::from_millis(10));
            loop {
                let pkt = cap.GetNextPacketSize().unwrap_or(0);
                if pkt == 0 {
                    break;
                }
                let mut data_ptr = core::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if cap
                    .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if frames > 0 && !data_ptr.is_null() && (flags & 2) == 0 && is_float {
                    let samples = core::slice::from_raw_parts(
                        data_ptr as *const f32,
                        frames as usize * ch,
                    );
                    for &s in samples {
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                        sum_sq += (s as f64) * (s as f64);
                    }
                }
                total_frames += frames as u64;
                let _ = cap.ReleaseBuffer(frames);
            }
        }
        let _ = client.Stop();

        let n = (total_frames as f64) * (ch as f64);
        let rms = if n > 0.0 { (sum_sq / n).sqrt() } else { 0.0 };
        let rms_db = if rms > 1e-12 {
            20.0 * rms.log10()
        } else {
            -120.0
        };
        let peak_db = if peak > 1e-12 {
            20.0 * (peak as f64).log10()
        } else {
            -120.0
        };
        println!(
            "capture-test (raw={raw_mode}): rate={rate}Hz ch={ch} bits={bits} float={is_float} frames={total_frames} rms={rms_db:.2} dBFS peak={peak_db:.2} dBFS"
        );
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: apo-spike <list | install <id> [--slot sfx|mfx|efx] | uninstall <id> | heartbeat | reload <id> [--service]>");
        std::process::exit(2);
    }

    let res = match args[0].as_str() {
        "list" => cmd_list(),
        "install" => {
            let id = args.get(1).ok_or_else(|| "missing <id>".to_string());
            let mut slot = "sfx";
            if let Some(pos) = args.iter().position(|a| a == "--slot") {
                if let Some(s) = args.get(pos + 1) {
                    slot = s.as_str();
                }
            }
            id.and_then(|id| cmd_install(id, slot))
        }
        "uninstall" => args
            .get(1)
            .ok_or_else(|| "missing <id>".to_string())
            .and_then(|id| cmd_uninstall(id)),
        "heartbeat" => {
            let mut count = None;
            if let Some(pos) = args.iter().position(|a| a == "--count") {
                if let Some(v) = args.get(pos + 1).and_then(|s| s.parse::<u32>().ok()) {
                    count = Some(v);
                }
            }
            cmd_heartbeat(count)
        }
        "reload" => {
            let use_service = args.iter().any(|a| a == "--service");
            args.get(1)
                .ok_or_else(|| "missing <id>".to_string())
                .and_then(|id| cmd_reload(id, use_service))
        }
        "capture-test" => {
            let raw_mode = args.iter().any(|a| a == "--raw");
            let mut ms = 1500u64;
            if let Some(pos) = args.iter().position(|a| a == "--ms") {
                if let Some(v) = args.get(pos + 1).and_then(|s| s.parse::<u64>().ok()) {
                    ms = v;
                }
            }
            args.get(1)
                .ok_or_else(|| "missing <id>".to_string())
                .and_then(|id| cmd_capture_test(id, raw_mode, ms))
        }
        other => Err(format!("unknown command '{other}'")),
    };

    if let Err(e) = res {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
