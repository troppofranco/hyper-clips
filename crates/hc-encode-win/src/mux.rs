//! Writes already-encoded packets to an MP4 file without re-encoding.

use crate::aac::aac_type;
use crate::mf::{h264_type, sys};
use crate::{annexb, AudioInfo, VideoInfo};
use hc_buffer::{Packet, Snapshot};
use hc_core::error::{Error, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;

/// Streaming MP4 writer: packets are written as they arrive (full-session recording) or
/// replayed from a snapshot (clips). Track 0 is H.264 video, tracks 1.. are AAC audio.
pub struct Mp4Writer {
    writer: IMFSinkWriter,
    /// Sink stream index per track (None = track not in this file).
    streams: Vec<Option<u32>>,
    finished: bool,
}

// SAFETY: the Media Foundation sink writer is a free-threaded (MTA) COM object; callers
// serialise access with a mutex, so it is never used by two threads at once.
unsafe impl Send for Mp4Writer {}

impl Mp4Writer {
    /// `sequence_header` is the Annex-B SPS+PPS (see [`annexb::sequence_header`]).
    /// `audio` lists `(track, format)` pairs for the audio tracks to include.
    pub fn create(
        path: &Path,
        info: &VideoInfo,
        sequence_header: &[u8],
        audio: &[(usize, AudioInfo)],
    ) -> Result<Self> {
        crate::mf::init()?;
        let media_type = h264_type(info, sequence_header)?;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; COM objects are released
        // when they go out of scope.
        unsafe {
            let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, None)
                .map_err(|e| sys("MFCreateSinkWriterFromURL", e))?;
            let mut streams: Vec<Option<u32>> = vec![None];
            let video = writer
                .AddStream(&media_type)
                .map_err(|e| sys("AddStream", e))?;
            // Same type in and out: the sink writer passes the encoded samples through.
            writer
                .SetInputMediaType(video, &media_type, None)
                .map_err(|e| sys("SetInputMediaType", e))?;
            streams[0] = Some(video);
            for (track, a) in audio {
                let t = aac_type(a)?;
                let s = writer
                    .AddStream(&t)
                    .map_err(|e| sys("AddStream(audio)", e))?;
                writer
                    .SetInputMediaType(s, &t, None)
                    .map_err(|e| sys("SetInputMediaType(audio)", e))?;
                if streams.len() <= *track {
                    streams.resize(*track + 1, None);
                }
                streams[*track] = Some(s);
            }
            writer.BeginWriting().map_err(|e| sys("BeginWriting", e))?;
            Ok(Self {
                writer,
                streams,
                finished: false,
            })
        }
    }

    /// Writes one packet; `time` is its timestamp in the file (100 ns ticks from zero).
    pub fn write(&mut self, track: usize, packet: &Packet, time: i64) -> Result<()> {
        let Some(stream) = self.streams.get(track).copied().flatten() else {
            return Ok(()); // track not part of this file
        };
        // SAFETY: the buffer is created, filled within its bounds and handed to the sink.
        unsafe {
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
                .SetSampleTime(time.max(0))
                .map_err(|e| sys("time", e))?;
            sample
                .SetSampleDuration(packet.duration)
                .map_err(|e| sys("duration", e))?;
            if packet.keyframe {
                sample
                    .SetUINT32(&MFSampleExtension_CleanPoint, 1)
                    .map_err(|e| sys("clean point", e))?;
            }
            self.writer
                .WriteSample(stream, &sample)
                .map_err(|e| sys("WriteSample", e))?;
        }
        Ok(())
    }

    /// Flushes and closes the file.
    pub fn finish(mut self) -> Result<()> {
        self.finished = true;
        // SAFETY: plain COM call on a writer that has begun writing.
        unsafe { self.writer.Finalize() }.map_err(|e| sys("Finalize", e))
    }
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        if !self.finished {
            // Best effort so an aborted recording still leaves a playable file.
            // SAFETY: as in `finish`.
            let _ = unsafe { self.writer.Finalize() };
        }
    }
}

/// Saves `snapshot` as an MP4 at `path`: track 0 is H.264 video, tracks 1.. are AAC audio
/// described by `audio` (same order; empty tracks are skipped). Timestamps are shifted so the
/// file starts at zero.
pub fn write_mp4(
    path: &Path,
    info: &VideoInfo,
    audio: &[AudioInfo],
    snapshot: &Snapshot,
) -> Result<()> {
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

    let used: Vec<(usize, AudioInfo)> = audio
        .iter()
        .enumerate()
        .filter(|(i, _)| snapshot.tracks.get(i + 1).is_some_and(|t| !t.is_empty()))
        .map(|(i, a)| (i + 1, *a))
        .collect();
    let mut writer = Mp4Writer::create(path, info, &header, &used)?;

    // Interleave all tracks by time so the sink does not have to buffer a whole track.
    let mut order: Vec<(i64, usize, &Packet)> = Vec::new();
    for (track, packets) in snapshot.tracks.iter().enumerate() {
        for p in packets {
            // Audio that ends before the first video frame would only add a silent lead-in.
            if track != 0 && p.pts < snapshot.start_pts {
                continue;
            }
            order.push((p.pts, track, p));
        }
    }
    order.sort_by_key(|(pts, _, _)| *pts);
    for (pts, track, packet) in order {
        writer.write(track, packet, pts - snapshot.start_pts)?;
    }
    writer.finish()
}
