//! Motion helpers. Every animation goes through here so the user's choice
//! (Personalisation → Animations: full, reduced, off; or "reduce motion")
//! applies everywhere: reduced halves durations and drops cascades and
//! shimmer, off makes everything instant.

use std::time::Instant;

use eframe::egui::{self, Color32, Id, emath::easing};
use hc_core::config::Animations;

use crate::theme::palette;

/// Duration multiplier for the current setting (0 = no animation).
pub fn speed(ctx: &egui::Context) -> f32 {
    match palette(ctx).animations {
        Animations::Full => 1.0,
        Animations::Reduced => 0.5,
        Animations::Off => 0.0,
    }
}

/// True when decorative motion (cascades, shimmer, pulses) is allowed.
pub fn rich(ctx: &egui::Context) -> bool {
    palette(ctx).animations == Animations::Full
}

/// Eased 0..1 transition towards `on` (hover, selection, open state).
pub fn flag(ctx: &egui::Context, id: Id, on: bool, base_secs: f32) -> f32 {
    let d = base_secs * speed(ctx);
    if d <= 0.0 {
        return if on { 1.0 } else { 0.0 };
    }
    ctx.animate_bool_with_time_and_easing(id, on, d, easing::cubic_out)
}

/// Smoothly follow a numeric target (positions, widths).
pub fn value(ctx: &egui::Context, id: Id, target: f32, base_secs: f32) -> f32 {
    let d = base_secs * speed(ctx);
    if d <= 0.0 {
        return target;
    }
    ctx.animate_value_with_time(id, target, d)
}

/// Progress 0..1 of an entrance that started at `since`, after `delay`
/// seconds and lasting `base_secs`; keeps repainting until done.
pub fn enter(ctx: &egui::Context, since: Instant, delay: f32, base_secs: f32) -> f32 {
    let s = speed(ctx);
    if s <= 0.0 {
        return 1.0;
    }
    let d = base_secs * s;
    let t = ((since.elapsed().as_secs_f32() - delay * s) / d).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    easing::cubic_out(t)
}

/// Fade of a transient message: in quickly, hold, out at the end.
pub fn toast(ctx: &egui::Context, since: Instant, total_secs: f32) -> f32 {
    let age = since.elapsed().as_secs_f32();
    if speed(ctx) <= 0.0 {
        return if age < total_secs { 1.0 } else { 0.0 };
    }
    ctx.request_repaint();
    let fade_in = (age / 0.18).clamp(0.0, 1.0);
    let fade_out = ((total_secs - age) / 0.3).clamp(0.0, 1.0);
    easing::cubic_out(fade_in.min(fade_out))
}

/// 0..1 repeating pulse (recording indicator); constant when not rich.
pub fn pulse(ctx: &egui::Context, period_secs: f32) -> f32 {
    if !rich(ctx) {
        return 0.0;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    let t = ctx.input(|i| i.time) as f32;
    (t / period_secs).fract()
}

/// Linear blend between two colours (alpha included).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    Color32::from_rgba_premultiplied(f(ar, br), f(ag, bg), f(ab, bb), f(aa, ba))
}

/// Start time of the current "screen" (page or category), reset on change.
#[derive(Debug, Clone, Copy)]
pub struct Screen<K: PartialEq + Copy> {
    key: Option<K>,
    since: Instant,
}

impl<K: PartialEq + Copy> Screen<K> {
    pub fn new() -> Self {
        Self {
            key: None,
            since: Instant::now(),
        }
    }

    /// Note the screen being shown; returns its entrance progress (0..1).
    pub fn show(&mut self, ctx: &egui::Context, key: K) -> f32 {
        if self.key != Some(key) {
            self.key = Some(key);
            self.since = Instant::now();
        }
        enter(ctx, self.since, 0.0, 0.28)
    }
}

/// Fade and lift the contents of `ui` by an entrance progress `t`.
pub fn apply_enter(ui: &mut egui::Ui, t: f32) {
    if t < 1.0 {
        ui.multiply_opacity(t);
        ui.add_space(12.0 * (1.0 - t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_endpoints() {
        let a = Color32::from_rgb(10, 20, 30);
        let b = Color32::from_rgb(110, 120, 130);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(60, 70, 80));
    }
}
