//! Primary-monitor capture with Windows Graphics Capture (WGC).
//!
//! WGC only delivers a frame when the screen content changes, so consumers must
//! pace themselves (keep the latest frame and encode at a fixed rate).
//! Frames are copied to the CPU as tightly packed BGRA; a GPU-only path comes later.

#![cfg(windows)]

use hc_core::error::{Error, Result};
use std::sync::{Arc, Mutex};
use windows::core::{factory, IInspectable, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

fn sys<E: std::fmt::Display>(what: &str, e: E) -> Error {
    Error::System(format!("{what}: {e}"))
}

/// One captured frame: tightly packed BGRA, `width * height * 4` bytes.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Capture time in 100 ns ticks (monotonic, arbitrary origin).
    pub pts: i64,
}

struct Shared {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    on_frame: Box<dyn FnMut(Frame) + Send>,
}

impl Shared {
    /// Copies a GPU texture into a CPU-side `Frame`.
    fn read_texture(&mut self, texture: &ID3D11Texture2D, pts: i64) -> Result<Frame> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out-pointer for the call.
        unsafe { texture.GetDesc(&mut desc) };
        let (width, height) = (desc.Width, desc.Height);

        let needs_new = !matches!(&self.staging, Some((_, w, h)) if *w == width && *h == height);
        if needs_new {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut created = None;
            // SAFETY: valid descriptor and out-pointer.
            unsafe {
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut created))
            }
            .map_err(|e| sys("CreateTexture2D(staging)", e))?;
            let tex = created.ok_or_else(|| Error::System("staging texture is null".into()))?;
            self.staging = Some((tex, width, height));
        }
        let Some((staging, _, _)) = self.staging.as_ref() else {
            return Err(Error::System("no staging texture".into()));
        };

        // SAFETY: both textures belong to this device; the mapped memory is only read
        // between Map and Unmap, and every row read stays inside `RowPitch * height`.
        let bgra = unsafe {
            self.context.CopyResource(staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| sys("Map", e))?;
            let row_bytes = width as usize * 4;
            let pitch = mapped.RowPitch as usize;
            let base = mapped.pData as *const u8;
            let mut out = Vec::with_capacity(row_bytes * height as usize);
            for y in 0..height as usize {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * pitch), row_bytes));
            }
            self.context.Unmap(staging, 0);
            out
        };
        Ok(Frame {
            width,
            height,
            bgra,
            pts,
        })
    }
}

/// Running capture of the primary monitor. Dropping it stops the capture.
pub struct ScreenCapture {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    token: i64,
    width: u32,
    height: u32,
}

impl ScreenCapture {
    /// Starts capturing the primary monitor. `on_frame` runs on a system thread-pool
    /// thread for every frame; keep it fast and never block in it.
    pub fn start_primary_monitor(on_frame: impl FnMut(Frame) + Send + 'static) -> Result<Self> {
        // SAFETY: plain FFI; failure (already initialised in another mode) is harmless.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };

        let mut device = None;
        let mut context = None;
        // SAFETY: valid out-pointers; no adapter means the default hardware adapter.
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| sys("D3D11CreateDevice", e))?;
        let device = device.ok_or_else(|| Error::System("no D3D11 device".into()))?;
        let context = context.ok_or_else(|| Error::System("no D3D11 context".into()))?;

        let dxgi: IDXGIDevice = device.cast().map_err(|e| sys("IDXGIDevice", e))?;
        // SAFETY: `dxgi` is a valid DXGI device.
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| sys("CreateDirect3D11DeviceFromDXGIDevice", e))?;
        let winrt_device: IDirect3DDevice =
            inspectable.cast().map_err(|e| sys("IDirect3DDevice", e))?;

        // SAFETY: default monitor lookup with a plain point.
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| sys("GraphicsCaptureItem factory", e))?;
        // SAFETY: `monitor` is a valid monitor handle.
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForMonitor(monitor) }.map_err(|e| sys("CreateForMonitor", e))?;
        let size = item.Size().map_err(|e| sys("item size", e))?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| sys("CreateFreeThreaded", e))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| sys("CreateCaptureSession", e))?;

        let shared = Arc::new(Mutex::new(Shared {
            device,
            context,
            staging: None,
            on_frame: Box::new(on_frame),
        }));

        let handler =
            TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else {
                    return Ok(());
                };
                let frame = pool.TryGetNextFrame()?;
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                // SAFETY: the surface wraps an ID3D11Texture2D.
                let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                let pts = frame.SystemRelativeTime()?.Duration;
                if let Ok(mut guard) = shared.lock() {
                    match guard.read_texture(&texture, pts) {
                        Ok(f) => (guard.on_frame)(f),
                        Err(e) => tracing::warn!("frame readback failed: {e}"),
                    }
                }
                Ok(())
            });
        let token = pool
            .FrameArrived(&handler)
            .map_err(|e| sys("FrameArrived", e))?;
        session.StartCapture().map_err(|e| sys("StartCapture", e))?;

        Ok(Self {
            session,
            pool,
            token,
            width: size.Width as u32,
            height: size.Height as u32,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
