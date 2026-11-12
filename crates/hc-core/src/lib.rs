//! Platform-independent core of Hyper Clips.
//!
//! Nothing in this crate touches Win32. OS-specific code lives behind the
//! traits in [`platform`] and is implemented in `hc-os-win`.

pub mod config;
pub mod consts;
pub mod error;
pub mod hotkey;
pub mod i18n;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod platform;

pub use error::{Error, Result};
