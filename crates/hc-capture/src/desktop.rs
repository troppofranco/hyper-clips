//! DXGI Desktop Duplication of one monitor.

use windows::Win32::Foundation::{LUID, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_INVALID_CALL, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutput6, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::CaptureError;
use crate::cursor::{CursorImage, shape_to_bgra};

#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub adapter_index: u32,
    pub output_index: u32,
    pub adapter_name: String,
    pub device_name: String,
    pub adapter_luid: LUID,
    pub rect: RECT,
}

impl OutputInfo {
    pub fn width(&self) -> u32 {
        (self.rect.right - self.rect.left).unsigned_abs()
    }
    pub fn height(&self) -> u32 {
        (self.rect.bottom - self.rect.top).unsigned_abs()
    }
    pub fn is_primary(&self) -> bool {
        self.rect.left == 0 && self.rect.top == 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.left && x < self.rect.right && y >= self.rect.top && y < self.rect.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorSelect {
    Primary,
    /// Monitor containing this virtual-desktop point (e.g. the game window centre).
    Containing(i32, i32),
}

fn wide_to_string(w: &[u16]) -> String {
    let len = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..len])
}

/// Every monitor attached to a hardware adapter.
pub fn list_outputs() -> Result<Vec<OutputInfo>, CaptureError> {
    let mut out = Vec::new();
    // SAFETY: DXGI enumeration; interfaces are ref-counted by windows-rs.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            let desc = adapter.GetDesc1()?;
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                let mut oi = 0;
                while let Ok(output) = adapter.EnumOutputs(oi) {
                    let od = output.GetDesc()?;
                    if od.AttachedToDesktop.as_bool() {
                        out.push(OutputInfo {
                            adapter_index: ai,
                            output_index: oi,
                            adapter_name: wide_to_string(&desc.Description),
                            device_name: wide_to_string(&od.DeviceName),
                            adapter_luid: desc.AdapterLuid,
                            rect: od.DesktopCoordinates,
                        });
                    }
                    oi += 1;
                }
            }
            ai += 1;
        }
    }
    Ok(out)
}

pub fn select_output(sel: MonitorSelect) -> Result<OutputInfo, CaptureError> {
    let outputs = list_outputs()?;
    let found = match sel {
        MonitorSelect::Primary => outputs.iter().find(|o| o.is_primary()),
        MonitorSelect::Containing(x, y) => outputs.iter().find(|o| o.contains(x, y)),
    };
    found
        .or(outputs.first())
        .cloned()
        .ok_or(CaptureError::NoOutput)
}

/// Create a D3D11 device on a specific adapter, usable from several threads
/// (Media Foundation encoders call into it from their own threads).
pub fn create_device(
    adapter_index: u32,
) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    // SAFETY: standard device creation; out-params receive owned interfaces.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(adapter_index)?;
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.ok_or_else(|| CaptureError::Other("no D3D11 device".into()))?;
        let context = context.ok_or_else(|| CaptureError::Other("no D3D11 context".into()))?;
        let mt: ID3D11Multithread = device.cast()?;
        let _ = mt.SetMultithreadProtected(true);
        Ok((device, context))
    }
}

pub struct DesktopCapture {
    pub info: OutputInfo,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    dupl: Option<IDXGIOutputDuplication>,
    frame: Option<ID3D11Texture2D>,
    hdr: bool,
    pointer: Pointer,
}

/// Mouse pointer as reported by Desktop Duplication (output coordinates).
#[derive(Debug, Clone, Default)]
pub struct Pointer {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub image: Option<std::sync::Arc<CursorImage>>,
    /// Bumped whenever `image` changes.
    pub version: u64,
}

impl DesktopCapture {
    pub fn new(
        info: OutputInfo,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
    ) -> Result<Self, CaptureError> {
        // SAFETY: DXGI queries on interfaces we own.
        let output: IDXGIOutput1 = unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(info.adapter_index)?;
            adapter.EnumOutputs(info.output_index)?.cast()?
        };
        let mut me = Self {
            info,
            device,
            context,
            output,
            dupl: None,
            frame: None,
            hdr: false,
            pointer: Pointer::default(),
        };
        me.reopen()?;
        Ok(me)
    }

    fn reopen(&mut self) -> Result<(), CaptureError> {
        self.dupl = None;
        // HDR desktops are duplicated as scRGB FP16 so highlights survive and
        // can be tone-mapped; SDR desktops stay BGRA8.
        // SAFETY: device belongs to the output's adapter.
        unsafe {
            self.hdr = self
                .output
                .cast::<IDXGIOutput6>()
                .and_then(|o| o.GetDesc1())
                .is_ok_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
            let formats: &[DXGI_FORMAT] = if self.hdr {
                &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM]
            } else {
                &[DXGI_FORMAT_B8G8R8A8_UNORM]
            };
            // DuplicateOutput1 requires a per-monitor-v2 DPI aware process;
            // fall back to the legacy call (SDR only) otherwise.
            let modern = self
                .output
                .cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(&self.device, 0, formats));
            let dupl = match modern {
                Ok(d) => d,
                Err(e) => {
                    if self.hdr {
                        tracing::warn!(error = %e, "HDR duplication unavailable, capturing as SDR");
                    }
                    self.hdr = false;
                    self.output.DuplicateOutput(&self.device)?
                }
            };
            self.dupl = Some(dupl);
        }
        Ok(())
    }

    /// Latest full desktop image (BGRA8, or FP16 scRGB when `is_hdr`).
    pub fn frame(&self) -> Option<&ID3D11Texture2D> {
        self.frame.as_ref()
    }

    pub fn is_hdr(&self) -> bool {
        self.hdr
    }

    pub fn pointer(&self) -> &Pointer {
        &self.pointer
    }

    /// Wait up to `timeout_ms` for a new desktop image and copy it into our
    /// own texture. Returns `Ok(true)` if the image changed.
    pub fn acquire(&mut self, timeout_ms: u32) -> Result<bool, CaptureError> {
        if self.dupl.is_none() && self.reopen().is_err() {
            return Err(CaptureError::AccessLost);
        }
        let Some(dupl) = self.dupl.clone() else {
            return Err(CaptureError::AccessLost);
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: out-params owned by this frame; ReleaseFrame is always called on success.
        match unsafe { dupl.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            Err(e) if e.code() == DXGI_ERROR_INVALID_CALL => {
                // Seen transiently right after a frame: release and retry next tick.
                // SAFETY: releasing a frame we may still hold is harmless.
                unsafe {
                    let _ = dupl.ReleaseFrame();
                }
                return Ok(false);
            }
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.dupl = None;
                return Err(CaptureError::AccessLost);
            }
            Err(e) => {
                self.dupl = None;
                return Err(e.into());
            }
        }
        if info.LastMouseUpdateTime != 0 {
            self.pointer.visible = info.PointerPosition.Visible.as_bool();
            self.pointer.x = info.PointerPosition.Position.x;
            self.pointer.y = info.PointerPosition.Position.y;
        }
        if info.PointerShapeBufferSize > 0 {
            let mut buf = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut needed = 0u32;
            let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            // SAFETY: buffer sized as DXGI requested.
            let ok = unsafe {
                dupl.GetFramePointerShape(
                    buf.len() as u32,
                    buf.as_mut_ptr().cast(),
                    &mut needed,
                    &mut shape,
                )
            };
            if ok.is_ok()
                && let Some(img) =
                    shape_to_bgra(shape.Type, shape.Width, shape.Height, shape.Pitch, &buf)
            {
                self.pointer.image = Some(std::sync::Arc::new(img));
                self.pointer.version += 1;
            }
        }
        // LastPresentTime == 0 means only the cursor moved; but always take the
        // very first image so a static screen still produces video.
        let updated = info.LastPresentTime != 0 || self.frame.is_none();
        let result = if updated {
            resource
                .ok_or_else(|| CaptureError::Other("no frame resource".into()))
                .and_then(|r| self.copy_frame(&r))
        } else {
            Ok(())
        };
        // SAFETY: matching ReleaseFrame for the successful AcquireNextFrame.
        unsafe {
            let _ = dupl.ReleaseFrame();
        }
        result.map(|()| updated)
    }

    fn copy_frame(&mut self, resource: &IDXGIResource) -> Result<(), CaptureError> {
        // SAFETY: the duplication surface is a Texture2D on our device.
        unsafe {
            let src: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut desc);
            let recreate = match &self.frame {
                Some(f) => {
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    f.GetDesc(&mut d);
                    d.Width != desc.Width || d.Height != desc.Height || d.Format != desc.Format
                }
                None => true,
            };
            if recreate {
                let own = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                self.device.CreateTexture2D(&own, None, Some(&mut tex))?;
                self.frame = tex;
            }
            if let Some(dst) = &self.frame {
                self.context.CopyResource(dst, &src);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_at_least_one_output() {
        // CI runners may have no monitor; only assert the call succeeds.
        let outs = list_outputs().unwrap();
        for o in &outs {
            assert!(o.width() > 0 && o.height() > 0);
        }
    }
}
