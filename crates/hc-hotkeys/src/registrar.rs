use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEMOUSE, RegisterRawInputDevices,
};

use crate::parse::{Hotkey, Key, Modifiers, MouseButton};

// RAWMOUSE.usButtonFlags bits (winuser.h).
const RI_MOUSE_MIDDLE_BUTTON_DOWN: u16 = 0x0010;
const RI_MOUSE_BUTTON_4_DOWN: u16 = 0x0040;
const RI_MOUSE_BUTTON_5_DOWN: u16 = 0x0100;

/// A binding that could not be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub action: String,
    pub binding: String,
    pub reason: String,
}

/// Owns the hotkey registrations of one window (must be used on its thread).
pub struct Registrar {
    hwnd: HWND,
    keyboard: Vec<(i32, String)>,
    mouse: Vec<(Hotkey, String)>,
    raw_input_on: bool,
}

impl Registrar {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            keyboard: Vec::new(),
            mouse: Vec::new(),
            raw_input_on: false,
        }
    }

    /// Replace all registrations with `bindings` (action id, binding string).
    /// Returns the bindings that could not be registered.
    pub fn apply(&mut self, bindings: &[(&str, &str)]) -> Vec<Conflict> {
        self.clear();
        let mut conflicts = Vec::new();
        let mut seen: Vec<(Hotkey, &str)> = Vec::new();
        for (idx, (action, binding)) in bindings.iter().enumerate() {
            let conflict = |reason: String| Conflict {
                action: (*action).to_owned(),
                binding: (*binding).to_owned(),
                reason,
            };
            let hk = match Hotkey::parse_optional(binding) {
                Ok(Some(h)) => h,
                Ok(None) => continue,
                Err(e) => {
                    conflicts.push(conflict(e.to_string()));
                    continue;
                }
            };
            if let Some((_, other)) = seen.iter().find(|(h, _)| *h == hk) {
                conflicts.push(conflict(format!("duplicate of {other}")));
                continue;
            }
            seen.push((hk, action));
            match hk.key {
                Key::Mouse(_) => self.mouse.push((hk, (*action).to_owned())),
                Key::Vk(vk) => {
                    let id = i32::try_from(idx).unwrap_or(0) + 1;
                    // SAFETY: hwnd belongs to the calling thread.
                    match unsafe {
                        RegisterHotKey(Some(self.hwnd), id, to_win_mods(hk.mods), u32::from(vk))
                    } {
                        Ok(()) => self.keyboard.push((id, (*action).to_owned())),
                        Err(e) => {
                            tracing::warn!(action, binding, error = %e, "hotkey registration failed");
                            conflicts.push(conflict(
                                "already in use by another application or by Windows".into(),
                            ));
                        }
                    }
                }
            }
        }
        if !self.mouse.is_empty() {
            match self.set_raw_mouse(true) {
                Ok(()) => self.raw_input_on = true,
                Err(e) => {
                    for (hk, action) in self.mouse.drain(..) {
                        conflicts.push(Conflict {
                            action,
                            binding: hk.to_string(),
                            reason: format!("raw input unavailable: {e}"),
                        });
                    }
                }
            }
        }
        conflicts
    }

    pub fn clear(&mut self) {
        for (id, _) in self.keyboard.drain(..) {
            // SAFETY: id was registered on this window.
            let _ = unsafe { UnregisterHotKey(Some(self.hwnd), id) };
        }
        self.mouse.clear();
        if self.raw_input_on {
            let _ = self.set_raw_mouse(false);
            self.raw_input_on = false;
        }
    }

    /// Map a `WM_HOTKEY` to its action id.
    pub fn on_hotkey(&self, wparam: WPARAM) -> Option<&str> {
        let id = i32::try_from(wparam.0).ok()?;
        self.keyboard
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, a)| a.as_str())
    }

    /// Map a `WM_INPUT` to an action id if a bound mouse button went down.
    pub fn on_raw_input(&self, lparam: LPARAM) -> Option<&str> {
        if self.mouse.is_empty() {
            return None;
        }
        let mut raw = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        // SAFETY: buffer is a full RAWINPUT; mouse packets fit in it.
        let n = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam.0 as *mut _),
                RID_INPUT,
                Some((&mut raw as *mut RAWINPUT).cast()),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if n == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
            return None;
        }
        // SAFETY: dwType says the union holds mouse data.
        let flags = unsafe { raw.data.mouse.Anonymous.Anonymous.usButtonFlags };
        let button = if flags & RI_MOUSE_BUTTON_4_DOWN != 0 {
            MouseButton::X1
        } else if flags & RI_MOUSE_BUTTON_5_DOWN != 0 {
            MouseButton::X2
        } else if flags & RI_MOUSE_MIDDLE_BUTTON_DOWN != 0 {
            MouseButton::Middle
        } else {
            return None;
        };
        let mods = current_modifiers();
        self.mouse
            .iter()
            .find(|(hk, _)| hk.key == Key::Mouse(button) && hk.mods == mods)
            .map(|(_, a)| a.as_str())
    }

    fn set_raw_mouse(&self, on: bool) -> windows::core::Result<()> {
        let dev = RAWINPUTDEVICE {
            usUsagePage: 0x01, // generic desktop
            usUsage: 0x02,     // mouse
            dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
            hwndTarget: if on { self.hwnd } else { HWND::default() },
        };
        // SAFETY: single, fully-initialised device descriptor.
        unsafe { RegisterRawInputDevices(&[dev], size_of::<RAWINPUTDEVICE>() as u32) }
    }
}

impl Drop for Registrar {
    fn drop(&mut self) {
        self.clear();
    }
}

fn to_win_mods(m: Modifiers) -> HOT_KEY_MODIFIERS {
    let mut out = MOD_NOREPEAT;
    if m.ctrl {
        out |= MOD_CONTROL;
    }
    if m.alt {
        out |= MOD_ALT;
    }
    if m.shift {
        out |= MOD_SHIFT;
    }
    if m.win {
        out |= MOD_WIN;
    }
    out
}

fn current_modifiers() -> Modifiers {
    // SAFETY: GetAsyncKeyState has no preconditions.
    let down = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
    Modifiers {
        ctrl: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        shift: down(VK_SHIFT.0),
        win: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}
