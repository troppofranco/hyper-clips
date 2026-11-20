//! Media Foundation plumbing shared by the video and audio encoders.

use std::sync::Once;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaBuffer, IMFSample, MFSTARTUP_NOSOCKET, MFStartup,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::PWSTR;

/// MF_SDK_VERSION << 16 | MF_API_VERSION (Windows 7+).
const MF_VERSION: u32 = 0x0002_0070;

/// Initialise COM (MTA) on this thread and Media Foundation once per process.
pub fn init_thread() {
    static MF: Once = Once::new();
    // SAFETY: CoInitializeEx is ref-counted per thread; MFStartup once per process.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MF.call_once(|| {
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) {
                tracing::error!(error = %e, "MFStartup failed (Windows N without Media Feature Pack?)");
            }
        });
    }
}

/// Take ownership of an MFT activate array returned by MFTEnum*.
///
/// # Safety
/// ptr must be the CoTaskMem array of count activates returned by MFTEnum*.
pub unsafe fn collect_activates(ptr: *mut Option<IMFActivate>, count: u32) -> Vec<IMFActivate> {
    let mut out = Vec::new();
    if ptr.is_null() {
        return out;
    }
    // SAFETY: MFTEnum* returns `count` owned interface pointers in a CoTaskMem array.
    unsafe {
        for i in 0..count as usize {
            if let Some(a) = std::ptr::read(ptr.add(i)) {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(ptr as *const _));
    }
    out
}

pub fn friendly_name(a: &IMFActivate) -> String {
    use windows::Win32::Media::MediaFoundation::MFT_FRIENDLY_NAME_Attribute;
    let mut p = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: allocated string is freed with CoTaskMemFree.
    unsafe {
        if a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len)
            .is_ok()
        {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            return s;
        }
    }
    String::from("unknown MFT")
}

/// Copy the payload of a sample into a Vec.
pub fn sample_bytes(sample: &IMFSample) -> windows::core::Result<Vec<u8>> {
    // SAFETY: buffer locked and unlocked around the copy.
    unsafe {
        let buf: IMFMediaBuffer = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        buf.Unlock()?;
        Ok(v)
    }
}
