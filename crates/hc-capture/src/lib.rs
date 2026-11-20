//! Capture for Hyper Clips.
//!
//! - [`desktop`]: DXGI Desktop Duplication of a monitor (no yellow border on
//!   Windows 10, works with borderless and most fullscreen games, no
//!   injection into game processes).
//! - [`convert`]: GPU BGRA → NV12 conversion and scaling (D3D11 video processor).
//! - [`screenshot`]: PNG/JPEG encoding of a captured frame via WIC.
//! - [`audio`]: WASAPI system loopback and microphone capture plus an
//!   aligning mixer.

pub mod audio;
pub mod compose;
pub mod convert;
pub mod cursor;
pub mod desktop;
pub mod dsp;
pub mod screenshot;
pub mod webcam;
pub mod window;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Windows error: {0}")]
    Os(#[from] windows::core::Error),
    #[error("no monitor matches the selection")]
    NoOutput,
    #[error("desktop duplication lost access (mode change, secure desktop, ...)")]
    AccessLost,
    #[error("{0}")]
    Other(String),
}
