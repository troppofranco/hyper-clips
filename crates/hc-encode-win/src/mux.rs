//! Writes already-encoded packets to an MP4 file without re-encoding.

use crate::aac::aac_type;
use crate::mf::{h264_type, sys};
use crate::{annexb, AudioInfo, VideoInfo};
use hc_buffer::Snapshot;
use hc_core::error::{Error, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;

/// Saves `snapshot` as an MP4 at `path`: track 0 is H.264 video, tracks 1.. are AAC audio
/// described by `audio` (same order; empty tracks are skipped). Timestamps are shifted so the
/// file starts at zero.
pub fn write_mp4(
    path: &Path,
    info: &VideoInfo,
    audio: &[AudioInfo],
    snapshot: &Snapshot,
) -> Result<()> {
    crate::mf::init()?;
    let video = snapshot
        .tracks
        .first()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Error::Invalid("nothing to save".into()))?;

    // The MP4 sink needs SPS/PPS; take them from the first keyframe when the encoder
    // did not report them.
    let header = if info.sequence_header.is_empty() {
        video
            .iter()
            .find(|p| p.keyframe)
            .map(|p| annexb::sequence_header(&p.data))
            .unwrap_or_default()
    } else {
        info.sequence_header.clone()
    };
    let media_type = h264_type(info, &header)?;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; every COM object is used
    // on this thread only and released when it goes out of scope.
    unsafe {
        let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, None)
            .map_err(|e| sys("MFCreateSinkWriterFromURL", e))?;
        let stream = writer
            .AddStream(&media_type)
            .map_err(|e| sys("AddStream", e))?;
        // Same type in and out: the sink writer passes the encoded samples through.
        writer
            .SetInputMediaType(stream, &media_type, None)
            .map_err(|e| sys("SetInputMediaType", e))?;

        // Audio tracks: (stream index, track index).
        let mut streams: Vec<(u32, usize)> = vec![(stream, 0)];
        for (i, a) in audio.iter().enumerate() {
            let track = i + 1;
            if snapshot.tracks.get(track).is_none_or(|t| t.is_empty()) {
                continue;
            }
            let t = aac_type(a)?;
            let s = writer
                .AddStream(&t)
                .map_err(|e| sys("AddStream(audio)", e))?;
            writer
                .SetInputMediaType(s, &t, None)
                .map_err(|e| sys("SetInputMediaType(audio)", e))?;
            streams.push((s, track));
        }
        writer.BeginWriting().map_err(|e| sys("BeginWriting", e))?;

        // Interleave all tracks by time so the sink does not have to buffer a whole track.
        let mut order: Vec<(i64, u32, &hc_buffer::Packet)> = Vec::new();
        for (s, track) in &streams {
            for p in &snapshot.tracks[*track] {
                // Audio that ends before the first video frame would only add a silent lead-in.
                if *track != 0 && p.pts < snapshot.start_pts {
                    continue;
                }
                order.push((p.pts, *s, p));
            }
        }
        order.sort_by_key(|(pts, _, _)| *pts);

        for (_, stream, packet) in order {
            let buffer =
                MFCreateMemoryBuffer(packet.data.len() as u32).map_err(|e| sys("buffer", e))?;
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|e| sys("lock", e))?;
            std::ptr::copy_nonoverlapping(packet.data.as_ptr(), data, packet.data.len());
            buffer.Unlock().map_err(|e| sys("unlock", e))?;
            buffer
                .SetCurrentLength(packet.data.len() as u32)
                .map_err(|e| sys("length", e))?;

            let sample = MFCreateSample().map_err(|e| sys("sample", e))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| sys("add buffer", e))?;
            sample
                .SetSampleTime((packet.pts - snapshot.start_pts).max(0))
                .map_err(|e| sys("time", e))?;
            sample
                .SetSampleDuration(packet.duration)
                .map_err(|e| sys("duration", e))?;
            if packet.keyframe {
                sample
                    .SetUINT32(&MFSampleExtension_CleanPoint, 1)
                    .map_err(|e| sys("clean point", e))?;
            }
            writer
                .WriteSample(stream, &sample)
                .map_err(|e| sys("WriteSample", e))?;
        }
        writer.Finalize().map_err(|e| sys("Finalize", e))?;
    }
    Ok(())
}
