//! Webcam capture for the picture-in-picture overlay (off by default).
//!
//! A Media Foundation source reader converts the camera stream to RGB32 on
//! its own thread; the video thread only picks up the latest frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::PWSTR;

#[derive(Debug, Clone)]
pub struct CamFrame {
    pub width: u32,
    pub height: u32,
    /// Top-down BGRA, alpha forced to 255.
    pub bgra: Arc<Vec<u8>>,
    pub version: u64,
}

pub struct Webcam {
    latest: Arc<Mutex<Option<CamFrame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Friendly names of the available cameras.
pub fn list_cameras() -> Vec<String> {
    with_devices(|devs| devs.iter().map(device_name).collect()).unwrap_or_default()
}

fn with_devices<R>(f: impl FnOnce(&[IMFActivate]) -> R) -> windows::core::Result<R> {
    // SAFETY: MF device enumeration; array freed after use.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(0x0002_0070, MFSTARTUP_NOSOCKET);
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut ptr, &mut count)?;
        let mut devs = Vec::new();
        for i in 0..count as usize {
            if let Some(a) = std::ptr::read(ptr.add(i)) {
                devs.push(a);
            }
        }
        if !ptr.is_null() {
            CoTaskMemFree(Some(ptr as *const _));
        }
        Ok(f(&devs))
    }
}

fn device_name(a: &IMFActivate) -> String {
    let mut p = PWSTR::null();
    let mut len = 0;
    // SAFETY: string freed with CoTaskMemFree.
    unsafe {
        if a.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut p, &mut len)
            .is_ok()
        {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            return s;
        }
    }
    String::new()
}

impl Webcam {
    /// Open the camera named `name` (or the first one).
    pub fn start(name: Option<String>) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (latest.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("webcam".into())
            .spawn(move || {
                if let Err(e) = run(name.as_deref(), &l, &s) {
                    tracing::warn!(error = %e, "webcam unavailable");
                }
            })
            .ok();
        Self {
            latest,
            stop,
            thread,
        }
    }

    pub fn latest(&self) -> Option<CamFrame> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for Webcam {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    name: Option<&str>,
    latest: &Mutex<Option<CamFrame>>,
    stop: &AtomicBool,
) -> windows::core::Result<()> {
    let activate = with_devices(|devs| {
        devs.iter()
            .find(|d| name.is_none_or(|n| device_name(d) == n))
            .or(devs.first())
            .cloned()
    })?
    .ok_or_else(windows::core::Error::empty)?;
    // SAFETY: MF source reader usage on this thread; objects are ref-counted.
    unsafe {
        let source: IMFMediaSource = activate.ActivateObject()?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &t)?;
        let cur = reader.GetCurrentMediaType(stream)?;
        let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, (size & 0xFFFF_FFFF) as u32);
        let stride = cur
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map_or((w * 4) as i32, |s| s as i32);
        tracing::info!(width = w, height = h, "webcam started");
        let mut version = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let mut flags = 0u32;
            let mut sample = None;
            reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else {
                continue;
            };
            let raw = crate::webcam::sample_bytes(&sample)?;
            let row = (w * 4) as usize;
            let mut bgra = vec![0u8; row * h as usize];
            for y in 0..h as usize {
                // Negative stride = bottom-up image.
                let src_y = if stride < 0 { h as usize - 1 - y } else { y };
                let src = raw.get(src_y * row..src_y * row + row);
                if let Some(src) = src {
                    bgra[y * row..y * row + row].copy_from_slice(src);
                }
            }
            for a in bgra.iter_mut().skip(3).step_by(4) {
                *a = 255;
            }
            version += 1;
            if let Ok(mut g) = latest.lock() {
                *g = Some(CamFrame {
                    width: w,
                    height: h,
                    bgra: Arc::new(bgra),
                    version,
                });
            }
        }
        let _ = source.Shutdown();
    }
    Ok(())
}

fn sample_bytes(sample: &IMFSample) -> windows::core::Result<Vec<u8>> {
    // SAFETY: buffer locked and unlocked around the copy.
    unsafe {
        let buf = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        buf.Unlock()?;
        Ok(v)
    }
}
