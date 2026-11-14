//! Which program is in the foreground, and is it covering a whole monitor?

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApp {
    /// File name such as `game.exe`.
    pub exe_name: String,
    /// True when the window covers its whole monitor (exclusive or borderless fullscreen).
    pub fullscreen: bool,
}

/// None when there is no foreground window or its process cannot be inspected.
pub fn foreground_app() -> Option<ForegroundApp> {
    // SAFETY: plain queries; every out-pointer is valid for the call and the process handle
    // is closed before returning.
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let queried = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        queried.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let exe_name = full.rsplit('\\').next()?.to_owned();

        let mut window = RECT::default();
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let fullscreen = GetWindowRect(hwnd, &mut window).is_ok()
            && GetMonitorInfoW(monitor, &mut info).as_bool()
            && window.left <= info.rcMonitor.left
            && window.top <= info.rcMonitor.top
            && window.right >= info.rcMonitor.right
            && window.bottom >= info.rcMonitor.bottom;
        Some(ForegroundApp {
            exe_name,
            fullscreen,
        })
    }
}
