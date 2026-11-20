//! Update panel: check now, read the changelog, install (verified) or ignore.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use hc_core::config::{Config, UpdateChannel};
use hc_core::i18n::Localizer;
use hc_core::ipc::{Request, one_shot};
use hc_core::paths::{AppPaths, InstallMode};
use hc_updater::{Channel, InstallKind, Release};

enum Msg {
    Checked(Result<Option<Release>, String>),
    Progress(f32),
    Downloaded(Result<PathBuf, String>),
}

enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, f32),
    Error(String),
}

pub struct UpdateUi {
    state: State,
    rx: Option<Receiver<Msg>>,
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
        }
    }

    fn poll(&mut self, paths: &AppPaths, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Checked(Ok(Some(r))) => self.state = State::Available(r),
                Msg::Checked(Ok(None)) => self.state = State::UpToDate,
                Msg::Checked(Err(e)) | Msg::Downloaded(Err(e)) => self.state = State::Error(e),
                Msg::Progress(p) => {
                    if let State::Downloading(_, prog) = &mut self.state {
                        *prog = p;
                    }
                }
                Msg::Downloaded(Ok(pkg)) => {
                    let kind = kind_of(paths);
                    let mut pids = hc_core::sys::find_processes(hc_core::CORE_EXE);
                    pids.push(std::process::id());
                    match hc_updater::launch_helper(
                        &paths.sibling_exe(hc_core::UPDATER_HELPER_EXE),
                        &pkg,
                        kind,
                        &paths.exe_dir,
                        &paths.data_dir,
                        &pids,
                    ) {
                        Ok(()) => {
                            // Let the helper replace the files: stop the core and close.
                            let _ = one_shot(&Request::Shutdown, Duration::from_secs(3));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => self.state = State::Error(e.to_string()),
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        config: &Config,
    ) -> Option<UpdateAction> {
        self.poll(paths, ui.ctx());
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(format!("Hyper Clips {}", hc_core::VERSION));
            let busy = matches!(self.state, State::Checking | State::Downloading(..));
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
                        let _ = tx.send(Msg::Checked(
                            hc_updater::check(ch, &ignored).map_err(|e| e.to_string()),
                        ));
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
                            "—"
                        } else {
                            r.notes.as_str()
                        });
                    });
                ui.horizontal(|ui| {
                    if ui.button(i.tr("upd-install")).clicked() {
                        let (tx, rx) = channel();
                        let rel = r.clone();
                        let kind = kind_of(paths);
                        let dir = paths.tmp_dir.join("update");
                        let _ = std::thread::Builder::new()
                            .name("update-download".into())
                            .spawn(move || {
                                let tx2 = tx.clone();
                                let res =
                                    hc_updater::download_verified(&rel, kind, &dir, &mut |p| {
                                        let _ = tx2.send(Msg::Progress(p));
                                    });
                                let _ = tx.send(Msg::Downloaded(res.map_err(|e| e.to_string())));
                            });
                        self.rx = Some(rx);
                        next = Some(State::Downloading(r.clone(), 0.0));
                    }
                    if ui.button(i.tr("upd-later")).clicked() {
                        next = Some(State::Idle);
                    }
                    if ui.button(i.tr("upd-ignore")).clicked() {
                        action = Some(UpdateAction::Ignore(r.version.to_string()));
                        next = Some(State::Idle);
                    }
                });
            }
            State::Downloading(r, p) => {
                ui.label(i.tr_args("upd-downloading", &[("version", &r.version.to_string())]));
                ui.add(egui::ProgressBar::new(*p).show_percentage());
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
        }
        if let Some(n) = next {
            self.state = n;
        }
        ui.small(i.tr("upd-privacy"));
        action
    }
}

fn kind_of(paths: &AppPaths) -> InstallKind {
    match paths.mode {
        InstallMode::Installed => InstallKind::Installed,
        InstallMode::Portable => InstallKind::Portable,
    }
}
