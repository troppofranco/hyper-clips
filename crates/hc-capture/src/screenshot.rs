//! Save a captured BGRA frame as PNG or JPEG with the Windows Imaging
//! Component (built into Windows: no extra dependencies, no metadata written).

use std::path::Path;

use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_ContainerFormatPng,
    GUID_WICPixelFormat32bppBGR, IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::HSTRING;

use crate::CaptureError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    /// Lossless WebP (pure-Rust encoder: WIC has no WebP encoder).
    Webp,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }
}

/// Encode BGRA/BGRX pixels in memory to `path`. COM must be initialised.
pub fn save_pixels(
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
    path: &Path,
    format: ImageFormat,
) -> Result<(), CaptureError> {
    if format == ImageFormat::Webp {
        return save_webp(width, height, stride, pixels, path);
    }
    // SAFETY: WIC objects are ref-counted; pixel slice outlives the calls.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = factory.CreateBitmapFromMemory(
            width,
            height,
            &GUID_WICPixelFormat32bppBGR,
            stride,
            pixels,
        )?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let container = match format {
            ImageFormat::Png => GUID_ContainerFormatPng,
            ImageFormat::Jpeg | ImageFormat::Webp => GUID_ContainerFormatJpeg,
        };
        let encoder = factory.CreateEncoder(&container, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame_enc = None;
        encoder.CreateNewFrame(&mut frame_enc, std::ptr::null_mut())?;
        let frame_enc = frame_enc.ok_or_else(|| CaptureError::Other("WIC frame".into()))?;
        frame_enc.Initialize(None)?;
        frame_enc.SetSize(width, height)?;
        // WriteSource converts to whatever the container needs (24bpp for JPEG).
        frame_enc.WriteSource(&bitmap, std::ptr::null())?;
        frame_enc.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}

/// Copy `frame` to the CPU and encode it to `path`. COM must be initialised.
pub fn save(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &ID3D11Texture2D,
    path: &Path,
    format: ImageFormat,
) -> Result<(), CaptureError> {
    // SAFETY: staging copy + map/unmap on our device.
    let (desc, pitch, pixels) = unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        frame.GetDesc(&mut desc);
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut staging = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| CaptureError::Other("staging texture".into()))?;
        context.CopyResource(&staging, frame);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let len = (mapped.RowPitch * desc.Height) as usize;
        let pixels = std::slice::from_raw_parts(mapped.pData as *const u8, len).to_vec();
        context.Unmap(&staging, 0);
        (desc, mapped.RowPitch, pixels)
    };
    save_pixels(desc.Width, desc.Height, pitch, &pixels, path, format)
}

/// Decode any image WIC understands (PNG, JPEG, BMP, WebP with the codec
/// installed, …) into top-down BGRA: (width, height, pixels).
pub fn load_image(path: &Path) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
        WICDecodeMetadataCacheOnDemand,
    };
    // SAFETY: WIC objects are ref-counted; the output buffer is sized from the frame.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory.CreateDecoderFromFilename(
            &HSTRING::from(path.as_os_str()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        Ok((w, h, px))
    }
}

/// Area-average downscale of BGRA pixels to fit within `max_w` × `max_h`.
pub fn downscale(
    w: u32,
    h: u32,
    stride: u32,
    px: &[u8],
    max_w: u32,
    max_h: u32,
) -> (u32, u32, Vec<u8>) {
    let scale = (f64::from(max_w) / f64::from(w.max(1)))
        .min(f64::from(max_h) / f64::from(h.max(1)))
        .min(1.0);
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1).min(h));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1).min(w));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in y0..y1 {
                let row = (sy * stride) as usize;
                for sx in x0..x1 {
                    let i = row + (sx * 4) as usize;
                    if let Some(p) = px.get(i..i + 4) {
                        for c in 0..4 {
                            acc[c] += u32::from(p[c]);
                        }
                        n += 1;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (nw, nh, out)
}

fn save_webp(
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
    path: &Path,
) -> Result<(), CaptureError> {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height as usize {
        let row = pixels
            .get(y * stride as usize..y * stride as usize + width as usize * 4)
            .ok_or_else(|| CaptureError::Other("short pixel buffer".into()))?;
        for px in row.as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let file = std::fs::File::create(path).map_err(|e| CaptureError::Other(e.to_string()))?;
    image_webp::WebPEncoder::new(std::io::BufWriter::new(file))
        .encode(&rgb, width, height, image_webp::ColorType::Rgb8)
        .map_err(|e| CaptureError::Other(format!("webp: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_webp() {
        let path = std::env::temp_dir().join(format!("hc-test-{}.webp", std::process::id()));
        let px: Vec<u8> = (0..16 * 8).flat_map(|i| [i as u8, 0, 255, 255]).collect();
        save_pixels(16, 8, 64, &px, &path, ImageFormat::Webp).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        std::fs::remove_file(path).unwrap();
    }
}
