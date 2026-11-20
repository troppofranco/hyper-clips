//! Studio look: light / dark / OLED / high-contrast palettes, user accent
//! colour, rounding, density, UI scale and reduced motion.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use hc_core::config::{Config, Theme};

/// High-contrast accent: yellow on black (WCAG AAA against black).
const HC_ACCENT: Color32 = Color32::from_rgb(255, 230, 0);

/// Colours the custom widgets use (sidebar, cards, badges).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub accent: Color32,
    /// Text drawn on top of the accent colour.
    pub on_accent: Color32,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub badge: Color32,
    pub ok: Color32,
    pub danger: Color32,
    pub radius: u8,
    pub compact: bool,
    pub dark: bool,
}

impl Palette {
    pub fn radius(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }

    /// Accent with the given alpha (selection backgrounds).
    pub fn accent_alpha(&self, a: u8) -> Color32 {
        let [r, g, b, _] = self.accent.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }
}

pub fn palette_for(theme: Theme, accent: [u8; 3], radius: u8, compact: bool) -> Palette {
    let accent = Color32::from_rgb(accent[0], accent[1], accent[2]);
    let lum = 0.299 * f32::from(accent.r())
        + 0.587 * f32::from(accent.g())
        + 0.114 * f32::from(accent.b());
    let on_accent = if lum > 160.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let ok = Color32::from_rgb(34, 197, 94);
    let danger = Color32::from_rgb(239, 68, 68);
    let base = |bg, sidebar, card, card_hover, border, text, weak, badge, dark| Palette {
        accent,
        on_accent,
        bg,
        sidebar,
        card,
        card_hover,
        border,
        text,
        weak,
        badge,
        ok,
        danger,
        radius,
        compact,
        dark,
    };
    match theme {
        Theme::Light => base(
            Color32::from_rgb(244, 245, 248),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(248, 248, 251),
            Color32::from_rgb(226, 228, 234),
            Color32::from_rgb(24, 25, 30),
            Color32::from_rgb(105, 108, 120),
            Color32::from_black_alpha(150),
            false,
        ),
        Theme::Dark => base(
            Color32::from_rgb(14, 15, 19),
            Color32::from_rgb(19, 20, 25),
            Color32::from_rgb(25, 26, 32),
            Color32::from_rgb(32, 34, 41),
            Color32::from_rgb(40, 42, 50),
            Color32::from_rgb(234, 235, 240),
            Color32::from_rgb(142, 145, 158),
            Color32::from_black_alpha(170),
            true,
        ),
        Theme::Oled => base(
            Color32::BLACK,
            Color32::BLACK,
            Color32::from_rgb(14, 14, 16),
            Color32::from_rgb(24, 24, 28),
            Color32::from_rgb(34, 34, 40),
            Color32::from_rgb(236, 236, 240),
            Color32::from_rgb(140, 140, 150),
            Color32::from_black_alpha(200),
            true,
        ),
        Theme::HighContrast => {
            let mut p = base(
                Color32::BLACK,
                Color32::BLACK,
                Color32::BLACK,
                Color32::from_gray(28),
                Color32::WHITE,
                Color32::WHITE,
                Color32::from_gray(220),
                Color32::BLACK,
                true,
            );
            p.accent = HC_ACCENT;
            p.on_accent = Color32::BLACK;
            p
        }
    }
}

pub fn visuals(p: &Palette, theme: Theme) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r = p.radius();
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(p.radius.saturating_add(4));
    v.menu_corner_radius = r;
    v.extreme_bg_color = if p.dark {
        p.sidebar
    } else {
        Color32::from_rgb(236, 237, 241)
    };
    v.faint_bg_color = p.card;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_alpha(if theme == Theme::HighContrast {
        255
    } else {
        110
    });
    v.selection.stroke = Stroke::new(
        1.5,
        if theme == Theme::HighContrast {
            Color32::BLACK
        } else {
            p.accent
        },
    );
    v.error_fg_color = p.danger;
    v.warn_fg_color = Color32::from_rgb(245, 158, 11);
    let hc = theme == Theme::HighContrast;
    let stroke = |w: f32, c: Color32| Stroke::new(if hc { w + 0.5 } else { w }, c);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = stroke(1.0, p.border);
    w.noninteractive.fg_stroke = stroke(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (wv, fill) in [
        (&mut w.inactive, p.card_hover),
        (&mut w.hovered, p.border),
        (&mut w.active, p.accent_alpha(90)),
        (&mut w.open, p.card_hover),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.corner_radius = r;
        wv.fg_stroke = stroke(1.0, p.text);
        wv.bg_stroke = if hc {
            stroke(1.0, Color32::WHITE)
        } else {
            Stroke::NONE
        };
    }
    w.hovered.bg_stroke = stroke(1.0, if hc { HC_ACCENT } else { p.border });
    w.active.bg_stroke = stroke(1.0, p.accent);
    if hc {
        w.inactive.bg_fill = Color32::BLACK;
        w.inactive.weak_bg_fill = Color32::BLACK;
    }
    v
}

/// Apply the configured look to the whole UI.
pub fn apply(ctx: &egui::Context, cfg: &Config) {
    let g = &cfg.general;
    let a = &cfg.appearance;
    let p = palette_for(g.theme, a.accent_rgb(), a.corner_radius, a.compact);
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
    let reduce = g.reduce_motion;
    ctx.global_style_mut(|s| {
        s.animation_time = if reduce { 0.0 } else { 1.0 / 12.0 };
        s.spacing.item_spacing = if compact {
            egui::vec2(6.0, 4.0)
        } else {
            egui::vec2(8.0, 7.0)
        };
        s.spacing.button_padding = if compact {
            egui::vec2(8.0, 3.0)
        } else {
            egui::vec2(12.0, 6.0)
        };
        s.spacing.interact_size.y = if compact { 22.0 } else { 28.0 };
        for (style, size) in [
            (egui::TextStyle::Heading, 22.0),
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 11.5),
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
        .unwrap_or_else(|| palette_for(Theme::Dark, [224, 64, 58], 10, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oled_is_pure_black() {
        let p = palette_for(Theme::Oled, [1, 2, 3], 8, false);
        assert_eq!(p.bg, Color32::BLACK);
        assert!(visuals(&p, Theme::Oled).dark_mode);
        assert!(
            !visuals(
                &palette_for(Theme::Light, [1, 2, 3], 8, false),
                Theme::Light
            )
            .dark_mode
        );
    }

    #[test]
    fn high_contrast_is_white_on_black_with_yellow_accent() {
        let p = palette_for(Theme::HighContrast, [224, 64, 58], 8, false);
        assert_eq!(p.bg, Color32::BLACK);
        assert_eq!(p.text, Color32::WHITE);
        assert_eq!(p.accent, HC_ACCENT);
        assert_eq!(p.on_accent, Color32::BLACK);
    }

    #[test]
    fn text_on_light_accent_is_dark() {
        assert_eq!(
            palette_for(Theme::Dark, [250, 220, 80], 8, false).on_accent,
            Color32::BLACK
        );
        assert_eq!(
            palette_for(Theme::Dark, [30, 40, 160], 8, false).on_accent,
            Color32::WHITE
        );
    }
}
