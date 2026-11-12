//! Encoding and muxing. `convert` and `annexb` are portable; the Media
//! Foundation parts only exist on Windows.

pub mod annexb;
pub mod convert;

#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
pub mod mf;
#[cfg(windows)]
pub mod mux;

/// Parameters of the encoded video stream (needed to write an MP4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// Annex-B SPS+PPS; may be empty, then it is taken from the first keyframe.
    pub sequence_header: Vec<u8>,
}
