//! "Press a key" capture for hotkeys: click the control, then press the
//! combination (Ctrl/Alt/Shift + key, or mouse button 3/4/5). Esc cancels.
//! The result is normalised through the hotkey parser, so it is always a
//! binding the core can register.

use eframe::egui::{
    self, Event, FontId, Key, Modifiers, PointerButton, Rect, Sense, Stroke, StrokeKind, Ui, vec2,
};
use hc_hotkeys::Hotkey;

use crate::theme::palette;

/// Name the hotkey parser understands for an egui key.
fn key_name(k: Key) -> Option<String> {
    use Key::*;
    let s = match k {
        Space => "Space",
        Tab => "Tab",
        Enter => "Enter",
        Backspace => "Backspace",
        Insert => "Insert",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        ArrowUp => "Up",
        ArrowDown => "Down",
        Minus => "Minus",
        Equals => "Equals",
        Comma => "Comma",
        Period => "Period",
        Backtick => "Backquote",
        Num0 => "0",
        Num1 => "1",
        Num2 => "2",
        Num3 => "3",
        Num4 => "4",
        Num5 => "5",
        Num6 => "6",
        Num7 => "7",
        Num8 => "8",
        Num9 => "9",
        _ => {
            let n = k.name();
            // Letters A-Z and F1-F24 have the same names in both.
            let ok = (n.len() == 1 && n.as_bytes()[0].is_ascii_uppercase())
                || (n.starts_with('F')
                    && n[1..].parse::<u8>().is_ok_and(|x| (1..=24).contains(&x)));
            return ok.then(|| n.to_owned());
        }
    };
    Some(s.to_owned())
}

fn with_mods(m: Modifiers, key: &str) -> String {
    let mut parts = Vec::new();
    if m.ctrl || m.command {
        parts.push("Ctrl");
    }
    if m.alt {
        parts.push("Alt");
    }
    if m.shift {
        parts.push("Shift");
    }
    parts.push(key);
    parts.join("+")
}

/// Canonical binding text (e.g. "Ctrl+Shift+F8"), if the parser accepts it.
fn normalise(s: &str) -> Option<String> {
    Hotkey::parse_optional(s)
        .ok()
        .flatten()
        .map(|h| h.to_string())
}

pub enum Capture {
    /// Still waiting, or nothing happened.
    None,
    /// The user pressed a new combination.
    Set(String),
}

/// Read one combination from this frame's input (consuming it).
fn read_combo(ui: &Ui) -> Option<Result<String, ()>> {
    ui.input_mut(|i| {
        let mut found = None;
        i.events.retain(|e| {
            if found.is_some() {
                return true;
            }
            match e {
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } => {
                    found = Some(Err(()));
                    false
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key_name(*key) {
                    Some(n) => {
                        found = normalise(&with_mods(*modifiers, &n)).map(Ok);
                        found.is_none()
                    }
                    None => true,
                },
                Event::PointerButton {
                    button,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let name = match button {
                        PointerButton::Middle => "MouseMiddle",
                        PointerButton::Extra1 => "Mouse4",
                        PointerButton::Extra2 => "Mouse5",
                        _ => return true,
                    };
                    found = normalise(&with_mods(*modifiers, name)).map(Ok);
                    found.is_none()
                }
                _ => true,
            }
        });
        found
    })
}

/// A key-cap control showing `current`; click it, then press a combination.
/// `label` (top bar) is drawn before the key-cap.
pub fn key_capture(
    ui: &mut Ui,
    id_salt: &str,
    label: Option<&str>,
    current: &str,
    prompt: &str,
) -> Capture {
    let p = palette(ui.ctx());
    let id = ui.make_persistent_id(("keycap", id_salt));
    let mut capturing = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let shown = if capturing {
        prompt.to_owned()
    } else if current.is_empty() {
        "-".to_owned()
    } else {
        current.to_owned()
    };
    let font = FontId::proportional(13.5);
    let lg = label.map(|l| {
        ui.painter()
            .layout_no_wrap(l.to_owned(), font.clone(), p.text)
    });
    let kg = ui
        .painter()
        .layout_no_wrap(shown, FontId::monospace(12.5), p.text);
    let h = if p.compact { 30.0 } else { 36.0 };
    let label_w = lg.as_ref().map_or(0.0, |g| g.size().x + 8.0);
    let w = 16.0 + label_w + kg.size().x + 14.0 + 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let hover = crate::anim::flag(ui.ctx(), id.with("hover"), resp.hovered(), 0.16);
    let on = crate::anim::flag(ui.ctx(), id.with("on"), capturing, 0.2);
    let pulse = if capturing {
        0.5 + 0.5 * (crate::anim::pulse(ui.ctx(), 1.2) * std::f32::consts::TAU).sin()
    } else {
        0.0
    };
    let fill = crate::anim::mix(
        crate::anim::mix(p.button_fill(false), p.button_fill(true), hover),
        p.accent_soft(),
        on,
    );
    let mut stroke = p.button_stroke();
    stroke.color = crate::anim::mix(stroke.color, p.accent_line, on * (0.6 + 0.4 * pulse));
    stroke.width = 1.0 + on * 0.5;
    let radius = if label.is_some() {
        h / 2.0
    } else {
        f32::from(p.radius_sm)
    };
    ui.painter()
        .rect(rect, radius, fill, stroke, StrokeKind::Inside);
    let mut x = rect.left() + 16.0;
    if let Some(g) = lg {
        ui.painter().galley(
            egui::pos2(x, rect.center().y - g.size().y / 2.0),
            g.clone(),
            p.text,
        );
        x += g.size().x + 8.0;
    }
    let kr = Rect::from_min_size(
        egui::pos2(x, rect.center().y - 11.0),
        vec2(kg.size().x + 14.0, 22.0),
    );
    ui.painter().rect(
        kr,
        6.0,
        crate::anim::mix(p.card_hover, p.accent_soft(), hover.max(on)),
        Stroke::new(1.0, p.line_strong),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        kr.center() - kg.size() / 2.0,
        kg,
        if capturing { p.accent_line } else { p.text },
    );
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    let mut result = Capture::None;
    if resp.clicked() && !capturing {
        capturing = true;
    } else if capturing {
        // Clicking elsewhere cancels; a key press (or mouse 3/4/5) is the answer.
        let clicked_away = ui.input(|i| i.pointer.primary_pressed()) && !resp.hovered();
        match read_combo(ui) {
            Some(Ok(combo)) => {
                capturing = false;
                result = Capture::Set(combo);
            }
            Some(Err(())) => capturing = false,
            None if clicked_away => capturing = false,
            None => ui.ctx().request_repaint(),
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, capturing));
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, label.unwrap_or(current))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_are_normalised() {
        let m = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            normalise(&with_mods(m, "F8")).as_deref(),
            Some("Ctrl+Shift+F8")
        );
        assert_eq!(key_name(Key::Insert).as_deref(), Some("Insert"));
        assert_eq!(key_name(Key::A).as_deref(), Some("A"));
        assert_eq!(key_name(Key::F12).as_deref(), Some("F12"));
        assert_eq!(
            normalise("Mouse5").as_deref().map(|s| !s.is_empty()),
            Some(true)
        );
    }
}
