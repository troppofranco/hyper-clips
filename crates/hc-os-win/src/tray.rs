//! Tray icon, context menu, global hotkeys and the message loop.
//!
//! Everything runs on the thread that calls [`run`]. A normal (never shown)
//! top-level window is used instead of a message-only window because message-only
//! windows do not receive the `TaskbarCreated` broadcast needed to restore the
//! icon after an Explorer restart.

use crate::wide;
use hc_core::error::{Error, Result};
use hc_core::hotkey::{Hotkey, HotkeyAction};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage,
    IDI_APPLICATION, MF_STRING, MSG, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
    WNDCLASSW,
};

const WM_TRAY: u32 = WM_APP + 1;
const MOD_NOREPEAT: u32 = 0x4000;
const ID_OPEN: usize = 1;
const ID_SAVE: usize = 2;
const ID_TOGGLE: usize = 3;
const ID_QUIT: usize = 4;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Open,
    SaveClip,
    ToggleRecording,
    Hotkey(HotkeyAction),
}

#[derive(Debug, Clone)]
pub struct TrayLabels {
    pub tooltip: String,
    pub open: String,
    pub save_clip: String,
    pub toggle_recording: String,
    pub quit: String,
}

struct State {
    on_event: Box<dyn FnMut(TrayEvent)>,
    labels: TrayLabels,
    hotkey_ids: Vec<(i32, HotkeyAction)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn sys<E: std::fmt::Display>(e: E) -> Error {
    Error::System(e.to_string())
}

/// Delivers an event without holding the state borrow across foreign code.
fn emit(event: TrayEvent) {
    STATE.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            if let Some(state) = guard.as_mut() {
                (state.on_event)(event);
            }
        }
    });
}

/// Asks the tray window (identified by the value passed to `on_ready`) to close;
/// this ends [`run`]. Safe to call from any thread.
pub fn request_quit(window: isize) {
    // SAFETY: posting a message to a window handle is harmless even if it is stale.
    let _ = unsafe {
        PostMessageW(
            Some(HWND(window as *mut c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        )
    };
}

/// Creates the tray icon and registers the hotkeys, then runs the message loop
/// until the user quits. `on_ready` receives the window handle once everything is set up.
pub fn run(
    labels: TrayLabels,
    hotkeys: &[(HotkeyAction, Hotkey)],
    on_event: impl FnMut(TrayEvent) + 'static,
    on_ready: impl FnOnce(isize),
) -> Result<()> {
    // SAFETY: standard Win32 window-class/window creation on the current thread; every
    // pointer passed is valid for the duration of its call.
    unsafe {
        let module = GetModuleHandleW(None).map_err(sys)?;
        let instance: HINSTANCE = module.into();
        let class = w!("HyperClipsTrayWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(sys("RegisterClassW failed"));
        }
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );

        let tooltip = labels.tooltip.clone();
        STATE.with(|cell| {
            *cell.borrow_mut() = Some(State {
                on_event: Box::new(on_event),
                labels,
                hotkey_ids: Vec::new(),
            });
        });

        // Never shown: no WS_VISIBLE and no ShowWindow call.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("Hyper Clips"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(sys)?;

        add_icon(hwnd, &tooltip);

        for (index, (action, hotkey)) in hotkeys.iter().enumerate() {
            let id = index as i32 + 1;
            let modifiers = HOT_KEY_MODIFIERS(hotkey.modifiers | MOD_NOREPEAT);
            match RegisterHotKey(Some(hwnd), id, modifiers, hotkey.vk) {
                Ok(()) => STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.hotkey_ids.push((id, *action));
                    }
                }),
                // Typically the combination is already taken by another program.
                Err(e) => tracing::warn!("cannot register hotkey {hotkey} ({action:?}): {e}"),
            }
        }

        on_ready(hwnd.0 as isize);

        let mut msg = MSG::default();
        loop {
            // 0 = WM_QUIT, -1 = error: stop in both cases.
            if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe fn add_icon(hwnd: HWND, tooltip: &str) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        // Phase 1 uses the stock application icon; a real .ico resource comes later.
        hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        ..Default::default()
    };
    // Leave room for the terminating NUL (szTip has 128 slots).
    for (dst, src) in nid.szTip.iter_mut().zip(tooltip.encode_utf16().take(127)) {
        *dst = src;
    }
    if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
        tracing::warn!("cannot add tray icon");
    }
}

unsafe fn remove_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn show_menu(hwnd: HWND) {
    let labels = STATE.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|s| s.labels.clone()))
    });
    let Some(labels) = labels else {
        return;
    };
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    for (id, text) in [
        (ID_OPEN, &labels.open),
        (ID_SAVE, &labels.save_clip),
        (ID_TOGGLE, &labels.toggle_recording),
        (ID_QUIT, &labels.quit),
    ] {
        let text = wide::str(text);
        let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()));
    }

    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    // Required so the menu closes when the user clicks elsewhere.
    let _ = SetForegroundWindow(hwnd);
    // The state borrow is not held here: TrackPopupMenu pumps messages (incl. WM_HOTKEY).
    let command = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        point.x,
        point.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);

    match command.0 as usize {
        ID_OPEN => emit(TrayEvent::Open),
        ID_SAVE => emit(TrayEvent::SaveClip),
        ID_TOGGLE => emit(TrayEvent::ToggleRecording),
        ID_QUIT => {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            let id = wparam.0 as i32;
            let action = STATE.with(|cell| {
                cell.try_borrow().ok().and_then(|guard| {
                    guard
                        .as_ref()
                        .and_then(|s| s.hotkey_ids.iter().find(|(i, _)| *i == id).map(|(_, a)| *a))
                })
            });
            if let Some(action) = action {
                emit(TrayEvent::Hotkey(action));
            }
            LRESULT(0)
        }
        WM_TRAY => {
            // With the legacy callback behaviour the low word of lparam is the mouse message.
            match (lparam.0 as u32) & 0xFFFF {
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                WM_LBUTTONDBLCLK => emit(TrayEvent::Open),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_icon(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted: the icon is gone, add it again.
            let tooltip = STATE.with(|cell| {
                cell.try_borrow()
                    .ok()
                    .and_then(|guard| guard.as_ref().map(|s| s.labels.tooltip.clone()))
            });
            if let Some(tooltip) = tooltip {
                add_icon(hwnd, &tooltip);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
