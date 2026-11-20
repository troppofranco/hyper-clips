//! Privacy Audit: live network connections of every Hyper Clips process,
//! the outgoing-request log, the folders we own, what our files contain,
//! and "delete everything".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use hc_core::config::Config;
use hc_core::i18n::Localizer;
use hc_core::ipc::Request;
use hc_core::netaudit::{Connection, tcp_connections};
use hc_core::paths::{AppPaths, InstallMode};
use hc_core::sys;

use crate::core_link::CoreLink;

const REFRESH: Duration = Duration::from_secs(2);
const OUR_EXES: [&str; 3] = [
    hc_core::CORE_EXE,
    hc_core::STUDIO_EXE,
    hc_core::UPDATER_HELPER_EXE,
];

#[derive(Default)]
struct Erase {
    logs: bool,
    cache: bool,
    library: bool,
    settings: bool,
    vault: bool,
    clips: bool,
    confirm: String,
}

pub struct PrivacyPage {
    conns: Vec<Connection>,
    pids: Vec<u32>,
    log_tail: Vec<String>,
    sizes: Vec<(PathBuf, u64)>,
    sizes_at: Option<Instant>,
    last: Option<Instant>,
    erase: Erase,
    message: Option<String>,
}

impl PrivacyPage {
    pub fn new() -> Self {
        Self {
            conns: Vec::new(),
            pids: Vec::new(),
            log_tail: Vec::new(),
            sizes: Vec::new(),
            sizes_at: None,
            last: None,
            erase: Erase {
                logs: true,
                cache: true,
                ..Erase::default()
            },
            message: None,
        }
    }

    fn refresh(&mut self, paths: &AppPaths, clips: &Path) {
        self.pids = OUR_EXES
            .iter()
            .flat_map(|e| sys::find_processes(e))
            .collect();
        self.conns = tcp_connections(&self.pids);
        let log = std::fs::read_to_string(paths.log_dir.join("network.log")).unwrap_or_default();
        self.log_tail = log.lines().rev().take(30).map(str::to_owned).collect();
        // Folder sizes walk the disk: refresh them far less often.
        if self
            .sizes_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(30))
        {
            let mut dirs: Vec<PathBuf> = paths
                .owned_dirs()
                .into_iter()
                .map(Path::to_path_buf)
                .collect();
            dirs.push(clips.to_path_buf());
            self.sizes = dirs
                .into_iter()
                .map(|d| (d.clone(), dir_size(&d)))
                .collect();
            self.sizes_at = Some(Instant::now());
        }
        self.last = Some(Instant::now());
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        cfg: &Config,
        core: &CoreLink,
    ) {
        let clips = cfg
            .general
            .clips_dir
            .clone()
            .unwrap_or_else(|| paths.default_clips_dir.clone());
        if self.last.is_none_or(|t| t.elapsed() > REFRESH) {
            self.refresh(paths, &clips);
        }
        ui.ctx().request_repaint_after(REFRESH);
        ui.heading(i.tr("privacy-title"));
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(i.tr("privacy-network"));
            let mode = match paths.mode {
                InstallMode::Installed => i.tr("mode-installed"),
                InstallMode::Portable => i.tr("mode-portable"),
            };
            ui.label(i.tr_args("privacy-mode", &[("mode", &mode)]));

            section(ui, &i.tr("audit-connections"), |ui| {
                ui.small(i.tr_args(
                    "audit-connections-hint",
                    &[("count", &self.pids.len().to_string())],
                ));
                if self.conns.is_empty() {
                    ui.colored_label(ui.visuals().hyperlink_color, i.tr("audit-no-connections"));
                } else {
                    egui::Grid::new("conns")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            for h in [
                                "audit-col-process",
                                "audit-col-local",
                                "audit-col-remote",
                                "audit-col-state",
                            ] {
                                ui.strong(i.tr(h));
                            }
                            ui.end_row();
                            for c in &self.conns {
                                ui.label(c.pid.to_string());
                                ui.monospace(c.local.to_string());
                                ui.monospace(
                                    c.remote
                                        .map(|r| r.to_string())
                                        .unwrap_or_else(|| "—".into()),
                                );
                                ui.label(c.state);
                                ui.end_row();
                            }
                        });
                    ui.small(i.tr("audit-connections-explain"));
                }
            });

            section(ui, &i.tr("audit-network-log"), |ui| {
                if self.log_tail.is_empty() {
                    ui.label(i.tr("audit-network-log-empty"));
                } else {
                    for l in &self.log_tail {
                        ui.monospace(l);
                    }
                }
            });

            section(ui, &i.tr("privacy-folders"), |ui| {
                egui::Grid::new("privacy-folders")
                    .striped(true)
                    .show(ui, |ui| {
                        for (dir, size) in &self.sizes {
                            ui.monospace(hc_core::paths::redact(dir));
                            ui.label(sys::format_bytes(*size));
                            if ui.small_button(i.tr("audit-open")).clicked() {
                                let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
                            }
                            ui.end_row();
                        }
                    });
            });

            section(ui, &i.tr("audit-metadata"), |ui| {
                ui.label(i.tr("audit-metadata-text"));
            });

            self.erase_ui(ui, i, paths, &clips, core);
        });
    }

    fn erase_ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        clips: &Path,
        core: &CoreLink,
    ) {
        section(ui, &i.tr("audit-erase"), |ui| {
            let e = &mut self.erase;
            ui.checkbox(&mut e.logs, i.tr("erase-logs"));
            ui.checkbox(&mut e.cache, i.tr("erase-cache"));
            ui.checkbox(&mut e.library, i.tr("erase-library"));
            ui.checkbox(&mut e.settings, i.tr("erase-settings"));
            ui.checkbox(&mut e.vault, i.tr("erase-vault"));
            ui.checkbox(&mut e.clips, i.tr("erase-clips"));
            let word = i.tr("erase-confirm-word");
            ui.horizontal(|ui| {
                ui.label(i.tr_args("erase-type", &[("word", &word)]));
                ui.add(egui::TextEdit::singleline(&mut e.confirm).desired_width(120.0));
            });
            ui.colored_label(ui.visuals().warn_fg_color, i.tr("erase-warning"));
            let any = e.logs || e.cache || e.library || e.settings || e.vault || e.clips;
            let ok = any && e.confirm.trim().eq_ignore_ascii_case(&word);
            if ui
                .add_enabled(ok, egui::Button::new(i.tr("erase-run")))
                .clicked()
            {
                // Stop the core first: it holds the logs, the library and the buffer files.
                let _ = core.request(&Request::Shutdown);
                std::thread::sleep(Duration::from_millis(1500));
                let failed = erase(e, paths, clips);
                if failed.is_empty() {
                    // Nothing left to show; the next start begins from scratch.
                    std::process::exit(0);
                }
                self.message = Some(i.tr_args("erase-partial", &[("files", &failed.join(", "))]));
            }
            if let Some(m) = &self.message {
                ui.colored_label(ui.visuals().error_fg_color, m);
            }
        });
    }
}

/// Delete what was selected; returns what could not be removed.
fn erase(e: &Erase, paths: &AppPaths, clips: &Path) -> Vec<String> {
    let mut targets: Vec<PathBuf> = Vec::new();
    if e.logs {
        targets.push(paths.log_dir.clone());
    }
    if e.cache {
        targets.push(paths.cache_dir.clone());
        targets.push(paths.tmp_dir.clone());
    }
    if e.library {
        let db = paths.library_db();
        for suffix in ["", "-wal", "-shm"] {
            targets.push(PathBuf::from(format!("{}{suffix}", db.display())));
        }
        targets.push(paths.thumbs_dir());
    }
    if e.settings {
        targets.push(paths.config_dir.clone());
        targets.push(paths.data_dir.join("plugins"));
        targets.push(paths.data_dir.join("api-token.txt"));
        targets.push(paths.data_dir.join("update-available.txt"));
    }
    if e.vault {
        targets.push(paths.data_dir.join("vault"));
    }
    if e.clips {
        targets.push(clips.to_path_buf());
    }
    let mut failed = Vec::new();
    for t in targets {
        let r = if t.is_dir() {
            std::fs::remove_dir_all(&t)
        } else if t.exists() {
            std::fs::remove_file(&t)
        } else {
            Ok(())
        };
        if r.is_err() {
            failed.push(hc_core::paths::redact(&t));
        }
    }
    failed
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    crate::settings::section(ui, title, body);
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}
