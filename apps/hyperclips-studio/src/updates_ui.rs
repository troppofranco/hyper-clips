//! Update panel (Settings → Updates): check now, read the changelog,
//! install in place (same installer as the pop-up) or skip a version.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use hc_core::config::{Config, UpdateChannel};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_updater::{Channel, Release};

use crate::installer::{Installer, State as Install};

enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Error(String),
}

pub struct UpdateUi {
    state: State,
    rx: Option<Receiver<Result<Option<Release>, String>>>,
    installer: Installer,
}

/// What the settings page must persist.
pub enum UpdateAction {
    Ignore(String),
}

impl UpdateUi {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            rx: None,
            installer: Installer::new(),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        config: &Config,
    ) -> Option<UpdateAction> {
        self.installer.poll(paths, ui.ctx());
        if let Some(rx) = &self.rx
            && let Ok(r) = rx.try_recv()
        {
            self.state = match r {
                Ok(Some(rel)) => State::Available(rel),
                Ok(None) => State::UpToDate,
                Err(e) => State::Error(e),
            };
            self.rx = None;
        }
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(format!("Hyper Clips {}", hc_core::VERSION));
            let busy = matches!(self.state, State::Checking) || self.installer.busy();
            if ui
                .add_enabled(!busy, egui::Button::new(i.tr("upd-check-now")))
                .clicked()
            {
                let (tx, rx) = channel();
                let ch = match config.updates.channel {
                    UpdateChannel::Stable => Channel::Stable,
                    UpdateChannel::Beta => Channel::Beta,
                };
                let ignored = config.updates.ignored_versions.clone();
                hc_updater::set_network_log(paths.log_dir.join("network.log"));
                let _ = std::thread::Builder::new()
                    .name("update-check".into())
                    .spawn(move || {
                        let _ = tx.send(hc_updater::check(ch, &ignored).map_err(|e| e.to_string()));
                    });
                self.rx = Some(rx);
                self.state = State::Checking;
            }
        });
        let mut next = None;
        match &self.state {
            State::Idle => {}
            State::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(i.tr("upd-checking"));
                });
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            State::UpToDate => {
                ui.label(i.tr("upd-up-to-date"));
            }
            State::Error(e) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    i.tr_args("upd-error", &[("error", e)]),
                );
            }
            State::Available(r) => {
                ui.strong(i.tr_args("upd-available", &[("version", &r.version.to_string())]));
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.label(if r.notes.trim().is_empty() {
                            "-"
                        } else {
                            r.notes.as_str()
                        });
                    });
                match &self.installer.state {
                    Install::Downloading(p) => {
                        ui.label(
                            i.tr_args("upd-downloading", &[("version", &r.version.to_string())]),
                        );
                        ui.add(egui::ProgressBar::new(*p).show_percentage().animate(true));
                    }
                    Install::Installing => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(i.tr("pop-installing"));
                        });
                    }
                    Install::Failed(e) => {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            i.tr_args("upd-error", &[("error", e)]),
                        );
                    }
                    Install::Idle => {}
                }
                ui.add_enabled_ui(!self.installer.busy(), |ui| {
                    ui.horizontal(|ui| {
                        if crate::widgets::primary_button(ui, &i.tr("pop-update-now")).clicked() {
                            self.installer.start(r, paths);
                        }
                        if ui.button(i.tr("upd-later")).clicked() {
                            next = Some(State::Idle);
                        }
                        if ui.button(i.tr("upd-ignore")).clicked() {
                            action = Some(UpdateAction::Ignore(r.version.to_string()));
                            next = Some(State::Idle);
                        }
                    });
                });
            }
        }
        if let Some(n) = next {
            self.state = n;
        }
        ui.small(i.tr("upd-privacy"));
        action
    }
}
