//! Hidden message window + notification-area icon, in plain Win32.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIIF_WARNING,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DestroyIcon, DestroyMenu,
    GetCursorPos, GetSystemMetrics, HICON, ICONINFO, MF_SEPARATOR, MF_STRING, PostMessageW,
    RegisterClassExW, SM_CXSMICON, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WM_APP, WM_NULL, WNDCLASSEXW, WNDPROC,
    WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_IPC: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCmd {
    Open = 1,
    SaveClip = 2,
    ToggleRecording = 3,
    Screenshot = 4,
    Quit = 9,
}

impl MenuCmd {
    fn from_id(id: i32) -> Option<Self> {
        [
            Self::Open,
            Self::SaveClip,
            Self::ToggleRecording,
            Self::Screenshot,
            Self::Quit,
        ]
        .into_iter()
        .find(|c| *c as i32 == id)
    }
}

pub fn create_window(wndproc: WNDPROC) -> windows::core::Result<HWND> {
    // SAFETY: standard window-class registration and creation on this thread.
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class = w!("HyperClipsCore");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: wndproc,
            hInstance: hinstance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        // A real (never shown) top-level window: message-only windows do not
        // receive the TaskbarCreated broadcast needed after Explorer restarts.
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            class,
            w!("Hyper Clips Core"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    added: bool,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            icon: make_icon().unwrap_or_default(),
            added: false,
        }
    }

    fn base(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        }
    }

    pub fn add(&mut self, tooltip: &str) -> bool {
        let mut nid = self.base();
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.icon;
        copy_wide(&mut nid.szTip, tooltip);
        // SAFETY: fully initialised NOTIFYICONDATAW owned by this frame.
        unsafe {
            self.added = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        self.added
    }

    pub fn set_tooltip(&self, tooltip: &str) {
        let mut nid = self.base();
        nid.uFlags = NIF_TIP | NIF_SHOWTIP;
        copy_wide(&mut nid.szTip, tooltip);
        // SAFETY: see `add`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Balloon / toast notification.
    pub fn notify(&self, title: &str, text: &str, warning: bool, sound: bool) {
        let mut nid = self.base();
        nid.uFlags = NIF_INFO;
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, text);
        nid.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        if !sound {
            nid.dwInfoFlags |= NIIF_NOSOUND;
        }
        // SAFETY: see `add`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    pub fn remove(&mut self) {
        if self.added {
            let nid = self.base();
            // SAFETY: see `add`.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
    }

    /// Show the context menu at the cursor and return the chosen command.
    /// Must be called without holding app state borrowed: the menu runs a
    /// nested message loop.
    pub fn show_menu(hwnd: HWND, items: &[(MenuCmd, String)]) -> Option<MenuCmd> {
        // SAFETY: menu is created, used and destroyed within this call.
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for (cmd, label) in items {
                if *cmd == MenuCmd::Quit {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                let text = HSTRING::from(label.as_str());
                let _ = AppendMenuW(menu, MF_STRING, *cmd as usize, &text);
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Required so the menu closes when clicking elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let id = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            );
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            MenuCmd::from_id(id.0)
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if !self.icon.is_invalid() {
            // SAFETY: icon created by CreateIconIndirect and owned by us.
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn copy_wide<const N: usize>(dst: &mut [u16; N], src: &str) {
    let mut i = 0;
    for unit in src.encode_utf16() {
        if i + 1 >= N {
            break;
        }
        dst[i] = unit;
        i += 1;
    }
    dst[i] = 0;
}

fn make_icon() -> windows::core::Result<HICON> {
    // SAFETY: plain GDI object creation; bitmaps are freed after the icon copies them.
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
        let rgba = hc_core::brand::icon_rgba(size as u32);
        // GDI wants BGRA.
        let bgra: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| [b, g, r, a])
            .collect();
        let color = CreateBitmap(size, size, 1, 32, Some(bgra.as_ptr().cast()));
        let mask_bits = vec![0u8; ((size as usize).div_ceil(16) * 2) * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
