//! Encoders and muxing for Hyper Clips.
//!
//! Video: Media Foundation encoder MFTs (vendor hardware first, Microsoft
//! software as fallback) fed with D3D11 NV12 textures. Audio: the Windows
//! AAC encoder MFT. Container: our own MP4 writer (no third-party codecs).

pub mod aac;
pub mod bitstream;
pub mod mf;
pub mod mp4;
pub mod probe;
pub mod video;
