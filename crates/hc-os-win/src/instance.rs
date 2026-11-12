//! Single-instance guard (per Windows session).

use hc_core::error::{Error, Result};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Holds the named mutex for as long as it lives.
pub struct InstanceGuard(HANDLE);

// SAFETY: the handle is only closed on drop and never used for anything else.
unsafe impl Send for InstanceGuard {}

pub fn acquire() -> Result<InstanceGuard> {
    // SAFETY: plain FFI call with a static, NUL-terminated name.
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\HyperClips-Service")) }
        .map_err(|e| Error::System(e.to_string()))?;
    // SAFETY: GetLastError right after CreateMutexW reports whether it already existed.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: we own `handle`.
        let _ = unsafe { CloseHandle(handle) };
        return Err(Error::AlreadyRunning);
    }
    Ok(InstanceGuard(handle))
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
