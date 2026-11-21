//! Widgets in the Hyper Launcher style: accent-filled sidebar entries,
//! pill buttons for the top bar, outlined buttons, cards, setting rows and
//! switches.

use eframe::egui::{
    self, Align, Color32, FontId, Layout, Margin, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};

use crate::icons::{Ic, paint};
use crate::theme::palette;

fn chevron(ui: &Ui, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    ui.painter()
        .line_segment([c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0)], s);
    ui.painter()
        .line_segment([c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], s);
}

/// Sidebar navigation entry: icon + label, filled with the accent when active.
pub fn nav_item(ui: &mut Ui, icon: Ic, label: &str, selected: bool) -> Response {
    let p = palette(ui.ctx());
    let h = if p.compact { 36.0 } else { 42.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let hovered = resp.hovered();
    let (bg, fg) = if selected {
        (p.primary_fill(false), p.on_accent)
    } else if hovered {
        (p.card_hover, p.text)
    } else {
        (Color32::TRANSPARENT, p.weak)
    };
    ui.painter().rect_filled(rect, p.radius_sm(), bg);
    paint(
        ui.painter(),
        Rect::from_center_size(rect.left_center() + vec2(22.0, 0.0), Vec2::splat(19.0)),
        icon,
        fg,
    );
    ui.painter().text(
        rect.left_center() + vec2(44.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        fg,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded pill used in the top bar ("Keybind F8", "60s", "1080p").
/// `key` draws a key-cap; `dropdown` a chevron.
pub fn bar_pill(ui: &mut Ui, label: &str, key: Option<&str>, dropdown: bool) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(13.5);
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), p.text);
    let kg = key.map(|k| {
        ui.painter()
            .layout_no_wrap(k.to_owned(), FontId::monospace(12.0), p.text)
    });
    let key_w = kg.as_ref().map_or(0.0, |k| k.size().x + 14.0 + 8.0);
    let w = 16.0 + g.size().x + key_w + if dropdown { 22.0 } else { 0.0 } + 12.0;
    let h = if p.compact { 30.0 } else { 36.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let fill = p.button_fill(resp.hovered());
    ui.painter()
        .rect(rect, h / 2.0, fill, p.button_stroke(), StrokeKind::Inside);
    let mut x = rect.left() + 16.0;
    ui.painter().galley(
        egui::pos2(x, rect.center().y - g.size().y / 2.0),
        g.clone(),
        p.text,
    );
    x += g.size().x + 8.0;
    if let Some(k) = kg {
        let kr = Rect::from_min_size(
            egui::pos2(x, rect.center().y - 11.0),
            vec2(k.size().x + 14.0, 22.0),
        );
        ui.painter().rect(
            kr,
            6.0,
            p.card_hover,
            Stroke::new(1.0, p.line_strong),
            StrokeKind::Inside,
        );
        ui.painter().galley(kr.center() - k.size() / 2.0, k, p.text);
        x = kr.right() + 8.0;
    }
    if dropdown {
        chevron(ui, egui::pos2(x + 6.0, rect.center().y), p.weak);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent-filled main action button.
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(14.0);
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, p.on_accent);
    let h = if p.compact { 32.0 } else { 40.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 32.0, h), Sense::click());
    let enabled = ui.is_enabled();
    let mut fill = p.primary_fill(resp.hovered() && enabled);
    if !enabled {
        fill = fill.gamma_multiply(0.5);
    }
    let stroke = if p.liquid {
        Stroke::new(1.0, p.accent_alpha(150))
    } else {
        Stroke::NONE
    };
    ui.painter()
        .rect(rect, p.radius_sm(), fill, stroke, StrokeKind::Inside);
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, p.on_accent);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Big page title with a one-line explanation under it.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    let p = palette(ui.ctx());
    ui.label(RichText::new(title).size(26.0).strong());
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(p.weak));
    }
    ui.add_space(if p.compact { 8.0 } else { 16.0 });
}

/// Rounded card with an optional title.
pub fn card<R>(ui: &mut Ui, title: Option<&str>, body: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui.ctx());
    let pad = if p.compact { 12 } else { 18 };
    let r = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(p.radius())
        .inner_margin(Margin::same(pad))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.label(RichText::new(t).size(15.5).strong());
                ui.add_space(6.0);
            }
            body(ui)
        })
        .inner;
    ui.add_space(if p.compact { 8.0 } else { 14.0 });
    r
}

/// Small uppercase section label (`.label` in the launcher).
pub fn label_caps(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .color(p.weak)
            .extra_letter_spacing(0.7),
    );
}

/// One setting: title and help text on the left, the control on the right.
pub fn row(ui: &mut Ui, title: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    let p = palette(ui.ctx());
    ui.add_space(if p.compact { 3.0 } else { 7.0 });
    ui.horizontal(|ui| {
        let text_w = (ui.available_width() * 0.55).max(200.0);
        ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.label(RichText::new(title).strong());
            if !help.is_empty() {
                ui.add(egui::Label::new(RichText::new(help).color(p.weak).size(12.5)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(if p.compact { 3.0 } else { 7.0 });
}

/// Thin separator between rows inside a card.
pub fn divider(ui: &mut Ui) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
}

/// Launcher switch: 44×26 track, white knob.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = palette(ui.ctx());
    let size = vec2(44.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { p.primary_fill(false) } else { p.track };
    ui.painter().rect_filled(rect, 13.0, bg);
    let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 10.0, Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Segmented control (`Segmented.svelte`): one option is "on".
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, String)]) -> bool {
    let p = palette(ui.ctx());
    let mut changed = false;
    let h = if p.compact { 30.0 } else { 36.0 };
    let font = FontId::proportional(13.5);
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, l)| ui.painter().layout_no_wrap(l.clone(), font.clone(), p.text))
        .collect();
    let gap = 2.0;
    let inner: f32 = galleys.iter().map(|g| g.size().x + 26.0).sum::<f32>()
        + gap * (options.len().saturating_sub(1)) as f32;
    // Exact size, so it sits right-aligned in setting rows and reads left to right.
    let (outer, _) = ui.allocate_exact_size(vec2(inner + 8.0, h), Sense::hover());
    ui.painter().rect(
        outer,
        p.radius_sm(),
        p.sidebar,
        Stroke::new(1.0, p.line_strong),
        StrokeKind::Inside,
    );
    let mut x = outer.left() + 4.0;
    for ((v, label), g) in options.iter().zip(galleys) {
        let rect = Rect::from_min_size(
            egui::pos2(x, outer.top() + 3.0),
            vec2(g.size().x + 26.0, h - 6.0),
        );
        x = rect.right() + gap;
        let resp = ui.interact(rect, ui.scope_id().with(("seg", label)), Sense::click());
        let on = *value == *v;
        let (bg, fg) = if on {
            (p.primary_fill(false), p.on_accent)
        } else if resp.hovered() {
            (p.card_hover, p.text)
        } else {
            (Color32::TRANSPARENT, p.weak)
        };
        ui.painter().rect_filled(rect, p.radius_sm(), bg);
        ui.painter()
            .galley_with_override_text_color(rect.center() - g.size() / 2.0, g, fg);
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && !on
        {
            *value = *v;
            changed = true;
        }
    }
    changed
}
/// Small coloured status dot followed by text.
pub fn status_line(ui: &mut Ui, color: Color32, text: &str) {
    let p = palette(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(RichText::new(text).color(p.weak).size(12.0));
    });
}
