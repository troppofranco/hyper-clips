//! Windows-only implementations. UNVERIFIED on a real Windows toolchain in Phase 1:
//! written against `windows` 0.61, compile it first (see STATO.md).

#![cfg(windows)]

pub mod dialog;
pub mod instance;
pub mod locale;
pub mod memory;
pub mod pipe;
pub mod shell;
pub mod tray;

pub(crate) mod wide {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    /// NUL-terminated UTF-16 copy of `s`.
    pub fn os(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn str(s: &str) -> Vec<u16> {
        os(OsStr::new(s))
    }
}
