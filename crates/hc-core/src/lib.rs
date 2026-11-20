//! Shared building blocks for every Hyper Clips process.
//!
//! This crate must stay lightweight: it is linked into the always-running
//! core process, so no GUI, media or network dependencies belong here.

pub mod backup;
pub mod brand;
pub mod config;
pub mod i18n;
pub mod ipc;
pub mod logging;
pub mod media;
pub mod paths;
pub mod sys;

/// Application version, taken from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Product name shown to users.
pub const APP_NAME: &str = "Hyper Clips";

/// Executable file names, used to locate sibling binaries.
pub const CORE_EXE: &str = "hyperclips.exe";
pub const STUDIO_EXE: &str = "hyperclips-studio.exe";
pub const UPDATER_HELPER_EXE: &str = "hc-updater-helper.exe";
