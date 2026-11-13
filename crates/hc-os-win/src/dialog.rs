//! Minimal error dialog for failures before logging exists (the apps have no console).

use crate::wide;
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDNO, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    MB_YESNOCANCEL,
};

pub fn show_error(title: &str, text: &str) {
    let title = wide::str(title);
    let text = wide::str(text);
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
    Cancel,
}

/// Informational message box.
pub fn info(title: &str, text: &str) {
    let title = wide::str(title);
    let text = wide::str(text);
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

/// Yes / No / Cancel question. Blocks until answered.
pub fn ask_yes_no_cancel(title: &str, text: &str) -> Choice {
    let title = wide::str(title);
    let text = wide::str(text);
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNOCANCEL | MB_ICONINFORMATION | MB_SETFOREGROUND,
        )
    };
    if answer == IDYES {
        Choice::Yes
    } else if answer == IDNO {
        Choice::No
    } else {
        Choice::Cancel
    }
}
