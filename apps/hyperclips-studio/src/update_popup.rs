//! "A new version is available" pop-up shown when the studio opens.
//!
//! Checks GitHub once in the background (respecting the update settings:
//! never when checks are off, channel, ignored versions) and, if a newer
//! release exists, shows its version and what is new. "Update now"
//! downloads, verifies and installs it in place (see `installer`).

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, RichText, vec2};
use hc_core::config::{Config, UpdateChannel, UpdateCheck};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_updater::{Channel, Release};

use crate::installer::{Installer, State};

/// What the app must persist after the pop-up closes.
pub enum PopupAction {
    Ignore(String),
}

pub struct UpdatePopup {
    rx: Option<Receiver<Option<Release>>>,
    release: Option<Release>,
    shown_at: std::time::Instant,
    current: String,
    installer: Installer,
}

impl UpdatePopup {
    pub fn start(paths: &AppPaths, config: &Config) -> Self {
        let mut me = Self {
            rx: None,
            release: None,
            shown_at: std::time::Instant::now(),
            current: hc_core::VERSION.to_owned(),
            installer: Installer::new(),
        };
        if config.updates.check == UpdateCheck::Never {
            return me;
        }
        let ch = match config.updates.channel {
            UpdateChannel::Stable => Channel::Stable,
            UpdateChannel::Beta => Channel::Beta,
        };
        let ignored = config.updates.ignored_versions.clone();
        hc_updater::set_network_log(paths.log_dir.join("network.log"));
        let current = current_version();
        if let Ok(v) = &current {
            me.current = v.to_string();
        }
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("update-popup".into())
            .spawn(move || {
                let found =
                    current
                        .ok()
                        .and_then(|v| match hc_updater::check_against(&v, ch, &ignored) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::info!(error = %e, "update check failed");
                                None
                            }
                        });
                let _ = tx.send(found);
            });
        me.rx = spawned.ok().map(|_| rx);
        me
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        i: &Localizer,
        paths: &AppPaths,
    ) -> Option<PopupAction> {
        self.installer.poll(paths, ctx);
        if let Some(rx) = &self.rx
            && let Ok(found) = rx.try_recv()
        {
            if let Some(r) = &found {
                tracing::info!(version = %r.version, "update available");
            }
            self.release = found;
            self.shown_at = std::time::Instant::now();
            self.rx = None;
        }
        if self.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let r = self.release.clone()?;
        let p = crate::theme::palette(ctx);
        let mut close = false;
        let mut action = None;
        let resp = egui::Modal::new(egui::Id::unique("update-popup")).show(ctx, |ui| {
            // Fades in and settles from slightly below.
            let t = crate::anim::enter(ui.ctx(), self.shown_at, 0.0, 0.3);
            ui.multiply_opacity(t);
            ui.add_space(10.0 * (1.0 - t));
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 10.0, p.accent);
                crate::icons::paint(
                    ui.painter(),
                    rect.shrink(10.0),
                    crate::icons::Ic::Download,
                    p.on_accent,
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(i.tr("pop-title")).size(19.0).strong());
                    ui.label(
                        RichText::new(i.tr_args(
                            "pop-versions",
                            &[("new", &r.version.to_string()), ("current", &self.current)],
                        ))
                        .color(p.weak),
                    );
                });
            });
            ui.add_space(10.0);
            let notes = summary(&r.notes);
            if !notes.is_empty() {
                ui.label(RichText::new(i.tr("pop-whats-new")).strong());
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(notes).color(p.weak).size(12.5));
                    });
                ui.add_space(8.0);
            }
            match &self.installer.state {
                State::Downloading(prog) => {
                    ui.label(i.tr_args("upd-downloading", &[("version", &r.version.to_string())]));
                    ui.add(
                        egui::ProgressBar::new(*prog)
                            .show_percentage()
                            .animate(true),
                    );
                }
                State::Installing => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(i.tr("pop-installing"));
                    });
                }
                State::Failed(e) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        i.tr_args("upd-error", &[("error", e)]),
                    );
                }
                State::Idle => {
                    ui.label(
                        RichText::new(i.tr("pop-install-hint"))
                            .color(p.weak)
                            .size(12.5),
                    );
                }
            }
            ui.add_space(12.0);
            let busy = self.installer.busy();
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    let label = if matches!(self.installer.state, State::Failed(_)) {
                        i.tr("pop-retry")
                    } else {
                        i.tr("pop-update-now")
                    };
                    if crate::widgets::primary_button(ui, &label).clicked() {
                        self.installer.start(&r, paths);
                    }
                    if ui.button(i.tr("upd-later")).clicked() {
                        close = true;
                    }
                    if ui.button(i.tr("upd-ignore")).clicked() {
                        action = Some(PopupAction::Ignore(r.version.to_string()));
                        close = true;
                    }
                });
            });
        });
        // While downloading or installing the pop-up cannot be dismissed.
        if close || (resp.should_close() && !self.installer.busy()) {
            self.release = None;
        }
        action
    }
}

/// Current version; debug builds can pretend to be older to test the pop-up.
fn current_version() -> Result<semver::Version, semver::Error> {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("HC_PRETEND_VERSION") {
        return semver::Version::parse(&v);
    }
    semver::Version::parse(hc_core::VERSION)
}

/// Plain-text excerpt of the release notes (bullets only, without markdown).
fn summary(notes: &str) -> String {
    notes
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- "))
        .take(8)
        .map(|l| {
            format!(
                "· {}",
                l.trim_start_matches("- ")
                    .replace("**", "")
                    .replace('`', "")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_summary_keeps_bullets_only() {
        let s =
            summary("## [0.2.0]\n\n### UI\n- **New** sidebar with `icons`\n- Cards\n\nVerify: x\n");
        assert_eq!(s, "· New sidebar with icons\n· Cards");
    }
}
