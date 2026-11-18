//! `sessions [name filter]`: lists audio sessions on every active capture
//! endpoint (process id + state). Used to check who is listening to a
//! virtual mic such as `CABLE Output`.

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    MMDeviceEnumerator, eCapture,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ};
use windows::core::Interface;

fn main() -> windows::core::Result<()> {
    let filter = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devs = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        for i in 0..devs.GetCount()? {
            let d = devs.Item(i)?;
            let name = d.OpenPropertyStore(STGM_READ)?.GetValue(&PKEY_Device_FriendlyName)?.to_string();
            if !name.to_lowercase().contains(&filter) {
                continue;
            }
            let mgr: IAudioSessionManager2 = d.Activate(CLSCTX_ALL, None)?;
            let list = mgr.GetSessionEnumerator()?;
            let n = list.GetCount()?;
            println!("{name}: {n} sessions");
            for j in 0..n {
                let c = list.GetSession(j)?;
                let c2: IAudioSessionControl2 = c.cast()?;
                let st = c.GetState()?;
                println!("  pid {:>6} active={}", c2.GetProcessId().unwrap_or(0), st == AudioSessionStateActive);
            }
        }
    }
    Ok(())
}
