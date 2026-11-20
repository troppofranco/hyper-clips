//! Clip player built on the Media Foundation Media Engine (hardware decode,
//! audio output and A/V sync handled by Windows). Frames are transferred into
//! a down-scaled D3D11 texture and copied to the CPU for egui.
//!
//! The D3D11 device exists only while a player is open, so browsing the
//! library does not pay its memory cost.

use std::path::Path;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ENDED,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MFCreateAttributes, MFCreateDXGIDeviceManager,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BSTR, Interface, implement};

#[derive(Default)]
struct Shared {
    loaded: bool,
    ended: bool,
    error: Option<u32>,
}

#[implement(IMFMediaEngineNotify)]
struct Notify(Arc<Mutex<Shared>>);

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, param2: u32) -> windows::core::Result<()> {
        if let Ok(mut s) = self.0.lock() {
            if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 {
                s.loaded = true;
            } else if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 {
                s.ended = true;
            } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
                s.error = Some(param2);
            }
        }
        Ok(())
    }
}

/// A decoded frame, BGRA top-down, tightly packed.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct Player {
    engine: IMFMediaEngine,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    target: Option<(u32, u32, ID3D11Texture2D, ID3D11Texture2D)>,
    shared: Arc<Mutex<Shared>>,
    last_pts: i64,
    max: (u32, u32),
    _manager: IMFDXGIDeviceManager,
}

impl Player {
    /// Open `path` and start playing. Frames are scaled to fit `max_w × max_h`.
    pub fn open(path: &Path, max_w: u32, max_h: u32) -> windows::core::Result<Self> {
        hc_encode::mf::init_thread();
        // SAFETY: Media Foundation / D3D11 object creation; all COM objects are ref-counted.
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(windows::core::Error::empty)?;
            let context = context.ok_or_else(windows::core::Error::empty)?;
            let _ = device
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true);
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or_else(windows::core::Error::empty)?;
            manager.ResetDevice(&device, token)?;

            let shared = Arc::new(Mutex::new(Shared::default()));
            let notify: IMFMediaEngineNotify = Notify(shared.clone()).into();
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
            attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
            attrs.SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let engine = factory.CreateInstance(0, &attrs)?;
            engine.SetSource(&BSTR::from(path.to_string_lossy().as_ref()))?;
            engine.Play()?;
            Ok(Self {
                engine,
                device,
                context,
                target: None,
                shared,
                last_pts: -1,
                max: (max_w, max_h),
                _manager: manager,
            })
        }
    }

    pub fn error(&self) -> Option<u32> {
        self.shared.lock().ok().and_then(|s| s.error)
    }

    pub fn is_paused(&self) -> bool {
        // SAFETY: simple engine query.
        unsafe { self.engine.IsPaused().as_bool() }
    }

    pub fn ended(&self) -> bool {
        self.shared.lock().is_ok_and(|s| s.ended)
    }

    pub fn toggle(&mut self) {
        // SAFETY: engine control calls.
        unsafe {
            if self.is_paused() || self.ended() {
                if self.ended() {
                    let _ = self.engine.SetCurrentTime(0.0);
                    if let Ok(mut s) = self.shared.lock() {
                        s.ended = false;
                    }
                }
                let _ = self.engine.Play();
            } else {
                let _ = self.engine.Pause();
            }
        }
    }

    pub fn pause(&self) {
        // SAFETY: engine control call.
        unsafe {
            let _ = self.engine.Pause();
        }
    }

    pub fn seek(&mut self, secs: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.ended = false;
        }
        // SAFETY: engine control call.
        unsafe {
            let _ = self.engine.SetCurrentTime(secs.max(0.0));
        }
    }

    pub fn position(&self) -> f64 {
        // SAFETY: engine query.
        unsafe { self.engine.GetCurrentTime() }
    }

    pub fn duration(&self) -> f64 {
        // SAFETY: engine query (NaN until metadata is loaded).
        let d = unsafe { self.engine.GetDuration() };
        if d.is_finite() { d } else { 0.0 }
    }

    pub fn set_volume(&self, v: f64) {
        // SAFETY: engine control call.
        unsafe {
            let _ = self.engine.SetVolume(v.clamp(0.0, 1.0));
        }
    }

    /// Return a new frame if the engine produced one since the last call.
    pub fn poll_frame(&mut self) -> Option<Frame> {
        // SAFETY: engine/D3D calls on objects we own; staging map/unmap paired.
        unsafe {
            let pts = self.engine.OnVideoStreamTick().ok()?;
            if pts == self.last_pts || pts < 0 {
                return None;
            }
            let (mut nw, mut nh) = (0u32, 0u32);
            self.engine
                .GetNativeVideoSize(Some(&mut nw), Some(&mut nh))
                .ok()?;
            if nw == 0 || nh == 0 {
                return None;
            }
            let scale = (f64::from(self.max.0) / f64::from(nw))
                .min(f64::from(self.max.1) / f64::from(nh))
                .min(1.0);
            let w = ((f64::from(nw) * scale) as u32).max(2) & !1;
            let h = ((f64::from(nh) * scale) as u32).max(2) & !1;
            if self.target.as_ref().is_none_or(|t| (t.0, t.1) != (w, h)) {
                let mut desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut rt = None;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut rt))
                    .ok()?;
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut st = None;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut st))
                    .ok()?;
                self.target = Some((w, h, rt?, st?));
            }
            let (_, _, rt, st) = self.target.as_ref()?;
            let dst = RECT {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            };
            self.engine.TransferVideoFrame(rt, None, &dst, None).ok()?;
            self.context.CopyResource(st, rt);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(st, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .ok()?;
            let mut bgra = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts(
                    (m.pData as *const u8).add(y * m.RowPitch as usize),
                    w as usize * 4,
                );
                bgra[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
            }
            self.context.Unmap(st, 0);
            self.last_pts = pts;
            Some(Frame {
                width: w,
                height: h,
                bgra,
            })
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: orderly shutdown releases decoder and audio session.
        unsafe {
            let _ = self.engine.Shutdown();
        }
    }
}
