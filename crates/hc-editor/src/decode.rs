//! Source readers for export: video frames as D3D11 textures (DXVA hardware
//! decode when available) and audio tracks as 48 kHz stereo PCM.

use std::mem::ManuallyDrop;
use std::path::Path;

use hc_capture::compose::{Overlay, srv_for};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows::core::{HSTRING, Interface};

pub const RATE: usize = 48_000;
const TICKS_PER_MS: i64 = 10_000;

fn seek(reader: &IMFSourceReader, ms: u64) -> windows::core::Result<()> {
    let mut pv = ManuallyDrop::new(PROPVARIANT::default());
    // SAFETY: filling the VT_I8 member of a zeroed PROPVARIANT (no heap data).
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = ms as i64 * TICKS_PER_MS;
        reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &*pv)
    }
}

/// Number of audio streams in a file (= our audio tracks, in order).
pub fn audio_stream_count(path: &Path) -> usize {
    hc_encode::mf::init_thread();
    // SAFETY: reader queries.
    unsafe {
        let Ok(reader) = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) else {
            return 0;
        };
        let mut n = 0;
        let mut i = 0u32;
        while let Ok(t) = reader.GetNativeMediaType(i, 0) {
            if t.GetGUID(&MF_MT_MAJOR_TYPE)
                .is_ok_and(|g| g == MFMediaType_Audio)
            {
                n += 1;
            }
            i += 1;
        }
        n
    }
}

/// Encoders pad 1080p to 1088 coded lines; show the real picture height.
pub fn display_height(h: u32) -> u32 {
    if h == 1088 { 1080 } else { h }
}

/// Native video size of a file.
pub fn video_size(path: &Path) -> Option<(u32, u32)> {
    let r = hc_encode::probe::probe(path).ok()?;
    let v = r.streams.iter().find(|s| s.major == "video")?;
    Some((v.width, display_height(v.height)))
}
/// Sequential video reader that returns the frame shown at a given time.
pub struct VideoReader {
    reader: IMFSourceReader,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    current: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    pending: Option<(i64, IMFSample)>,
    ended: bool,
    pub width: u32,
    pub height: u32,
    _manager: Option<IMFDXGIDeviceManager>,
}

impl VideoReader {
    pub fn open(
        path: &Path,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        start_ms: u64,
    ) -> windows::core::Result<Self> {
        hc_encode::mf::init_thread();
        // SAFETY: MF reader setup; all objects are ref-counted.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or_else(windows::core::Error::empty)?;
            manager.ResetDevice(device, token)?;
            // Hardware decode + GPU colour conversion into DXGI surfaces.
            attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager)?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(stream, true)?;
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(stream, None, &t)?;
            let cur = reader.GetCurrentMediaType(stream)?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let mut me = Self {
                reader,
                device: device.clone(),
                context: context.clone(),
                current: None,
                pending: None,
                ended: false,
                width: (size >> 32) as u32,
                height: display_height((size & 0xFFFF_FFFF) as u32),
                _manager: Some(manager),
            };
            if start_ms > 0 {
                seek(&me.reader, start_ms)?;
            }
            me.read_next()?;
            Ok(me)
        }
    }

    fn read_next(&mut self) -> windows::core::Result<()> {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        // SAFETY: synchronous ReadSample on our reader.
        unsafe {
            self.reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            self.ended = true;
        }
        self.pending = sample.map(|s| (ts, s));
        Ok(())
    }

    fn ensure_texture(
        &mut self,
        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    ) -> windows::core::Result<()> {
        if self.current.is_some() {
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        // SAFETY: plain texture creation on our device.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        let tex = tex.ok_or_else(windows::core::Error::empty)?;
        let srv = srv_for(&self.device, &tex).map_err(|_| windows::core::Error::empty())?;
        self.current = Some((tex, srv));
        Ok(())
    }

    fn upload(&mut self, sample: &IMFSample) -> windows::core::Result<()> {
        // SAFETY: buffer/texture access on our device.
        unsafe {
            let buffer = sample.GetBufferByIndex(0)?;
            if let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() {
                // GPU path: copy the decoded surface (possibly a texture-array
                // slice). Our texture must use the same format (often BGRX).
                let mut raw = std::ptr::null_mut();
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
                let src = ID3D11Texture2D::from_raw(raw);
                let sub = dxgi.GetSubresourceIndex()?;
                let mut sd = D3D11_TEXTURE2D_DESC::default();
                src.GetDesc(&mut sd);
                self.ensure_texture(sd.Format)?;
                let Some((dst, _)) = &self.current else {
                    return Ok(());
                };
                let bx = windows::Win32::Graphics::Direct3D11::D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: self.width.min(sd.Width),
                    bottom: self.height.min(sd.Height),
                    back: 1,
                };
                self.context
                    .CopySubresourceRegion(dst, 0, 0, 0, 0, &src, sub, Some(&bx));
            } else {
                // CPU path: upload the RGB32 bytes.
                self.ensure_texture(DXGI_FORMAT_B8G8R8A8_UNORM)?;
                let Some((dst, _)) = &self.current else {
                    return Ok(());
                };
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut ptr, None, Some(&mut len))?;
                let pitch = len / self.height.max(1);
                if pitch >= self.width * 4 {
                    self.context
                        .UpdateSubresource(dst, 0, None, ptr.cast(), pitch, 0);
                }
                buffer.Unlock()?;
            }
        }
        Ok(())
    }
    /// Texture showing the frame at `t_ms` (source time). Times must not decrease.
    pub fn frame_at(&mut self, t_ms: f64) -> Option<&ID3D11ShaderResourceView> {
        let t = (t_ms * TICKS_PER_MS as f64) as i64;
        let mut guard = 0;
        while let Some((pts, _)) = &self.pending {
            if *pts > t && self.current.is_some() {
                break;
            }
            let Some((_, sample)) = self.pending.take() else {
                break;
            };
            if let Err(e) = self.upload(&sample) {
                tracing::warn!(error = %e, "frame upload failed");
            }
            if self.ended || self.read_next().is_err() {
                break;
            }
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        self.current.as_ref().map(|(_, srv)| srv)
    }
}

/// Decode audio stream `track` (0-based among audio streams) between
/// `in_ms` and `out_ms` as interleaved stereo i16 at 48 kHz.
pub fn decode_audio(
    path: &Path,
    track: usize,
    in_ms: u64,
    out_ms: u64,
) -> windows::core::Result<Vec<i16>> {
    hc_encode::mf::init_thread();
    let want = ((out_ms.saturating_sub(in_ms)) as usize * RATE / 1000) * 2;
    // SAFETY: MF reader usage; buffers locked/unlocked around copies.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        let mut idx = None;
        let mut i = 0u32;
        let mut seen = 0usize;
        while let Ok(t) = reader.GetNativeMediaType(i, 0) {
            if t.GetGUID(&MF_MT_MAJOR_TYPE)
                .is_ok_and(|g| g == MFMediaType_Audio)
            {
                if seen == track {
                    idx = Some(i);
                }
                seen += 1;
            }
            i += 1;
        }
        let Some(stream) = idx else {
            return Ok(vec![0; want]);
        };
        reader.SetStreamSelection(stream, true)?;
        let pcm = MFCreateMediaType()?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        reader.SetCurrentMediaType(stream, None, &pcm)?;
        let cur = reader.GetCurrentMediaType(stream)?;
        let rate = cur
            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .unwrap_or(48_000) as usize;
        let ch = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2) as usize;
        if in_ms > 0 {
            seek(&reader, in_ms)?;
        }
        let start_tick = in_ms as i64 * TICKS_PER_MS;
        let mut raw: Vec<i16> = Vec::with_capacity(want);
        let mut first_ts = None;
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            if let Some(s) = sample {
                first_ts.get_or_insert(ts);
                let buf = s.ConvertToContiguousBuffer()?;
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut ptr, None, Some(&mut len))?;
                let data = std::slice::from_raw_parts(ptr as *const i16, len as usize / 2);
                for frame in data.chunks(ch.max(1)) {
                    let l = frame[0];
                    let r = if ch > 1 { frame[1] } else { frame[0] };
                    raw.push(l);
                    raw.push(r);
                }
                buf.Unlock()?;
            }
            let have_ms = raw.len() / 2 * 1000 / rate.max(1);
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0
                || have_ms as u64 > out_ms - in_ms + 200
            {
                break;
            }
        }
        // Seeking lands on the previous packet: skip the excess at the front.
        let skip_frames = first_ts
            .map(|f| ((start_tick - f).max(0) as usize * rate) / 10_000_000)
            .unwrap_or(0);
        let raw = raw
            .get(skip_frames * 2..)
            .map(<[i16]>::to_vec)
            .unwrap_or_default();
        let mut out = if rate == RATE {
            raw
        } else {
            resample(&raw, rate as f64 / RATE as f64)
        };
        out.resize(want, 0);
        Ok(out)
    }
}

/// Linear-interpolation resample of interleaved stereo by `step` input
/// frames per output frame (also used for speed changes).
pub fn resample(src: &[i16], step: f64) -> Vec<i16> {
    let frames = src.len() / 2;
    if frames == 0 || step <= 0.0 {
        return Vec::new();
    }
    let out_frames = (frames as f64 / step) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for k in 0..out_frames {
        let pos = k as f64 * step;
        let i = pos as usize;
        let frac = pos - i as f64;
        for c in 0..2 {
            let a = f64::from(src[(i.min(frames - 1)) * 2 + c]);
            let b = f64::from(src[((i + 1).min(frames - 1)) * 2 + c]);
            out.push((a + (b - a) * frac) as i16);
        }
    }
    out
}

/// Image overlay (sticker/logo) loaded into a GPU texture.
pub fn load_overlay_image(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    path: &Path,
) -> Option<Overlay> {
    let (w, h, bgra) = hc_capture::screenshot::load_image(path).ok()?;
    let ov = Overlay::new(device, w, h).ok()?;
    ov.upload(context, &bgra);
    Some(ov)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_lengths() {
        let src: Vec<i16> = (0..2000).map(|i| (i % 100) as i16).collect();
        assert_eq!(resample(&src, 2.0).len(), 1000);
        assert_eq!(resample(&src, 0.5).len(), 4000);
        assert_eq!(resample(&src, 1.0), src);
    }
}
