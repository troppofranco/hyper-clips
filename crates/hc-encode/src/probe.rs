//! Read back an MP4 with the Media Foundation source reader: used by tests
//! and `hc-bench probe` to prove files are decodable by a standard demuxer.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::*;
use windows::core::{HSTRING, Interface};

use crate::mf::init_thread;

#[derive(Debug, Clone, Default)]
pub struct StreamReport {
    pub index: u32,
    pub major: String,
    pub subtype: String,
    pub samples: u64,
    pub first_pts: i64,
    pub last_pts: i64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeReport {
    pub duration_ticks: i64,
    pub streams: Vec<StreamReport>,
}

/// Decode the video frame nearest to `at_ticks` into BGRA pixels
/// (width, height, stride, bytes). Used to eyeball captures in tests.
pub fn decode_frame(path: &Path, at_ticks: i64) -> windows::core::Result<(u32, u32, u32, Vec<u8>)> {
    init_thread();
    // SAFETY: Media Foundation reader calls on objects we own.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &t)?;
        let cur = reader.GetCurrentMediaType(stream)?;
        let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, (size & 0xFFFF_FFFF) as u32);
        let mut last = None;
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
                last = Some(s);
                if ts >= at_ticks {
                    break;
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        let s = last.ok_or_else(windows::core::Error::empty)?;
        let bytes = crate::mf::sample_bytes(&s)?;
        let stride = frame_stride(cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok(), w, h, bytes.len());
        // Encoders pad 1080p to 1088 coded rows; show only the real picture.
        let h = if h == 1088 { 1080 } else { h };
        Ok((w, h, stride, bytes))
    }
}

/// Row pitch of a decoded RGB32 frame. Prefer what the decoder declares;
/// otherwise rows are tightly packed (`w * 4`) when the buffer is a whole
/// number of such rows, which also covers extra padded rows at the bottom.
/// Dividing the size by the height (the old approach) gave a slightly wrong
/// pitch whenever rows were added, and the picture came out sheared.
fn frame_stride(declared: Option<u32>, w: u32, h: u32, len: usize) -> u32 {
    let tight = w * 4;
    if let Some(s) = declared.map(|s| (s as i32).unsigned_abs())
        && s >= tight
        && len >= s as usize * h as usize
    {
        return s;
    }
    if tight > 0 && len.is_multiple_of(tight as usize) && len >= tight as usize * h as usize {
        return tight;
    }
    ((len / h.max(1) as usize) as u32).max(tight)
}

/// Count every compressed sample of every stream (no decoding).
pub fn probe(path: &Path) -> windows::core::Result<ProbeReport> {
    init_thread();
    // SAFETY: Media Foundation reader calls on objects we own.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let mut report = ProbeReport::default();
        if let Ok(v) =
            reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
        {
            let inner = &*v.Anonymous.Anonymous;
            report.duration_ticks = inner.Anonymous.uhVal as i64;
        }
        let mut i = 0u32;
        while let Ok(t) = reader.GetNativeMediaType(i, 0) {
            let major = t.GetGUID(&MF_MT_MAJOR_TYPE).unwrap_or_default();
            let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
            let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
            reader.SetStreamSelection(i, true)?;
            // Keep compressed output: set the native type as current.
            let _ = reader.SetCurrentMediaType(i, None, &t);
            report.streams.push(StreamReport {
                index: i,
                major: if major == MFMediaType_Video {
                    "video".into()
                } else if major == MFMediaType_Audio {
                    "audio".into()
                } else {
                    format!("{major:?}")
                },
                subtype: if sub == MFVideoFormat_H264 {
                    "H264".into()
                } else if sub == MFVideoFormat_HEVC {
                    "HEVC".into()
                } else if sub == MFVideoFormat_AV1 {
                    "AV1".into()
                } else if sub == MFAudioFormat_AAC {
                    "AAC".into()
                } else {
                    format!("{sub:?}")
                },
                width: (size >> 32) as u32,
                height: (size & 0xFFFF_FFFF) as u32,
                first_pts: -1,
                ..Default::default()
            });
            i += 1;
        }
        loop {
            let mut actual = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut actual),
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                break;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                let _ = reader.SetStreamSelection(actual, false);
            }
            if let Some(_s) = sample
                && let Some(st) = report.streams.get_mut(actual as usize)
            {
                st.samples += 1;
                if st.first_pts < 0 {
                    st.first_pts = ts;
                }
                st.last_pts = ts;
            }
            let any = report.streams.iter().any(|s| {
                reader
                    .GetStreamSelection(s.index)
                    .is_ok_and(|b| b.as_bool())
            });
            if !any {
                break;
            }
        }
        Ok(report)
    }
}

/// Decode every audio stream to PCM and return (stream index, MP4 track id,
/// RMS dBFS, peak dBFS). Used to verify that per-process tracks carry sound.
pub fn audio_levels(path: &Path) -> windows::core::Result<Vec<(u32, String, f64, f64)>> {
    init_thread();
    // SAFETY: Media Foundation reader calls on objects we own.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let mut audio = Vec::new();
        let mut i = 0u32;
        while let Ok(t) = reader.GetNativeMediaType(i, 0) {
            let is_audio = t
                .GetGUID(&MF_MT_MAJOR_TYPE)
                .is_ok_and(|g| g == MFMediaType_Audio);
            reader.SetStreamSelection(i, is_audio)?;
            if is_audio {
                let pcm = MFCreateMediaType()?;
                pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                reader.SetCurrentMediaType(i, None, &pcm)?;
                audio.push(i);
            }
            i += 1;
        }
        let mut acc: Vec<(f64, u64, i32)> = vec![(0.0, 0, 0); i as usize];
        loop {
            let mut actual = 0u32;
            let mut flags = 0u32;
            let mut sample = None;
            reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut actual),
                Some(&mut flags),
                None,
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                break;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                let _ = reader.SetStreamSelection(actual, false);
            }
            if let Some(s) = sample {
                let bytes = crate::mf::sample_bytes(&s)?;
                let a = &mut acc[actual as usize];
                for c in bytes.as_chunks::<2>().0 {
                    let v = i16::from_le_bytes(*c);
                    a.0 += f64::from(v) * f64::from(v);
                    a.1 += 1;
                    a.2 = a.2.max(i32::from(v).abs());
                }
            }
            let any = audio
                .iter()
                .any(|s| reader.GetStreamSelection(*s).is_ok_and(|b| b.as_bool()));
            if !any {
                break;
            }
        }
        let db = |x: f64| {
            if x <= 0.0 {
                -120.0
            } else {
                20.0 * (x / 32768.0).log10()
            }
        };
        Ok(audio
            .into_iter()
            .map(|s| {
                let (sum, n, peak) = acc[s as usize];
                let rms = (sum / n.max(1) as f64).sqrt();
                (s, stream_name(&reader, s), db(rms), db(f64::from(peak)))
            })
            .collect())
    }
}

/// Stream identifier reported by the media source for a reader stream.
fn stream_name(reader: &IMFSourceReader, stream: u32) -> String {
    // SAFETY: presentation descriptor queries on the reader's media source.
    unsafe {
        let mut raw = std::ptr::null_mut();
        let source: windows::core::Result<IMFMediaSource> = reader
            .GetServiceForStream(
                MF_SOURCE_READER_MEDIASOURCE.0 as u32,
                &windows::core::GUID::zeroed(),
                &IMFMediaSource::IID,
                &mut raw,
            )
            .map(|()| IMFMediaSource::from_raw(raw));
        let id = source
            .and_then(|s| s.CreatePresentationDescriptor())
            .and_then(|pd| {
                let mut selected = windows::core::BOOL::default();
                let mut sd = None;
                pd.GetStreamDescriptorByIndex(stream, &mut selected, &mut sd)?;
                sd.ok_or_else(windows::core::Error::empty)?
                    .GetStreamIdentifier()
            });
        id.map(|i| format!("id {i}")).unwrap_or_default()
    }
}

#[cfg(test)]
mod stride_tests {
    use super::frame_stride;

    #[test]
    fn stride_detection() {
        // 1920x1080 frame decoded with 1088 rows: pitch is still 7680.
        assert_eq!(frame_stride(None, 1920, 1080, 7680 * 1088), 7680);
        // Declared pitch wins (bottom-up = negative).
        assert_eq!(
            frame_stride(Some((-8192i32) as u32), 1920, 1080, 8192 * 1080),
            8192
        );
        // Padded pitch, no declaration, exact rows.
        assert_eq!(frame_stride(None, 1000, 10, 4096 * 10), 4096);
    }
}
