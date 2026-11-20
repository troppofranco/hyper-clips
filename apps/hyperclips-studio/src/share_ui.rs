//! "Share on LAN" window: URL, QR code and countdown for a temporary share.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Rect, Sense, vec2};
use hc_core::config::Share as ShareSettings;
use hc_core::i18n::Localizer;

pub struct ShareWindow {
    share: hc_share::Share,
    qr: Option<hc_share::QrMatrix>,
    title: String,
    url: String,
}

impl ShareWindow {
    pub fn start(path: &std::path::Path, title: &str, s: &ShareSettings) -> Result<Self, String> {
        let mut opts = hc_share::ShareOptions::new(path, title);
        opts.expires = Duration::from_secs(u64::from(s.expire_minutes.max(1)) * 60);
        opts.max_downloads = (s.max_downloads > 0).then_some(s.max_downloads);
        let share = hc_share::Share::start(opts).map_err(|e| e.to_string())?;
        let url = share.url();
        Ok(Self {
            qr: hc_share::qr_matrix(&url).ok(),
            share,
            title: title.to_owned(),
            url,
        })
    }

    /// Draw the window; returns false once it should be closed.
    pub fn ui(&mut self, ctx: &egui::Context, i: &Localizer) -> bool {
        let mut open = true;
        let mut stop = false;
        egui::Window::new(i.tr("share-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.strong(&self.title);
                if !self.share.is_running() {
                    ui.label(i.tr("share-ended"));
                    return;
                }
                ui.label(i.tr("share-scan"));
                if let Some(q) = &self.qr {
                    draw_qr(ui, q, 220.0);
                }
                ui.horizontal(|ui| {
                    ui.monospace(&self.url);
                    if ui.button(i.tr("share-copy")).clicked() {
                        ui.ctx().copy_text(self.url.clone());
                    }
                });
                let left = self
                    .share
                    .expires_at()
                    .saturating_duration_since(Instant::now());
                ui.label(i.tr_args(
                    "share-status",
                    &[
                        (
                            "minutes",
                            &format!("{}:{:02}", left.as_secs() / 60, left.as_secs() % 60),
                        ),
                        ("downloads", &self.share.downloads().to_string()),
                    ],
                ));
                ui.small(i.tr("share-warning"));
                if ui.button(i.tr("share-stop")).clicked() {
                    stop = true;
                }
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            });
        open && !stop
    }
}

fn draw_qr(ui: &mut egui::Ui, q: &hc_share::QrMatrix, size: f32) {
    // Quiet zone of 4 modules, dark modules on white for any theme.
    let n = q.size + 8;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::WHITE);
    let m = size / n as f32;
    for y in 0..q.size {
        for x in 0..q.size {
            if q.get(x, y) {
                let min = rect.min + vec2((x + 4) as f32 * m, (y + 4) as f32 * m);
                p.rect_filled(
                    Rect::from_min_size(min, vec2(m + 0.5, m + 0.5)),
                    0.0,
                    Color32::BLACK,
                );
            }
        }
    }
}
