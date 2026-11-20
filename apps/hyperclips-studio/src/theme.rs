//! Light / dark / OLED / high-contrast themes, UI scale and reduced motion.

use eframe::egui::{self, Color32, Stroke, Visuals};
use hc_core::config::{General, Theme};

const ACCENT: Color32 = Color32::from_rgb(124, 58, 237);
const ACCENT_LIGHT: Color32 = Color32::from_rgb(167, 139, 250);
/// High-contrast accent: yellow on black (WCAG AAA against black).
const HC_ACCENT: Color32 = Color32::from_rgb(255, 230, 0);

pub fn visuals(theme: Theme) -> Visuals {
    let mut v = match theme {
        Theme::Light => Visuals::light(),
        Theme::Dark | Theme::Oled | Theme::HighContrast => Visuals::dark(),
    };
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = if theme == Theme::Light {
        ACCENT
    } else {
        ACCENT_LIGHT
    };
    if matches!(theme, Theme::Oled | Theme::HighContrast) {
        // Pure black surfaces: pixels off on OLED panels.
        v.panel_fill = Color32::BLACK;
        v.window_fill = Color32::BLACK;
        v.extreme_bg_color = Color32::BLACK;
        v.faint_bg_color = Color32::from_gray(10);
    }
    if theme == Theme::HighContrast {
        v.override_text_color = Some(Color32::WHITE);
        v.hyperlink_color = HC_ACCENT;
        v.selection.bg_fill = HC_ACCENT;
        v.selection.stroke = Stroke::new(2.0, Color32::BLACK);
        v.faint_bg_color = Color32::from_gray(24);
        v.error_fg_color = Color32::from_rgb(255, 110, 110);
        v.warn_fg_color = HC_ACCENT;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.bg_stroke = Stroke::new(1.5, Color32::WHITE);
            w.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        }
        v.widgets.inactive.bg_fill = Color32::BLACK;
        v.widgets.inactive.weak_bg_fill = Color32::BLACK;
        v.widgets.hovered.bg_stroke = Stroke::new(2.5, HC_ACCENT);
        v.widgets.active.bg_stroke = Stroke::new(2.5, HC_ACCENT);
    }
    v
}

pub fn apply(ctx: &egui::Context, g: &General) {
    ctx.set_visuals(visuals(g.theme));
    ctx.set_zoom_factor(g.ui_scale.clamp(0.75, 2.0));
    ctx.global_style_mut(|s| {
        s.animation_time = if g.reduce_motion { 0.0 } else { 1.0 / 12.0 };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oled_is_pure_black() {
        let v = visuals(Theme::Oled);
        assert_eq!(v.panel_fill, Color32::BLACK);
        assert!(v.dark_mode);
        assert!(!visuals(Theme::Light).dark_mode);
    }

    #[test]
    fn high_contrast_is_white_on_black() {
        let v = visuals(Theme::HighContrast);
        assert_eq!(v.panel_fill, Color32::BLACK);
        assert_eq!(v.override_text_color, Some(Color32::WHITE));
    }
}
