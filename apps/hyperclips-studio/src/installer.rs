//! In-app update installation, shared by the update pop-up and the
//! Settings → Updates panel: download the package for this install kind,
//! verify its SHA-256, start the updater helper, stop the core and close
//! the studio so its files can be replaced. The helper reopens the studio
//! on the new version (or on the old one if it had to roll back).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use hc_core::ipc::{Request, one_shot};
use hc_core::paths::{AppPaths, InstallMode};
use hc_updater::{InstallKind, Release};

enum Msg {
    Progress(f32),
    Downloaded(Result<PathBuf, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    /// Download progress 0..1.
    Downloading(f32),
    /// Helper started; the studio is closing.
    Installing,
    Failed(String),
}

pub struct Installer {
    rx: Option<Receiver<Msg>>,
    pub state: State,
}

pub fn kind_of(paths: &AppPaths) -> InstallKind {
    match paths.mode {
        InstallMode::Installed => InstallKind::Installed,
        InstallMode::Portable => InstallKind::Portable,
    }
}

impl Installer {
    pub fn new() -> Self {
        Self {
            rx: None,
            state: State::Idle,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, State::Downloading(_) | State::Installing)
    }

    /// Start downloading and verifying `release` in the background.
    pub fn start(&mut self, release: &Release, paths: &AppPaths) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let rel = release.clone();
        let kind = kind_of(paths);
        let dir = paths.tmp_dir.join("update");
        hc_updater::set_network_log(paths.log_dir.join("network.log"));
        tracing::info!(version = %rel.version, ?kind, "downloading update");
        let spawned = std::thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let tx2 = tx.clone();
                let res = hc_updater::download_verified(&rel, kind, &dir, &mut |p| {
                    let _ = tx2.send(Msg::Progress(p));
                });
                let _ = tx.send(Msg::Downloaded(res.map_err(|e| e.to_string())));
            });
        match spawned {
            Ok(_) => {
                self.rx = Some(rx);
                self.state = State::Downloading(0.0);
            }
            Err(e) => self.state = State::Failed(e.to_string()),
        }
    }

    /// Handle progress; once the package is verified, hand over to the helper.
    pub fn poll(&mut self, paths: &AppPaths, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let mut done = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Progress(p) => self.state = State::Downloading(p),
                Msg::Downloaded(r) => done = Some(r),
            }
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let Some(result) = done else { return };
        self.rx = None;
        match result.and_then(|pkg| hand_over(&pkg, paths)) {
            Ok(()) => {
                self.state = State::Installing;
                // Let the helper replace the files: stop the core and close.
                let _ = one_shot(&Request::Shutdown, Duration::from_secs(3));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                tracing::warn!(error = %e, "update failed");
                self.state = State::Failed(e);
            }
        }
    }
}

fn hand_over(pkg: &std::path::Path, paths: &AppPaths) -> Result<(), String> {
    let mut pids = hc_core::sys::find_processes(hc_core::CORE_EXE);
    pids.push(std::process::id());
    tracing::info!(package = %hc_core::paths::redact(pkg), "starting the updater helper");
    hc_updater::launch_helper(
        &paths.sibling_exe(hc_core::UPDATER_HELPER_EXE),
        pkg,
        kind_of(paths),
        &paths.exe_dir,
        &paths.data_dir,
        &pids,
        Some(&paths.sibling_exe(hc_core::STUDIO_EXE)),
    )
    .map_err(|e| e.to_string())
}
