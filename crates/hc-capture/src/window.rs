//! Single-window capture with Windows.Graphics.Capture (WGC).
//!
//! Captures just the game window even when it is covered or on a mixed-DPI
//! setup, cursor included (WGC composes it). On Windows 10 the system draws
//! a yellow border around the captured window; hiding it is only possible on
//! Windows 11, where we request it.

use std::time::{Duration, Instant};

use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::Interface;

use crate::CaptureError;
use crate::compose::texture_desc;

pub struct WindowCapture {
    pub hwnd: isize,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d3d: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    size: SizeInt32,
    frame: Option<ID3D11Texture2D>,
}

impl WindowCapture {
    pub fn new(
        hwnd: isize,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        cursor: bool,
    ) -> Result<Self, CaptureError> {
        // SAFETY: WinRT interop calls; all objects are reference counted.
        unsafe {
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(HWND(hwnd as *mut _))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &d3d,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(cursor);
            // Windows 11 only; harmless failure on Windows 10.
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;
            Ok(Self {
                hwnd,
                device,
                context,
                d3d,
                pool,
                _session: session,
                size,
                frame: None,
            })
        }
    }

    /// Window client size at start (used to size the encoder).
    pub fn initial_size(&self) -> (u32, u32) {
        (
            self.size.Width.max(2) as u32,
            self.size.Height.max(2) as u32,
        )
    }

    pub fn frame(&self) -> Option<&ID3D11Texture2D> {
        self.frame.as_ref()
    }

    pub fn acquire(&mut self, timeout_ms: u32) -> Result<bool, CaptureError> {
        // SAFETY: plain query on a window handle.
        if !unsafe { IsWindow(Some(HWND(self.hwnd as *mut _))) }.as_bool() {
            return Err(CaptureError::Other("captured window was closed".into()));
        }
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let frame = loop {
            if let Ok(f) = self.pool.TryGetNextFrame() {
                break f;
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let content = frame.ContentSize()?;
        if content.Width != self.size.Width || content.Height != self.size.Height {
            // Window resized: rebuild the pool; this frame is still usable.
            self.size = content;
            self.pool.Recreate(
                &self.d3d,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                content,
            )?;
        }
        let (w, h) = (content.Width.max(2) as u32, content.Height.max(2) as u32);
        // SAFETY: the surface is a D3D11 texture on our device.
        unsafe {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let src: ID3D11Texture2D = access.GetInterface()?;
            let mut sd = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut sd);
            let (w, h) = (w.min(sd.Width), h.min(sd.Height));
            let recreate = self.frame.as_ref().is_none_or(|f| {
                let mut d = D3D11_TEXTURE2D_DESC::default();
                f.GetDesc(&mut d);
                d.Width != w || d.Height != h
            });
            if recreate {
                let mut tex = None;
                self.device.CreateTexture2D(
                    &texture_desc(
                        w,
                        h,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0,
                    ),
                    None,
                    Some(&mut tex),
                )?;
                self.frame = tex;
            }
            if let Some(dst) = &self.frame {
                let region = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: w,
                    bottom: h,
                    back: 1,
                };
                self.context
                    .CopySubresourceRegion(dst, 0, 0, 0, 0, &src, 0, Some(&region));
            }
        }
        let _ = frame.Close();
        Ok(true)
    }
}

/// Whether WGC is available on this system (Windows 10 1903+).
pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}
