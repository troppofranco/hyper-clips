//! Editor model and export engine.
//!
//! Export runs entirely on local hardware: Media Foundation decodes the
//! sources (DXVA), our D3D11 compositor applies crop/zoom, transitions and
//! overlays, and the same hardware encoders as capture produce the output.

pub mod anim;
pub mod decode;
pub mod project;
pub mod queue;
pub mod render;
pub mod srt;
pub mod text;
pub mod webm;

pub use project::*;
