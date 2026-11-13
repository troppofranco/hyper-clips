//! Minimal audible feedback (the real overlay/notification comes later).

use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONHAND, MB_OK};

pub fn beep_ok() {
    // SAFETY: plain FFI, no pointers.
    let _ = unsafe { MessageBeep(MB_OK) };
}

pub fn beep_error() {
    // SAFETY: plain FFI, no pointers.
    let _ = unsafe { MessageBeep(MB_ICONHAND) };
}
