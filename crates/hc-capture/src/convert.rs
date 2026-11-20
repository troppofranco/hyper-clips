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
    #[allow(clippy::too_many_arguments)] // flat D3D parameters read clearer than a one-off struct
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        fps: u32,
        pool_size: usize,
    ) -> Result<Self, CaptureError> {
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
            let src = RECT {
                left: 0,
                top: 0,
                right: in_w as i32,
                bottom: in_h as i32,
            };
            let dst = RECT {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            };
            vctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
            vctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
            vctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst));

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
    fn sizes() {
        assert_eq!(output_size(2560, 1440, 1080), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 1080), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 1440), (1920, 1080));
        assert_eq!(output_size(1440, 2560, 1080), (606, 1080));
        assert_eq!(output_size(1366, 768, 0), (1366, 768));
        assert_eq!(output_size(3440, 1440, 720), (1720, 720));
    }
}
