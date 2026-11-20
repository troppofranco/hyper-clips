//! Codec-agnostic media types shared by capture, encoders, buffer and muxer.
//!
//! All timestamps are in 100 ns units ([`TIMEBASE`]) on a single monotonic
//! clock (QPC) shared by audio and video.

use std::sync::Arc;

/// Ticks per second of every `pts`/`dur` value.
pub const TIMEBASE: i64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Parameter sets without start codes: H.264 = [SPS, PPS], HEVC = [VPS, SPS, PPS].
    pub param_sets: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRole {
    /// System + microphone, the track players pick by default.
    Mix,
    System,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    pub role: AudioRole,
    pub sample_rate: u32,
    pub channels: u16,
    /// AAC AudioSpecificConfig.
    pub asc: Vec<u8>,
    pub bitrate: u32,
}

/// One encoded access unit. Video payloads use 4-byte length-prefixed NAL
/// units (MP4 style); audio payloads are raw AAC frames.
#[derive(Debug, Clone)]
pub struct Packet {
    /// 0 = video, 1.. = audio tracks in `StreamSet::audio` order.
    pub track: u8,
    pub pts: i64,
    pub dur: i64,
    pub key: bool,
    pub data: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSet {
    pub video: VideoInfo,
    pub audio: Vec<AudioInfo>,
}

/// Current QPC time in [`TIMEBASE`] units.
pub fn now_ticks() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    // SAFETY: plain out-parameters; both calls cannot fail on XP+.
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0;
    }
    ((i128::from(c) * i128::from(TIMEBASE)) / i128::from(f)) as i64
}

/// Convert a QPC counter value (as returned by DXGI/WASAPI) to ticks.
pub fn qpc_to_ticks(qpc: i64) -> i64 {
    use windows::Win32::System::Performance::QueryPerformanceFrequency;
    let mut f = 0i64;
    // SAFETY: plain out-parameter.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0;
    }
    ((i128::from(qpc) * i128::from(TIMEBASE)) / i128::from(f)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic() {
        let a = now_ticks();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now_ticks();
        assert!(b - a >= 40_000, "{a} {b}");
    }
}
