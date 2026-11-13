//! Local date/time without pulling in a date crate.

use windows::Win32::System::SystemInformation::GetLocalTime;

/// `YYYY-MM-DD_HH-MM-SS` in local time, safe for file names.
pub fn local_timestamp() -> String {
    // SAFETY: plain FFI returning a value.
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}
