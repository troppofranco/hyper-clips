//! WASAPI capture of the system mix (loopback) or the default microphone.

pub mod timeline;

#[cfg(windows)]
mod wasapi;
#[cfg(windows)]
pub use wasapi::{AudioCapture, Source};
