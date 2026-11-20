//! Minimal in-game notification ("Clip saved", "Recording…").
//!
//! A topmost, click-through, non-activating layered window. It is excluded
//! from every capture API (`WDA_EXCLUDEFROMCAPTURE`), so it never appears in
//! clips. It shows over borderless/windowed games; true exclusive-fullscreen
//! games bypass the desktop compositor, so the tray toast remains the
//! fallback there.

use std::cell::RefCell;

use hc_core::config::OverlayPosition;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DT_CALCRECT,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FW_SEMIBOLD, FillRect,
    GetDC, GetMonitorInfoW, HFONT, InvalidateRect, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, HWND_TOPMOST, KillTimer, LWA_ALPHA, RegisterClassExW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetLayeredWindowAttributes, SetTimer,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_PAINT, WM_TIMER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{HSTRING, w};

const HIDE_TIMER: usize = 1;
const SHOW_MS: u32 = 2500;
const PAD_X: i32 = 18;
const PAD_Y: i32 = 12;
const ACCENT_W: i32 = 5;

thread_local! {
    static TEXT: RefCell<(Vec<u16>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub struct Overlay {
    hwnd: HWND,
    font: HFONT,
}

impl Overlay {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: window class/window creation on the UI thread.
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class = w!("HyperClipsOverlay");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                class,
                w!("Hyper Clips overlay"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(hinstance.into()),
                None,
            )?;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
            // Never visible in our own (or anyone's) captures.
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            let font = CreateFontW(
                -20,
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                Default::default(),
                Default::default(),
                Default::default(),
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            Ok(Self { hwnd, font })
        }
    }

    /// Show `text` for a few seconds near the given corner of the monitor
    /// containing `anchor` (the game window centre).
    pub fn show(&self, text: &str, position: OverlayPosition, anchor: (i32, i32), recording: bool) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return;
        }
        TEXT.with(|t| *t.borrow_mut() = (wide.clone(), recording));
        // SAFETY: GDI measurement + window placement on our own window.
        unsafe {
            let dc = GetDC(Some(self.hwnd));
            let old = SelectObject(dc, self.font.into());
            let mut rc = RECT::default();
            let mut buf = wide.clone();
            DrawTextW(dc, &mut buf, &mut rc, DT_CALCRECT | DT_SINGLELINE);
            SelectObject(dc, old);
            ReleaseDC(Some(self.hwnd), dc);
            let w = rc.right - rc.left + 2 * PAD_X + ACCENT_W;
            let h = rc.bottom - rc.top + 2 * PAD_Y;

            let monitor = MonitorFromPoint(
                windows::Win32::Foundation::POINT {
                    x: anchor.0,
                    y: anchor.1,
                },
                MONITOR_DEFAULTTOPRIMARY,
            );
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let m = mi.rcMonitor;
            let margin = 24;
            let (x, y) = match position {
                OverlayPosition::TopLeft => (m.left + margin, m.top + margin),
                OverlayPosition::TopRight => (m.right - w - margin, m.top + margin),
                OverlayPosition::BottomLeft => (m.left + margin, m.bottom - h - margin),
                OverlayPosition::BottomRight => (m.right - w - margin, m.bottom - h - margin),
            };
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
            let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, 12, 12);
            SetWindowRgn(self.hwnd, Some(rgn), true);
            let _ = InvalidateRect(Some(self.hwnd), None, true);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(self.hwnd), HIDE_TIMER, SHOW_MS, None);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: font created in new(); the window dies with the thread/process.
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == HIDE_TIMER => {
            // SAFETY: our own window/timer.
            unsafe {
                let _ = KillTimer(Some(hwnd), HIDE_TIMER);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // SAFETY: standard paint cycle on our own window.
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
                let bg = CreateSolidBrush(COLORREF(0x0020_1A18));
                FillRect(dc, &rc, bg);
                let _ = DeleteObject(bg.into());
                let (text, recording) = TEXT.with(|t| t.borrow().clone());
                // Accent bar: red while recording, violet otherwise (BGR).
                let accent =
                    CreateSolidBrush(COLORREF(if recording { 0x0044_44EF } else { 0x00ED_3A7C }));
                let bar = RECT {
                    right: ACCENT_W,
                    ..rc
                };
                FillRect(dc, &bar, accent);
                let _ = DeleteObject(accent.into());
                let font = CreateFontW(
                    -20,
                    0,
                    0,
                    0,
                    FW_SEMIBOLD.0 as i32,
                    0,
                    0,
                    0,
                    Default::default(),
                    Default::default(),
                    Default::default(),
                    CLEARTYPE_QUALITY,
                    0,
                    &HSTRING::from("Segoe UI"),
                );
                let old = SelectObject(dc, font.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00FF_FFFF));
                let mut trc = RECT {
                    left: ACCENT_W + PAD_X,
                    ..rc
                };
                // Windows may paint before the first notification: an empty
                // slice would hand DrawTextW a dangling pointer.
                let mut buf = text;
                if !buf.is_empty() {
                    DrawTextW(dc, &mut buf, &mut trc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                }
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
