//! Reusable widgets for the studio look: sidebar items, filter chips,
//! icon buttons, setting rows and cards.

use eframe::egui::{
    self, Align, Color32, FontId, Layout, Margin, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};

use crate::icons::{Ic, paint};
use crate::theme::palette;

/// Sidebar navigation entry (icon + label, accent bar when selected).
pub fn nav_item(ui: &mut Ui, icon: Ic, label: &str, selected: bool) -> Response {
    let p = palette(ui.ctx());
    let h = if p.compact { 34.0 } else { 40.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let painter = ui.painter();
    let bg = if selected {
        p.accent_alpha(38)
    } else if resp.hovered() {
        p.card_hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, p.radius(), bg);
    if selected {
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 8.0), vec2(3.0, h - 16.0));
        painter.rect_filled(bar, 2.0, p.accent);
    }
    let color = if selected { p.text } else { p.weak };
    paint(
        painter,
        Rect::from_center_size(rect.left_center() + vec2(24.0, 0.0), Vec2::splat(17.0)),
        icon,
        if selected { p.accent } else { color },
    );
    painter.text(
        rect.left_center() + vec2(44.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.5),
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded filter chip ("All", "Starred", ...).
pub fn chip(ui: &mut Ui, label: &str, selected: bool) -> Response {
    chip_ex(ui, label, selected, false)
}

/// Chip, optionally with a drop-down chevron.
pub fn chip_ex(ui: &mut Ui, label: &str, selected: bool, dropdown: bool) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let extra = if dropdown { 16.0 } else { 0.0 };
    let size = vec2(
        galley.size().x + 28.0 + extra,
        if p.compact { 26.0 } else { 32.0 },
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let (bg, fg, stroke) = if selected {
        (
            p.text,
            if p.dark {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
            Stroke::NONE,
        )
    } else if resp.hovered() {
        (p.card_hover, p.text, Stroke::new(1.0, p.border))
    } else {
        (Color32::TRANSPARENT, p.text, Stroke::new(1.0, p.border))
    };
    ui.painter()
        .rect(rect, rect.height() / 2.0, bg, stroke, StrokeKind::Inside);
    let text_pos = rect.center() - galley.size() / 2.0 - vec2(extra / 2.0, 0.0);
    ui.painter()
        .galley_with_override_text_color(text_pos, galley, fg);
    if dropdown {
        let c = rect.right_center() - vec2(16.0, 0.0);
        let s = Stroke::new(1.5, fg);
        ui.painter()
            .line_segment([c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0)], s);
        ui.painter()
            .line_segment([c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)], s);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Flat button with an icon and optional label.
pub fn icon_label_button(ui: &mut Ui, icon: Ic, label: &str) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let w = if label.is_empty() {
        32.0
    } else {
        galley.size().x + 44.0
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, p.radius(), p.card_hover);
    }
    let ir = Rect::from_center_size(
        if label.is_empty() {
            rect.center()
        } else {
            rect.left_center() + vec2(17.0, 0.0)
        },
        Vec2::splat(15.0),
    );
    paint(ui.painter(), ir, icon, p.text);
    if !label.is_empty() {
        ui.painter().galley(
            rect.left_center() + vec2(30.0, -galley.size().y / 2.0),
            galley,
            p.text,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent-filled main action button.
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).color(p.on_accent).strong())
            .fill(p.accent)
            .corner_radius(p.radius())
            .min_size(vec2(0.0, 32.0)),
    )
}

/// Big page title with a one-line explanation under it.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    let p = palette(ui.ctx());
    ui.label(RichText::new(title).size(24.0).strong());
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(p.weak));
    }
    ui.add_space(if p.compact { 6.0 } else { 12.0 });
}

/// Rounded card with an optional title.
pub fn card<R>(ui: &mut Ui, title: Option<&str>, body: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui.ctx());
    let pad = if p.compact { 10 } else { 16 };
    let r = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(p.radius())
        .inner_margin(Margin::same(pad))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.label(RichText::new(t).size(16.0).strong());
                ui.add_space(4.0);
            }
            body(ui)
        })
        .inner;
    ui.add_space(if p.compact { 6.0 } else { 10.0 });
    r
}

/// One setting: title and help text on the left, the control on the right.
pub fn row(ui: &mut Ui, title: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    let p = palette(ui.ctx());
    ui.add_space(if p.compact { 2.0 } else { 6.0 });
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
    ui.add_space(if p.compact { 2.0 } else { 6.0 });
}

/// Thin separator between rows inside a card.
pub fn divider(ui: &mut Ui) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
}

/// iOS-style on/off switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = palette(ui.ctx());
    let size = vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { p.accent } else { p.border };
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Small coloured status dot followed by text (inside a pill).
pub fn status_pill(ui: &mut Ui, color: Color32, text: &str) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 52.0, 30.0), Sense::click());
    let bg = if resp.hovered() {
        p.border
    } else {
        p.card_hover
    };
    ui.painter().rect_filled(rect, 15.0, bg);
    ui.painter()
        .circle_filled(rect.left_center() + vec2(15.0, 0.0), 4.5, color);
    ui.painter().galley(
        rect.left_center() + vec2(27.0, -galley.size().y / 2.0),
        galley,
        p.text,
    );
    // Chevron.
    let c = rect.right_center() - vec2(14.0, 0.0);
    ui.painter().line_segment(
        [c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0)],
        Stroke::new(1.5, p.weak),
    );
    ui.painter().line_segment(
        [c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)],
        Stroke::new(1.5, p.weak),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
