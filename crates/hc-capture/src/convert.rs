//! BGRA → NV12 conversion + scaling on the GPU with the D3D11 video processor
//! (fixed-function hardware on all vendors: no shaders, minimal GPU load).

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::CaptureError;

/// Output size for a source, scaled to `target_height` (0 = keep), preserving
/// aspect ratio, rounded to even dimensions as NV12 requires.
pub fn output_size(src_w: u32, src_h: u32, target_height: u32) -> (u32, u32) {
    let (w, h) = if target_height == 0 || target_height >= src_h {
        (src_w, src_h)
    } else {
        let w = (u64::from(src_w) * u64::from(target_height) / u64::from(src_h)) as u32;
        (w, target_height)
    };
    (w.max(2) & !1, h.max(2) & !1)
}

/// Fit src (w, h) inside dst (w, h) preserving aspect: returns the
/// centred destination rectangle (letterbox / pillarbox).
pub fn fit_rect(src: (u32, u32), dst: (u32, u32)) -> RECT {
    let (sw, sh) = (u64::from(src.0.max(1)), u64::from(src.1.max(1)));
    let (dw, dh) = (u64::from(dst.0), u64::from(dst.1));
    let (w, h) = if sw * dh > dw * sh {
        (dw, sh * dw / sw)
    } else {
        (sw * dh / sh, dh)
    };
    let x = (dw - w) / 2;
    let y = (dh - h) / 2;
    RECT {
        left: x as i32,
        top: y as i32,
        right: (x + w) as i32,
        bottom: (y + h) as i32,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConvertParams {
    pub in_width: u32,
    pub in_height: u32,
    /// Region of the input to keep (monitor-region capture); None = all.
    pub crop: Option<RECT>,
    pub out_width: u32,
    pub out_height: u32,
    pub fps: u32,
    pub pool_size: usize,
}

pub struct Nv12Converter {
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input: Option<(usize, ID3D11VideoProcessorInputView)>,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    pub out_width: u32,
    pub out_height: u32,
}

impl Nv12Converter {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        p: &ConvertParams,
    ) -> Result<Self, CaptureError> {
        let (in_w, in_h, out_w, out_h, fps, pool_size) = (
            p.in_width,
            p.in_height,
            p.out_width,
            p.out_height,
            p.fps,
            p.pool_size,
        );
        // SAFETY: D3D11 video API calls on interfaces we own.
        unsafe {
            let vdev: ID3D11VideoDevice = device.cast()?;
            let vctx: ID3D11VideoContext = context.cast()?;
            let rate = DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: in_w,
                InputHeight: in_h,
                OutputFrameRate: rate,
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = vdev.CreateVideoProcessorEnumerator(&content)?;
            let processor = vdev.CreateVideoProcessor(&enumerator, 0)?;

            // Full-range RGB in, BT.709 limited-range YCbCr out.
            let rgb_full = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let yuv_709 = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            vctx.VideoProcessorSetStreamColorSpace(&processor, 0, &rgb_full);
            vctx.VideoProcessorSetOutputColorSpace(&processor, &yuv_709);
            vctx.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            vctx.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            let full = RECT {
                left: 0,
                top: 0,
                right: in_w as i32,
                bottom: in_h as i32,
            };
            let src = p.crop.unwrap_or(full);
            let src_size = (
                (src.right - src.left).max(1) as u32,
                (src.bottom - src.top).max(1) as u32,
            );
            let dst = fit_rect(src_size, (out_w, out_h));
            let target = RECT {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            };
            vctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
            vctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
            vctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target));
            // Black bars around letterboxed content (RGB black).
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    RGBA: D3D11_VIDEO_COLOR_RGBA {
                        R: 0.0,
                        G: 0.0,
                        B: 0.0,
                        A: 1.0,
                    },
                },
            };
            vctx.VideoProcessorSetOutputBackgroundColor(&processor, false, &black);

            let mut pool = Vec::with_capacity(pool_size);
            for _ in 0..pool_size.max(2) {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: out_w,
                    Height: out_h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                device.CreateTexture2D(&desc, None, Some(&mut tex))?;
                let tex = tex.ok_or_else(|| CaptureError::Other("NV12 texture".into()))?;
                let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut view = None;
                vdev.CreateVideoProcessorOutputView(
                    &tex,
                    &enumerator,
                    &view_desc,
                    Some(&mut view),
                )?;
                let view = view.ok_or_else(|| CaptureError::Other("output view".into()))?;
                pool.push((tex, view));
            }
            Ok(Self {
                vdev,
                vctx,
                enumerator,
                processor,
                input: None,
                pool,
                next: 0,
                out_width: out_w,
                out_height: out_h,
            })
        }
    }

    /// Convert `src` into the next pool texture and return it.
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D, CaptureError> {
        let key = src.as_raw() as usize;
        if self.input.as_ref().map(|(k, _)| *k) != Some(key) {
            let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut view = None;
            // SAFETY: src is a BGRA texture on the same device.
            unsafe {
                self.vdev.CreateVideoProcessorInputView(
                    src,
                    &self.enumerator,
                    &desc,
                    Some(&mut view),
                )?;
            }
            let view = view.ok_or_else(|| CaptureError::Other("input view".into()))?;
            self.input = Some((key, view));
        }
        let Some((_, input_view)) = &self.input else {
            return Err(CaptureError::Other("input view".into()));
        };
        let (tex, out_view) = &self.pool[self.next];
        self.next = (self.next + 1) % self.pool.len();
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: ManuallyDrop::new(Some(input_view.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let streams = [stream];
        // SAFETY: views belong to this processor's enumerator.
        let res = unsafe {
            self.vctx
                .VideoProcessorBlt(&self.processor, out_view, 0, &streams)
        };
        // Release the clone held by the ManuallyDrop field.
        let [mut s] = streams;
        // SAFETY: taken exactly once.
        unsafe { ManuallyDrop::drop(&mut s.pInputSurface) };
        res?;
        Ok(tex.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox() {
        let r = fit_rect((1440, 2560), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (656, 0, 1263, 1080));
        let r = fit_rect((1920, 1080), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
        let r = fit_rect((3440, 1440), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 138, 1920, 941));
    }

    #[test]
    fn sizes() {
        assert_eq!(output_size(2560, 1440, 1080), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 1080), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 1440), (1920, 1080));
        assert_eq!(output_size(1440, 2560, 1080), (606, 1080));
        assert_eq!(output_size(1366, 768, 0), (1366, 768));
        assert_eq!(output_size(3440, 1440, 720), (1720, 720));
    }
}
