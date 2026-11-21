//! Studio look, shared with Hyper Launcher: the same dark/light palettes,
//! accent colours, corner radii, solid or gradient background, standard or
//! "liquid glass" buttons and animation levels. OLED and high contrast are
//! extra variants for accessibility.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use hc_core::config::{
    Animations, Appearance, Background, ButtonStyle, Config, GradientDirection, Theme,
};

/// High-contrast accent: yellow on black (WCAG AAA against black).
const HC_ACCENT: Color32 = Color32::from_rgb(255, 230, 0);

/// Colours and shapes the custom widgets use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub accent: Color32,
    /// Accent for text/lines on the background (lighter on dark themes).
    pub accent_line: Color32,
    /// Text drawn on top of the accent colour.
    pub on_accent: Color32,
    /// Window background (transparent when a gradient is painted behind).
    pub bg: Color32,
    pub bg_deep: Color32,
    /// Sidebar and secondary panels.
    pub sidebar: Color32,
    /// Cards.
    pub card: Color32,
    /// Hovered cards, inputs.
    pub card_hover: Color32,
    pub border: Color32,
    pub line_strong: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub weak: Color32,
    pub faint: Color32,
    pub track: Color32,
    pub badge: Color32,
    pub ok: Color32,
    pub danger: Color32,
    pub radius: u8,
    pub radius_sm: u8,
    pub compact: bool,
    pub dark: bool,
    pub liquid: bool,
    /// (from, to, direction) when the background is a gradient.
    pub gradient: Option<(Color32, Color32, GradientDirection)>,
    pub animations: Animations,
}

impl Palette {
    pub fn radius(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }

    pub fn radius_sm(&self) -> CornerRadius {
        CornerRadius::same(self.radius_sm)
    }

    /// Accent with the given alpha (selection backgrounds).
    pub fn accent_alpha(&self, a: u8) -> Color32 {
        let [r, g, b, _] = self.accent.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    /// Accent mixed into the panel colour (`accent-soft` in the launcher).
    pub fn accent_soft(&self) -> Color32 {
        mix(
            self.accent,
            self.sidebar,
            if self.dark { 0.22 } else { 0.14 },
        )
    }

    /// Fill of a standard / liquid-glass button.
    pub fn button_fill(&self, hovered: bool) -> Color32 {
        match (self.liquid, self.dark) {
            (true, true) => Color32::from_white_alpha(if hovered { 23 } else { 14 }),
            (true, false) => Color32::from_white_alpha(if hovered { 178 } else { 128 }),
            (false, _) if hovered => self.card_hover,
            (false, _) => Color32::TRANSPARENT,
        }
    }

    /// Border of a standard / liquid-glass button.
    pub fn button_stroke(&self) -> Stroke {
        match (self.liquid, self.dark) {
            (true, true) => Stroke::new(1.0, Color32::from_white_alpha(26)),
            (true, false) => Stroke::new(1.0, Color32::from_black_alpha(18)),
            (false, _) => Stroke::new(1.0, self.line_strong),
        }
    }

    /// Fill of the primary (accent) button.
    pub fn primary_fill(&self, hovered: bool) -> Color32 {
        if self.liquid {
            self.accent_alpha(if hovered { 140 } else { 107 })
        } else if hovered {
            mix(self.accent, Color32::WHITE, 0.15)
        } else {
            self.accent
        }
    }
}

fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// `color-mix(in srgb, a t, b)`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) * t + f32::from(y) * (1.0 - t)).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn palette_for(theme: Theme, look: &Appearance) -> Palette {
    let [r, g, b] = look.accent_rgb();
    let accent = Color32::from_rgb(r, g, b);
    let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    let on_accent = if lum > 170.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let radius = look.corner_radius;
    let radius_sm = ((f32::from(radius) * 0.72).round() as u8).max(4);
    let mut p = Palette {
        accent,
        accent_line: mix(accent, Color32::WHITE, 0.7),
        on_accent,
        bg: hex(0x0f1115),
        bg_deep: hex(0x0b0d10),
        sidebar: hex(0x12151b),
        card: hex(0x171a21),
        card_hover: hex(0x1e222b),
        border: hex(0x232731),
        line_strong: hex(0x2a2f3a),
        text: hex(0xe8eaf0),
        text2: hex(0xc4cad6),
        weak: hex(0xa3abba),
        faint: hex(0x7f8796),
        track: hex(0x3a3f4c),
        badge: Color32::from_black_alpha(165),
        ok: hex(0x3fb97f),
        danger: hex(0xe5484d),
        radius,
        radius_sm,
        compact: look.compact,
        dark: true,
        liquid: look.buttons == ButtonStyle::Liquid,
        gradient: None,
        animations: look.animations,
    };
    match theme {
        Theme::Dark => {}
        Theme::Light => {
            p.accent_line = mix(accent, Color32::BLACK, 0.8);
            p.bg = hex(0xf4f5f8);
            p.bg_deep = hex(0xe8eaef);
            p.sidebar = hex(0xeceef3);
            p.card = hex(0xffffff);
            p.card_hover = hex(0xeff1f5);
            p.border = hex(0xe0e3ea);
            p.line_strong = hex(0xcdd2dc);
            p.text = hex(0x15171c);
            p.text2 = hex(0x353a45);
            p.weak = hex(0x5b6371);
            p.faint = hex(0x7b8391);
            p.track = hex(0xc5cad4);
            p.ok = hex(0x23955f);
            p.danger = hex(0xc62b30);
            p.dark = false;
        }
        Theme::Oled => {
            p.bg = Color32::BLACK;
            p.bg_deep = Color32::BLACK;
            p.sidebar = Color32::BLACK;
            p.card = hex(0x0e0f12);
            p.card_hover = hex(0x17191e);
            p.border = hex(0x1c1f26);
            p.line_strong = hex(0x262a33);
        }
        Theme::HighContrast => {
            p.accent = HC_ACCENT;
            p.accent_line = HC_ACCENT;
            p.on_accent = Color32::BLACK;
            p.bg = Color32::BLACK;
            p.bg_deep = Color32::BLACK;
            p.sidebar = Color32::BLACK;
            p.card = Color32::BLACK;
            p.card_hover = Color32::from_gray(28);
            p.border = Color32::WHITE;
            p.line_strong = Color32::WHITE;
            p.text = Color32::WHITE;
            p.text2 = Color32::WHITE;
            p.weak = Color32::from_gray(220);
            p.faint = Color32::from_gray(200);
            p.track = Color32::from_gray(90);
            p.badge = Color32::BLACK;
            p.liquid = false;
        }
    }
    // Gradient background: the window colour disappears and the panels
    // become translucent (as in the launcher).
    if look.background == Background::Gradient && theme != Theme::HighContrast {
        let (f, t) = look.gradient_rgb();
        p.gradient = Some((
            Color32::from_rgb(f[0], f[1], f[2]),
            Color32::from_rgb(t[0], t[1], t[2]),
            look.gradient_direction,
        ));
        let a = f32::from(look.panel_opacity) / 100.0;
        p.bg = Color32::TRANSPARENT;
        p.bg_deep = with_alpha(p.bg_deep, a * 0.85);
        p.sidebar = with_alpha(p.sidebar, a * 0.9);
        p.card = with_alpha(p.card, (a + 0.08).min(1.0));
        p.card_hover = with_alpha(p.card_hover, (a + 0.12).min(1.0));
    }
    p
}

pub fn visuals(p: &Palette, theme: Theme) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r = p.radius_sm();
    v.panel_fill = p.bg;
    v.window_fill = if p.gradient.is_some() {
        p.sidebar
    } else {
        p.card
    };
    v.window_stroke = Stroke::new(1.0, p.line_strong);
    v.window_corner_radius = p.radius();
    v.menu_corner_radius = r;
    v.extreme_bg_color = p.sidebar;
    v.faint_bg_color = p.card;
    v.code_bg_color = p.card_hover;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent_line;
    v.selection.bg_fill = if theme == Theme::HighContrast {
        p.accent
    } else {
        p.accent_alpha(110)
    };
    v.selection.stroke = Stroke::new(
        1.5,
        if theme == Theme::HighContrast {
            Color32::BLACK
        } else {
            p.accent_line
        },
    );
    v.error_fg_color = p.danger;
    v.warn_fg_color = if p.dark { hex(0xf3c98a) } else { hex(0x94570c) };
    v.window_shadow.color = Color32::from_black_alpha(if p.dark { 90 } else { 36 });
    v.popup_shadow.color = Color32::from_black_alpha(if p.dark { 90 } else { 36 });
    let hc = theme == Theme::HighContrast;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    let base = p.button_stroke();
    for (wv, fill) in [
        (&mut w.inactive, p.button_fill(false)),
        (&mut w.hovered, p.button_fill(true)),
        (&mut w.active, p.accent_alpha(90)),
        (&mut w.open, p.button_fill(true)),
    ] {
        wv.bg_fill = if p.liquid { fill } else { p.card_hover };
        wv.weak_bg_fill = fill;
        wv.corner_radius = r;
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.bg_stroke = base;
        wv.expansion = 0.0;
    }
    // Sliders, checkboxes and text inputs use bg_fill: keep them readable.
    w.inactive.bg_fill = if hc { Color32::BLACK } else { p.track };
    w.hovered.bg_stroke = Stroke::new(1.0, if hc { HC_ACCENT } else { p.accent_line });
    w.active.bg_stroke = Stroke::new(1.0, p.accent_line);
    if hc {
        for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.bg_stroke = Stroke::new(1.5, Color32::WHITE);
        }
    }
    v
}

/// Apply the configured look to the whole UI.
pub fn apply(ctx: &egui::Context, cfg: &Config) {
    let g = &cfg.general;
    let a = &cfg.appearance;
    let p = palette_for(g.theme, a);
    let v = visuals(&p, g.theme);
    let pref = if p.dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    };
    ctx.set_theme(pref);
    ctx.set_visuals(v);
    ctx.set_zoom_factor(g.ui_scale.clamp(0.75, 2.0));
    let compact = a.compact;
    let anim = if g.reduce_motion {
        Animations::Off
    } else {
        a.animations
    };
    ctx.global_style_mut(|s| {
        s.animation_time = match anim {
            Animations::Full => 0.18,
            Animations::Reduced => 0.09,
            Animations::Off => 0.0,
        };
        s.spacing.item_spacing = if compact {
            egui::vec2(6.0, 5.0)
        } else {
            egui::vec2(10.0, 8.0)
        };
        s.spacing.button_padding = if compact {
            egui::vec2(10.0, 4.0)
        } else {
            egui::vec2(14.0, 8.0)
        };
        s.spacing.interact_size.y = if compact { 26.0 } else { 32.0 };
        s.spacing.slider_rail_height = 6.0;
        for (style, size) in [
            (egui::TextStyle::Heading, 26.0),
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 12.0),
            (egui::TextStyle::Monospace, 13.0),
        ] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size;
            }
        }
    });
    ctx.data_mut(|d| d.insert_temp(palette_id(), p));
}

fn palette_id() -> egui::Id {
    egui::Id::unique("hc-palette")
}

/// The palette set by the last [`apply`].
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp(palette_id()))
        .unwrap_or_else(|| palette_for(Theme::Dark, &Appearance::default()))
}

/// Paint the gradient background (if any) behind every panel.
pub fn paint_background(ctx: &egui::Context) {
    let p = palette(ctx);
    let Some((from, to, dir)) = p.gradient else {
        return;
    };
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let (a, b) = match dir {
        GradientDirection::Down | GradientDirection::Right => (from, to),
        GradientDirection::Up | GradientDirection::Left => (to, from),
    };
    let mut mesh = egui::Mesh::default();
    let vertical = matches!(dir, GradientDirection::Down | GradientDirection::Up);
    let (c0, c1, c2, c3) = if vertical { (a, a, b, b) } else { (a, b, b, a) };
    mesh.colored_vertex(rect.left_top(), c0);
    mesh.colored_vertex(rect.right_top(), c1);
    mesh.colored_vertex(rect.right_bottom(), c2);
    mesh.colored_vertex(rect.left_bottom(), c3);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_dark_palette() {
        let p = palette_for(Theme::Dark, &Appearance::default());
        assert_eq!(p.bg, hex(0x0f1115));
        assert_eq!(p.accent, hex(0x6e5ae6));
        assert_eq!(p.on_accent, Color32::WHITE);
        assert_eq!(p.radius, 14);
    }

    #[test]
    fn oled_is_pure_black() {
        let p = palette_for(Theme::Oled, &Appearance::default());
        assert_eq!(p.bg, Color32::BLACK);
        assert!(visuals(&p, Theme::Oled).dark_mode);
        assert!(
            !visuals(
                &palette_for(Theme::Light, &Appearance::default()),
                Theme::Light
            )
            .dark_mode
        );
    }

    #[test]
    fn high_contrast_is_white_on_black_with_yellow_accent() {
        let p = palette_for(Theme::HighContrast, &Appearance::default());
        assert_eq!(
            (p.bg, p.text, p.accent, p.on_accent),
            (Color32::BLACK, Color32::WHITE, HC_ACCENT, Color32::BLACK)
        );
    }

    #[test]
    fn gradient_makes_window_transparent() {
        let look = Appearance {
            background: Background::Gradient,
            ..Appearance::default()
        };
        let p = palette_for(Theme::Dark, &look);
        assert_eq!(p.bg, Color32::TRANSPARENT);
        assert!(p.gradient.is_some());
        assert!(p.sidebar.a() < 255);
    }

    #[test]
    fn mix_matches_css() {
        assert_eq!(
            mix(Color32::WHITE, Color32::BLACK, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
    }
}
