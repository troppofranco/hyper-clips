//! Read back an MP4 with the Media Foundation source reader: used by tests
//! and `hc-bench probe` to prove files are decodable by a standard demuxer.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::*;
use windows::core::HSTRING;

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
        Ok((w, h, w * 4, bytes))
    }
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
