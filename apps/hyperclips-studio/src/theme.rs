//! Light / dark / OLED themes.

use eframe::egui::{self, Color32, Visuals};
use hc_core::config::Theme;

const ACCENT: Color32 = Color32::from_rgb(124, 58, 237);
const ACCENT_LIGHT: Color32 = Color32::from_rgb(167, 139, 250);

pub fn visuals(theme: Theme) -> Visuals {
    let mut v = match theme {
        Theme::Light => Visuals::light(),
        Theme::Dark | Theme::Oled => Visuals::dark(),
    };
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = if theme == Theme::Light {
        ACCENT
    } else {
        ACCENT_LIGHT
    };
    if theme == Theme::Oled {
        // Pure black surfaces: pixels off on OLED panels.
        v.panel_fill = Color32::BLACK;
        v.window_fill = Color32::BLACK;
        v.extreme_bg_color = Color32::BLACK;
        v.faint_bg_color = Color32::from_gray(10);
    }
    v
}

pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.set_visuals(visuals(theme));
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
}
