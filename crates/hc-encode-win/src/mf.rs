//! Media Foundation start-up and shared helpers.

use crate::VideoInfo;
use hc_core::error::{Error, Result};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub(crate) fn sys<E: std::fmt::Display>(what: &str, e: E) -> Error {
    Error::System(format!("{what}: {e}"))
}

/// Initialises COM (multithreaded) and Media Foundation for the calling thread/process.
/// Safe to call repeatedly.
pub fn init() -> Result<()> {
    // SAFETY: plain FFI; "already initialised" results are harmless and ignored.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: plain FFI.
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| sys("MFStartup", e))
}

/// Builds an H.264 video media type for `info`.
pub(crate) fn h264_type(info: &VideoInfo, sequence_header: &[u8]) -> Result<IMFMediaType> {
    // SAFETY: all calls are plain COM calls on objects created here.
    unsafe {
        let t = MFCreateMediaType().map_err(|e| sys("MFCreateMediaType", e))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| sys("major type", e))?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| sys("subtype", e))?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, info.bitrate_bps)
            .map_err(|e| sys("bitrate", e))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| sys("interlace", e))?;
        MFSetAttributeSize(&t, &MF_MT_FRAME_SIZE, info.width, info.height)
            .map_err(|e| sys("frame size", e))?;
        MFSetAttributeRatio(&t, &MF_MT_FRAME_RATE, info.fps, 1)
            .map_err(|e| sys("frame rate", e))?;
        MFSetAttributeRatio(&t, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)
            .map_err(|e| sys("pixel aspect", e))?;
        if !sequence_header.is_empty() {
            t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, sequence_header)
                .map_err(|e| sys("sequence header", e))?;
        }
        Ok(t)
    }
}
