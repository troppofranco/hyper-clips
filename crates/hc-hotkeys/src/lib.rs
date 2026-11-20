//! Global hotkeys.
//!
//! Keyboard shortcuts use `RegisterHotKey` (no hooks, no injection — safe
//! with anti-cheat software and conflict-aware: Windows refuses a combo
//! already owned by another app). Mouse side/middle buttons use Raw Input
//! with `RIDEV_INPUTSINK`, which only *observes* input and never blocks it.

mod parse;
mod registrar;

pub use parse::{Hotkey, Key, Modifiers, MouseButton, ParseError, find_duplicates};
pub use registrar::{Conflict, Registrar};
