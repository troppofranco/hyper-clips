//! System UI locale, e.g. `it-IT`.

use windows::Win32::Globalization::GetUserDefaultLocaleName;

pub fn system_locale() -> String {
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
                              // SAFETY: `buf` is a valid mutable slice.
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if len > 1 {
        String::from_utf16_lossy(&buf[..(len as usize - 1)])
    } else {
        "en".to_owned()
    }
}
