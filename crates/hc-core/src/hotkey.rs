//! Hotkey specification: parsing, formatting and conflict detection.
//!
//! Std-only on purpose. Modifier bits and key codes map 1:1 to the Win32
//! `MOD_*` flags and virtual-key codes, so the Windows backend needs no table.

use crate::error::{Error, Result};
use std::fmt;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CTRL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    SaveClip,
    ToggleRecording,
    Screenshot,
    OpenUi,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 4] = [
        HotkeyAction::SaveClip,
        HotkeyAction::ToggleRecording,
        HotkeyAction::Screenshot,
        HotkeyAction::OpenUi,
    ];

    /// Field name used in the config file.
    pub fn config_key(self) -> &'static str {
        match self {
            HotkeyAction::SaveClip => "save_clip",
            HotkeyAction::ToggleRecording => "toggle_recording",
            HotkeyAction::Screenshot => "screenshot",
            HotkeyAction::OpenUi => "open_ui",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Canonical name and Win32 virtual-key code of the non-alphanumeric keys we accept.
const NAMED_KEYS: &[(&str, u32)] = &[
    ("Space", 0x20),
    ("Enter", 0x0D),
    ("Tab", 0x09),
    ("Esc", 0x1B),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
];

const VK_F1: u32 = 0x70;
const VK_F24: u32 = 0x87;
const VK_NUMPAD0: u32 = 0x60;
const VK_NUMPAD9: u32 = 0x69;

fn key_from_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    if let [b] = bytes {
        if b.is_ascii_lowercase() {
            return Some(u32::from(b.to_ascii_uppercase()));
        }
        if b.is_ascii_digit() {
            return Some(u32::from(*b));
        }
    }
    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(VK_F1 - 1 + n);
            }
        }
    }
    if let Some(rest) = lower.strip_prefix("num") {
        if let [d] = rest.as_bytes() {
            if d.is_ascii_digit() {
                return Some(VK_NUMPAD0 + u32::from(d - b'0'));
            }
        }
    }
    NAMED_KEYS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&lower))
        .map(|(_, vk)| *vk)
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        VK_F1..=VK_F24 => format!("F{}", vk - VK_F1 + 1),
        VK_NUMPAD0..=VK_NUMPAD9 => format!("Num{}", vk - VK_NUMPAD0),
        _ => NAMED_KEYS
            .iter()
            .find(|(_, code)| *code == vk)
            .map_or_else(|| format!("0x{vk:02X}"), |(n, _)| (*n).to_owned()),
    }
}

impl Hotkey {
    /// Parses strings such as `"Ctrl+Alt+F10"` (case-insensitive, spaces allowed).
    pub fn parse(spec: &str) -> Result<Self> {
        let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
        let (key_part, mod_parts) = parts
            .split_last()
            .ok_or_else(|| Error::Hotkey("empty hotkey".into()))?;
        if key_part.is_empty() {
            return Err(Error::Hotkey(format!("'{spec}' has no key")));
        }

        let mut modifiers = 0u32;
        for m in mod_parts {
            let bit = match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => MOD_CTRL,
                "alt" => MOD_ALT,
                "shift" => MOD_SHIFT,
                "win" | "super" | "meta" => MOD_WIN,
                other => return Err(Error::Hotkey(format!("unknown modifier '{other}'"))),
            };
            if modifiers & bit != 0 {
                return Err(Error::Hotkey(format!("repeated modifier in '{spec}'")));
            }
            modifiers |= bit;
        }

        let vk = key_from_name(key_part)
            .ok_or_else(|| Error::Hotkey(format!("unknown key '{key_part}'")))?;
        let hotkey = Hotkey { modifiers, vk };
        if modifiers == 0 && !hotkey.is_safe_without_modifier() {
            return Err(Error::Hotkey(format!(
                "'{key_part}' needs at least one modifier"
            )));
        }
        Ok(hotkey)
    }

    /// Keys that can be registered globally without hijacking normal typing.
    fn is_safe_without_modifier(self) -> bool {
        matches!(self.vk, VK_F1..=VK_F24 | 0x2C | 0x13 | 0x91)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (bit, name) in [
            (MOD_CTRL, "Ctrl"),
            (MOD_ALT, "Alt"),
            (MOD_SHIFT, "Shift"),
            (MOD_WIN, "Win"),
        ] {
            if self.modifiers & bit != 0 {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", key_name(self.vk))
    }
}

/// Returns every pair of actions bound to the same hotkey.
pub fn find_conflicts(list: &[(HotkeyAction, Hotkey)]) -> Vec<(HotkeyAction, HotkeyAction)> {
    let mut out = Vec::new();
    for (i, (a, ha)) in list.iter().enumerate() {
        for (b, hb) in &list[i + 1..] {
            if ha == hb {
                out.push((*a, *b));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let hk = Hotkey::parse("alt+f10").unwrap();
        assert_eq!(hk.modifiers, MOD_ALT);
        assert_eq!(hk.vk, 0x79);
        assert_eq!(hk.to_string(), "Alt+F10");
        assert_eq!(
            Hotkey::parse(" ctrl + shift + s ").unwrap().to_string(),
            "Ctrl+Shift+S"
        );
        assert_eq!(Hotkey::parse("Win+Num5").unwrap().to_string(), "Win+Num5");
        assert_eq!(
            Hotkey::parse("Ctrl+PageUp").unwrap().to_string(),
            "Ctrl+PageUp"
        );
    }

    #[test]
    fn f_keys_and_print_screen_work_alone() {
        assert!(Hotkey::parse("F9").is_ok());
        assert!(Hotkey::parse("F24").is_ok());
        assert!(Hotkey::parse("PrintScreen").is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "+",
            "Ctrl+",
            "A",
            "Space",
            "Ctrl+Ctrl+A",
            "Foo+A",
            "Ctrl+F25",
            "Ctrl+Nope",
        ] {
            assert!(Hotkey::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn detects_conflicts() {
        let a = Hotkey::parse("Alt+F9").unwrap();
        let b = Hotkey::parse("Alt+F10").unwrap();
        let list = [
            (HotkeyAction::SaveClip, a),
            (HotkeyAction::Screenshot, b),
            (HotkeyAction::OpenUi, a),
        ];
        assert_eq!(
            find_conflicts(&list),
            vec![(HotkeyAction::SaveClip, HotkeyAction::OpenUi)]
        );
    }
}
