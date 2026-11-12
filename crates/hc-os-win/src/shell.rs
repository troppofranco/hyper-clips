//! Opening folders/files with the default handler.

use crate::wide;
use hc_core::error::{Error, Result};
use hc_core::platform::Shell;
use std::path::Path;
use windows::core::{w, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub struct WinShell;

impl Shell for WinShell {
    fn open_path(&self, path: &Path) -> Result<()> {
        let target = wide::os(path.as_os_str());
        // SAFETY: `target` is NUL-terminated and outlives the call; other pointers are null.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute reports success with a value greater than 32.
        if (result.0 as isize) > 32 {
            Ok(())
        } else {
            Err(Error::System(format!("cannot open {}", path.display())))
        }
    }
}
