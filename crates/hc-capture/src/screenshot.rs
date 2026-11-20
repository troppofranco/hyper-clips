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
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
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
            ImageFormat::Jpeg => GUID_ContainerFormatJpeg,
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
