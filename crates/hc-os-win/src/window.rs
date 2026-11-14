//! Finding windows by title.

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};

struct Search {
    needle: String,
    found: Option<isize>,
}

unsafe extern "system" fn check(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the address of the `Search` owned by `find_by_title` for the whole call.
    let search = unsafe { &mut *(data.0 as *mut Search) };
    // SAFETY: plain queries on a window handle handed out by EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
            if title.contains(&search.needle) {
                search.found = Some(hwnd.0 as isize);
                return BOOL(0); // stop
            }
        }
    }
    BOOL(1)
}

/// First visible top-level window whose title contains `needle` (case-insensitive).
pub fn find_by_title(needle: &str) -> Option<isize> {
    if needle.trim().is_empty() {
        return None;
    }
    let mut search = Search {
        needle: needle.trim().to_lowercase(),
        found: None,
    };
    // SAFETY: the callback only touches `search`, which outlives the call. EnumWindows reports
    // an error when the callback stops the enumeration early; that is expected and ignored.
    let _ = unsafe { EnumWindows(Some(check), LPARAM(&mut search as *mut _ as isize)) };
    search.found
}
