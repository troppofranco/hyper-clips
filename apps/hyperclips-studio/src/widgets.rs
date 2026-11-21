//! Widgets in the Hyper Launcher style: accent-filled sidebar entries,
//! pill buttons for the top bar, outlined buttons, cards, setting rows and
//! switches. Hover, press and selection are animated (see `anim`).

use eframe::egui::{
    self, Align, Color32, FontId, Layout, Margin, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};

use crate::anim::{flag, mix};
use crate::icons::{Ic, paint};
use crate::theme::palette;

/// Hover and press state of a widget, smoothly animated (0..1 each).
fn motion(ui: &Ui, resp: &Response) -> (f32, f32) {
    let ctx = ui.ctx();
    let hover = flag(ctx, resp.id.with("hover"), resp.hovered(), 0.16);
    let press = flag(
        ctx,
        resp.id.with("press"),
        resp.is_pointer_button_down_on(),
        0.09,
    );
    (hover, press)
}

/// Shrink a rect a little while it is pressed (the launcher's `scale(.97)`).
fn pressed(rect: Rect, press: f32, amount: f32) -> Rect {
    rect.shrink2(rect.size() * (amount * press) / 2.0)
}

/// Sidebar navigation entry: icon + label, filled with the accent when active.
pub fn nav_item(ui: &mut Ui, icon: Ic, label: &str, selected: bool) -> Response {
    let p = palette(ui.ctx());
    let h = if p.compact { 36.0 } else { 42.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let (hover, press) = motion(ui, &resp);
    let sel = flag(ui.ctx(), resp.id.with("sel"), selected, 0.22);
    let rect = pressed(rect, press, 0.03);
    // Hover tints the background; the selection fill grows from the left.
    let hover_bg = mix(Color32::TRANSPARENT, p.card_hover, hover * (1.0 - sel));
    ui.painter().rect_filled(rect, p.radius_sm(), hover_bg);
    if sel > 0.0 {
        let mut fill = rect;
        fill.set_width(rect.width() * (0.35 + 0.65 * sel));
        ui.painter().rect_filled(
            fill,
            p.radius_sm(),
            mix(Color32::TRANSPARENT, p.primary_fill(false), sel),
        );
    }
    let fg = mix(mix(p.weak, p.text, hover), p.on_accent, sel);
    // Icon and label nudge right on hover.
    let nudge = 2.0 * hover * (1.0 - sel);
    paint(
        ui.painter(),
        Rect::from_center_size(
            rect.left_center() + vec2(22.0 + nudge, 0.0),
            Vec2::splat(19.0),
        ),
        icon,
        fg,
    );
    ui.painter().text(
        rect.left_center() + vec2(44.0 + nudge, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        fg,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded pill used in the top bar ("Keybind F8", "60s", "1080p").
/// `key` draws a key-cap; `dropdown` a chevron that flips while open.
pub fn bar_pill(ui: &mut Ui, label: &str, key: Option<&str>, dropdown: bool) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(13.5);
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let kg = key.map(|k| {
        ui.painter()
            .layout_no_wrap(k.to_owned(), FontId::monospace(12.0), p.text)
    });
    let key_w = kg.as_ref().map_or(0.0, |k| k.size().x + 14.0 + 8.0);
    let w = 16.0 + g.size().x + key_w + if dropdown { 22.0 } else { 0.0 } + 12.0;
    let h = if p.compact { 30.0 } else { 36.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let (hover, press) = motion(ui, &resp);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let open_t = flag(ui.ctx(), resp.id.with("open"), open, 0.2);
    let rect = pressed(rect, press, 0.04);
    let fill = mix(p.button_fill(false), p.button_fill(true), hover.max(open_t));
    let mut stroke = p.button_stroke();
    stroke.color = mix(stroke.color, p.accent_line, open_t * 0.8);
    ui.painter()
        .rect(rect, h / 2.0, fill, stroke, StrokeKind::Inside);
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
        let key_bg = mix(p.card_hover, p.accent_soft(), hover);
        ui.painter().rect(
            kr,
            6.0,
            key_bg,
            Stroke::new(1.0, p.line_strong),
            StrokeKind::Inside,
        );
        ui.painter().galley(kr.center() - k.size() / 2.0, k, p.text);
        x = kr.right() + 8.0;
    }
    if dropdown {
        let c = egui::pos2(x + 6.0, rect.center().y);
        let s = Stroke::new(1.6, mix(p.weak, p.text, hover));
        let dy = 2.0 * (1.0 - 2.0 * open_t);
        ui.painter()
            .line_segment([c + vec2(-4.0, -dy), c + vec2(0.0, dy)], s);
        ui.painter()
            .line_segment([c + vec2(0.0, dy), c + vec2(4.0, -dy)], s);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent-filled main action button (glow on hover, shrink on press).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    let font = FontId::proportional(14.0);
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, p.on_accent);
    let h = if p.compact { 32.0 } else { 40.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 32.0, h), Sense::click());
    let enabled = ui.is_enabled();
    let (hover, press) = motion(ui, &resp);
    let hover = if enabled { hover } else { 0.0 };
    let rect = pressed(rect, press, 0.04);
    let mut fill = mix(p.primary_fill(false), p.primary_fill(true), hover);
    if !enabled {
        fill = fill.gamma_multiply(0.5);
    }
    // Soft accent glow (launcher: box-shadow 0 6px 18px accent).
    if hover > 0.0 {
        for k in 1..=3 {
            let grow = 2.0 * k as f32 * hover;
            ui.painter().rect_filled(
                rect.translate(vec2(0.0, 3.0 * hover)).expand(grow),
                p.radius_sm(),
                p.accent_alpha((22.0 * hover / k as f32) as u8),
            );
        }
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

/// Launcher switch: 44×26 track, white knob that slides with a slight stretch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = palette(ui.ctx());
    let size = vec2(44.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = flag(ui.ctx(), resp.id, *on, 0.22);
    let (hover, press) = motion(ui, &resp);
    ui.painter()
        .rect_filled(rect, 13.0, mix(p.track, p.primary_fill(false), t));
    if hover > 0.0 {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            15.0,
            Stroke::new(1.5, p.accent_alpha((90.0 * hover) as u8)),
            StrokeKind::Inside,
        );
    }
    // The knob stretches while it moves or is pressed.
    let moving = 1.0 - (2.0 * t - 1.0).abs();
    let stretch = 6.0 * moving.max(press);
    let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
    let knob = Rect::from_center_size(egui::pos2(x, rect.center().y), vec2(20.0 + stretch, 20.0));
    ui.painter().rect_filled(knob, 10.0, Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Segmented control (`Segmented.svelte`): the highlight slides to the
/// chosen option.
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
    let (outer, outer_resp) = ui.allocate_exact_size(vec2(inner + 8.0, h), Sense::hover());
    ui.painter().rect(
        outer,
        p.radius_sm(),
        p.sidebar,
        Stroke::new(1.0, p.line_strong),
        StrokeKind::Inside,
    );
    let mut rects = Vec::with_capacity(options.len());
    let mut x = outer.left() + 4.0;
    for g in &galleys {
        let r = Rect::from_min_size(
            egui::pos2(x, outer.top() + 3.0),
            vec2(g.size().x + 26.0, h - 6.0),
        );
        x = r.right() + gap;
        rects.push(r);
    }
    // Highlight behind the selected option, sliding between options.
    if let Some(k) = options.iter().position(|(v, _)| v == value) {
        let id = outer_resp.id;
        let x0 = crate::anim::value(ui.ctx(), id.with("x"), rects[k].left() - outer.left(), 0.22);
        let w = crate::anim::value(ui.ctx(), id.with("w"), rects[k].width(), 0.22);
        let hl = Rect::from_min_size(
            egui::pos2(outer.left() + x0, rects[k].top()),
            vec2(w, rects[k].height()),
        );
        ui.painter()
            .rect_filled(hl, p.radius_sm(), p.primary_fill(false));
    }
    for (((v, label), g), rect) in options.iter().zip(galleys).zip(rects) {
        let resp = ui.interact(rect, ui.scope_id().with(("seg", label)), Sense::click());
        let on = *value == *v;
        let hover = flag(ui.ctx(), resp.id.with("hover"), resp.hovered() && !on, 0.15);
        if hover > 0.0 {
            ui.painter().rect_filled(
                rect,
                p.radius_sm(),
                mix(Color32::TRANSPARENT, p.card_hover, hover),
            );
        }
        let sel = flag(ui.ctx(), resp.id.with("on"), on, 0.2);
        let fg = mix(mix(p.weak, p.text, hover), p.on_accent, sel);
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

/// Small coloured status dot followed by text; `live` adds a pulsing ring.
pub fn status_line(ui: &mut Ui, color: Color32, text: &str, live: bool) {
    let p = palette(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        if live {
            let t = crate::anim::pulse(ui.ctx(), 1.6);
            if t > 0.0 {
                let ring = Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    (120.0 * (1.0 - t)) as u8,
                );
                ui.painter().circle_filled(r.center(), 4.0 + 5.0 * t, ring);
            }
        }
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(RichText::new(text).color(p.weak).size(12.0));
    });
}
