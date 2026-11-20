use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Middle,
    X1,
    X2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Win32 virtual-key code.
    Vk(u16),
    Mouse(MouseButton),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty key name")]
    EmptyToken,
    #[error("unknown key \"{0}\"")]
    UnknownKey(String),
    #[error("missing a non-modifier key")]
    NoKey,
    #[error("more than one non-modifier key")]
    MultipleKeys,
    #[error("modifier \"{0}\" repeated")]
    DuplicateModifier(String),
}

/// Named keys besides letters, digits and F-keys.
const NAMED: &[(&str, u16)] = &[
    ("Space", 0x20),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Backspace", 0x08),
    ("Escape", 0x1B),
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
    ("NumpadMultiply", 0x6A),
    ("NumpadAdd", 0x6B),
    ("NumpadSubtract", 0x6D),
    ("NumpadDecimal", 0x6E),
    ("NumpadDivide", 0x6F),
    ("Minus", 0xBD),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Period", 0xBE),
    ("Backquote", 0xC0),
];

const ALIASES: &[(&str, &str)] = &[
    ("esc", "Escape"),
    ("return", "Enter"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("prtsc", "PrintScreen"),
    ("printscr", "PrintScreen"),
];

fn parse_key(token: &str) -> Result<Key, ParseError> {
    let lower = token.to_ascii_lowercase();
    let canonical = ALIASES
        .iter()
        .find(|(a, _)| *a == lower)
        .map_or(lower.as_str(), |(_, c)| c);
    let lower = canonical.to_ascii_lowercase();

    match lower.as_str() {
        "mousemiddle" | "mbutton" | "mouse3" => return Ok(Key::Mouse(MouseButton::Middle)),
        "mouse4" | "xbutton1" => return Ok(Key::Mouse(MouseButton::X1)),
        "mouse5" | "xbutton2" => return Ok(Key::Mouse(MouseButton::X2)),
        _ => {}
    }
    let bytes = lower.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        if c.is_ascii_lowercase() {
            return Ok(Key::Vk(u16::from(c.to_ascii_uppercase())));
        }
        if c.is_ascii_digit() {
            return Ok(Key::Vk(u16::from(c)));
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(Key::Vk(0x6F + n));
    }
    if let Some(n) = lower
        .strip_prefix("numpad")
        .and_then(|n| n.parse::<u16>().ok())
        && n <= 9
    {
        return Ok(Key::Vk(0x60 + n));
    }
    NAMED
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&lower))
        .map(|(_, vk)| Key::Vk(*vk))
        .ok_or_else(|| ParseError::UnknownKey(token.to_owned()))
}

fn key_name(key: Key) -> String {
    match key {
        Key::Mouse(MouseButton::Middle) => "MouseMiddle".into(),
        Key::Mouse(MouseButton::X1) => "Mouse4".into(),
        Key::Mouse(MouseButton::X2) => "Mouse5".into(),
        Key::Vk(vk) => match vk {
            0x41..=0x5A | 0x30..=0x39 => char::from(vk as u8).to_string(),
            0x70..=0x87 => format!("F{}", vk - 0x6F),
            0x60..=0x69 => format!("Numpad{}", vk - 0x60),
            _ => NAMED
                .iter()
                .find(|(_, v)| *v == vk)
                .map_or_else(|| format!("VK{vk:#04X}"), |(n, _)| (*n).to_owned()),
        },
    }
}

impl Hotkey {
    /// Parse a binding; an empty/whitespace string means "disabled".
    pub fn parse_optional(s: &str) -> Result<Option<Self>, ParseError> {
        if s.trim().is_empty() {
            Ok(None)
        } else {
            s.parse().map(Some)
        }
    }
}

impl FromStr for Hotkey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut mods = Modifiers::NONE;
        let mut key = None;
        for raw in s.split('+') {
            let token = raw.trim();
            if token.is_empty() {
                return Err(ParseError::EmptyToken);
            }
            let flag = match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some(&mut mods.ctrl),
                "alt" => Some(&mut mods.alt),
                "shift" => Some(&mut mods.shift),
                "win" | "super" | "meta" => Some(&mut mods.win),
                _ => None,
            };
            if let Some(flag) = flag {
                if *flag {
                    return Err(ParseError::DuplicateModifier(token.to_owned()));
                }
                *flag = true;
                continue;
            }
            if key.is_some() {
                return Err(ParseError::MultipleKeys);
            }
            key = Some(parse_key(token)?);
        }
        Ok(Hotkey {
            mods,
            key: key.ok_or(ParseError::NoKey)?,
        })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (on, name) in [
            (m.ctrl, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.win, "Win"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key))
    }
}

/// Pairs of action ids that share the same (valid) hotkey.
pub fn find_duplicates<'a>(bindings: &[(&'a str, &str)]) -> Vec<(&'a str, &'a str)> {
    let parsed: Vec<(&str, Hotkey)> = bindings
        .iter()
        .filter_map(|(id, b)| Hotkey::parse_optional(b).ok().flatten().map(|h| (*id, h)))
        .collect();
    let mut dups = Vec::new();
    for (i, (a, ha)) in parsed.iter().enumerate() {
        for (b, hb) in &parsed[i + 1..] {
            if ha == hb {
                dups.push((*a, *b));
            }
        }
    }
    dups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn parses_common_bindings() {
        assert_eq!(
            hk("F8"),
            Hotkey {
                mods: Modifiers::NONE,
                key: Key::Vk(0x77)
            }
        );
        let h = hk("ctrl + shift + s");
        assert!(h.mods.ctrl && h.mods.shift && !h.mods.alt);
        assert_eq!(h.key, Key::Vk(u16::from(b'S')));
        assert_eq!(hk("Alt+Mouse5").key, Key::Mouse(MouseButton::X2));
        assert_eq!(hk("Numpad7").key, Key::Vk(0x67));
        assert_eq!(hk("F24").key, Key::Vk(0x87));
        assert_eq!(hk("esc").key, Key::Vk(0x1B));
    }

    #[test]
    fn display_is_canonical_and_roundtrips() {
        for s in [
            "F8",
            "Ctrl+F9",
            "Ctrl+Alt+Shift+Win+H",
            "Alt+Mouse4",
            "MouseMiddle",
            "Shift+PageDown",
            "Numpad0",
            "7",
        ] {
            assert_eq!(hk(s).to_string(), s);
            assert_eq!(hk(&hk(s).to_string()), hk(s));
        }
        assert_eq!(hk("shift+ctrl+a").to_string(), "Ctrl+Shift+A");
    }

    #[test]
    fn rejects_bad_bindings() {
        assert_eq!("Ctrl".parse::<Hotkey>(), Err(ParseError::NoKey));
        assert_eq!("A+B".parse::<Hotkey>(), Err(ParseError::MultipleKeys));
        assert_eq!("Ctrl++A".parse::<Hotkey>(), Err(ParseError::EmptyToken));
        assert!(matches!(
            "Ctrl+Ctrl+A".parse::<Hotkey>(),
            Err(ParseError::DuplicateModifier(_))
        ));
        assert!(matches!(
            "Ctrl+Foo".parse::<Hotkey>(),
            Err(ParseError::UnknownKey(_))
        ));
        assert!(matches!(
            "F25".parse::<Hotkey>(),
            Err(ParseError::UnknownKey(_))
        ));
    }

    #[test]
    fn empty_means_disabled() {
        assert_eq!(Hotkey::parse_optional("  "), Ok(None));
    }

    #[test]
    fn duplicates_detected_case_insensitively() {
        let d = find_duplicates(&[("a", "Ctrl+S"), ("b", "F8"), ("c", "control+s"), ("d", "")]);
        assert_eq!(d, vec![("a", "c")]);
    }
}
