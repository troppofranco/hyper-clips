//! Hyper Clips UI (Phase 1 spike: also used to measure the RAM cost of egui + glow).
//! Talks to the background service over the named pipe; owns no capture logic.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui;
use hc_core::config::{Config, Theme};
use hc_core::consts::{current_user, pipe_path};
use hc_core::i18n::Translator;
use hc_core::ipc::{send_request, Request, Response, ServiceStatus};
use hc_core::paths::AppPaths;
use hc_library::{Clip, Filter, Library};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Result of one status poll.
enum Poll {
    Status(ServiceStatus),
    Offline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    Library,
    Settings,
}

#[derive(Default)]
struct LibraryView {
    clips: Vec<Clip>,
    search: String,
    tag: String,
    favorites_only: bool,
    new_tag: String,
    loaded: bool,
    error: Option<String>,
}

struct App {
    tab: Tab,
    config: Config,
    config_path: PathBuf,
    settings_msg: Option<String>,
    library_path: PathBuf,
    library: Option<Library>,
    view: LibraryView,
    pipe: String,
    tr: Translator,
    updates: Receiver<Poll>,
    status: Option<ServiceStatus>,
    last_action: Option<String>,
}

impl App {
    fn new(
        ctx: &egui::Context,
        config: &Config,
        paths: Option<&AppPaths>,
        lang_dir: &std::path::Path,
    ) -> Self {
        let pipe = pipe_path(&current_user());
        // The service passes the Windows locale; without it "auto" falls back to English.
        let locale = std::env::var("HC_LOCALE").unwrap_or_default();
        let tr = Translator::new(&config.general.language, &locale, lang_dir);
        apply_theme(ctx, config.general.theme);

        // Poll the service from a worker thread so a slow pipe can never freeze the window.
        let (tx, updates) = mpsc::channel();
        let poll_pipe = pipe.clone();
        let poll_ctx = ctx.clone();
        std::thread::spawn(move || loop {
            let poll = match send_request(&poll_pipe, &Request::GetStatus) {
                Ok(Response::Status(s)) => Poll::Status(s),
                _ => Poll::Offline,
            };
            // The receiver is gone when the window closes: stop the thread.
            if tx.send(poll).is_err() {
                break;
            }
            poll_ctx.request_repaint();
            std::thread::sleep(Duration::from_secs(1));
        });

        let config_path = paths.map_or_else(|| PathBuf::from("config.toml"), |p| p.config_file());
        let library_path = paths.map_or_else(
            || PathBuf::from("library.db"),
            |p| p.data_dir().join("library.db"),
        );
        Self {
            tab: Tab::Home,
            config: config.clone(),
            config_path,
            settings_msg: None,
            library_path,
            library: None,
            view: LibraryView::default(),
            pipe,
            tr,
            updates,
            status: None,
            last_action: None,
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let visuals = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
        Theme::Oled => {
            let mut v = egui::Visuals::dark();
            v.panel_fill = egui::Color32::BLACK;
            v.window_fill = egui::Color32::BLACK;
            v.extreme_bg_color = egui::Color32::BLACK;
            v
        }
    };
    ctx.set_visuals(visuals);
}

fn megabytes(kb: Option<u64>) -> String {
    kb.map_or_else(
        || "?".to_owned(),
        |k| format!("{:.1} MB", k as f64 / 1024.0),
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(poll) = self.updates.try_recv() {
            self.status = match poll {
                Poll::Status(s) => Some(s),
                Poll::Offline => None,
            };
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Home, self.tr.t("ui_tab_home"));
                ui.selectable_value(&mut self.tab, Tab::Library, self.tr.t("ui_tab_library"));
                ui.selectable_value(&mut self.tab, Tab::Settings, self.tr.t("ui_tab_settings"));
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Home => self.home_ui(ui),
            Tab::Library => self.library_ui(ui),
            Tab::Settings => self.settings_ui(ui),
        });
    }
}

impl App {
    fn home_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.tr.t("ui_title"));
        ui.add_space(8.0);
        ui.label(self.tr.t("ui_status"));
        match &self.status {
            Some(s) => {
                ui.label(format!("v{} - {} s", s.version, s.uptime_secs));
                ui.label(format!(
                    "{}: {} / {}",
                    self.tr.t("ui_memory"),
                    megabytes(s.private_kb),
                    megabytes(s.working_set_kb)
                ));
            }
            None => {
                ui.colored_label(egui::Color32::LIGHT_RED, self.tr.t("ui_service_offline"));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        if ui.button(self.tr.t("ui_save_clip")).clicked() {
            self.last_action = Some(match send_request(&self.pipe, &Request::SaveClip) {
                Ok(Response::Ok) => "OK".to_owned(),
                Ok(Response::Saved { path }) => path,
                Ok(Response::Error { message }) => message,
                Ok(_) => "?".to_owned(),
                Err(e) => e.to_string(),
            });
            self.view.loaded = false;
        }
        if let Some(msg) = &self.last_action {
            ui.label(msg);
        }
        ui.add_space(8.0);
        ui.separator();
        ui.weak(self.tr.t("ui_privacy_note"));
    }

    fn reload_library(&mut self) {
        if self.library.is_none() {
            match Library::open(&self.library_path) {
                Ok(l) => self.library = Some(l),
                Err(e) => {
                    self.view.error = Some(e.to_string());
                    self.view.loaded = true;
                    return;
                }
            }
        }
        if let Some(lib) = &self.library {
            let filter = Filter {
                game: String::new(),
                favorites_only: self.view.favorites_only,
                tag: self.view.tag.trim().to_lowercase(),
                text: self.view.search.trim().to_owned(),
                limit: 200,
            };
            match lib.list(&filter) {
                Ok(c) => {
                    self.view.clips = c;
                    self.view.error = None;
                }
                Err(e) => self.view.error = Some(e.to_string()),
            }
        }
        self.view.loaded = true;
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        if !self.view.loaded {
            self.reload_library();
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(self.tr.t("lib_search"));
            changed |= ui.text_edit_singleline(&mut self.view.search).lost_focus();
            ui.label(self.tr.t("lib_tag"));
            changed |= ui.text_edit_singleline(&mut self.view.tag).lost_focus();
            changed |= ui
                .checkbox(&mut self.view.favorites_only, self.tr.t("lib_favorites"))
                .changed();
            if ui.button(self.tr.t("lib_refresh")).clicked() {
                changed = true;
            }
        });
        if changed {
            self.reload_library();
        }
        ui.separator();
        if let Some(e) = &self.view.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
        if self.view.clips.is_empty() {
            ui.weak(self.tr.t("lib_empty"));
            return;
        }

        // Actions are collected while drawing and applied afterwards (no borrow conflicts).
        let mut toggle_fav: Option<(i64, bool)> = None;
        let mut delete: Option<(i64, String)> = None;
        let mut open: Option<String> = None;
        let mut tag_add: Option<i64> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for clip in &self.view.clips {
                ui.horizontal(|ui| {
                    let star = if clip.favorite { "[*]" } else { "[ ]" };
                    if ui.small_button(star).clicked() {
                        toggle_fav = Some((clip.id, !clip.favorite));
                    }
                    let name = std::path::Path::new(&clip.path)
                        .file_name()
                        .map_or_else(|| clip.path.clone(), |n| n.to_string_lossy().into_owned());
                    ui.label(name);
                    ui.weak(format!(
                        "{} - {:.1} MB - {}s",
                        format_date(clip.created_at),
                        clip.size_bytes as f64 / 1_048_576.0,
                        clip.duration_ms / 1000
                    ));
                    if ui.small_button(self.tr.t("lib_open")).clicked() {
                        open = Some(clip.path.clone());
                    }
                    if ui.small_button(self.tr.t("lib_delete")).clicked() {
                        delete = Some((clip.id, clip.path.clone()));
                    }
                });
                if !clip.tags.is_empty() {
                    ui.weak(format!("   #{}", clip.tags.join(" #")));
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(self.tr.t("lib_add_tag"));
            let r = ui.text_edit_singleline(&mut self.view.new_tag);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                tag_add = self.view.clips.first().map(|c| c.id);
            }
        });

        if let Some(lib) = &self.library {
            if let Some((id, fav)) = toggle_fav {
                let _ = lib.set_favorite(id, fav);
                self.view.loaded = false;
            }
            if let Some((id, path)) = delete {
                if std::fs::remove_file(&path).is_ok() || !std::path::Path::new(&path).exists() {
                    let _ = lib.remove(id);
                }
                self.view.loaded = false;
            }
            if let Some(id) = tag_add {
                // The newest clip receives the tag; per-clip tagging comes with the editor.
                let _ = lib.add_tag(id, &self.view.new_tag);
                self.view.new_tag.clear();
                self.view.loaded = false;
            }
        }
        if let Some(path) = open {
            open_file(&path);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let cfg = &mut self.config;
        egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
            ui.label(self.tr.t("set_language"));
            egui::ComboBox::from_id_salt("lang")
                .selected_text(cfg.general.language.clone())
                .show_ui(ui, |ui| {
                    for code in ["auto", "en", "it"] {
                        ui.selectable_value(&mut cfg.general.language, code.to_owned(), code);
                    }
                });
            ui.end_row();

            ui.label(self.tr.t("set_theme"));
            egui::ComboBox::from_id_salt("theme")
                .selected_text(format!("{:?}", cfg.general.theme))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.general.theme, Theme::Dark, "Dark");
                    ui.selectable_value(&mut cfg.general.theme, Theme::Light, "Light");
                    ui.selectable_value(&mut cfg.general.theme, Theme::Oled, "Oled");
                });
            ui.end_row();

            ui.label(self.tr.t("set_replay"));
            ui.add(egui::DragValue::new(&mut cfg.replay.duration_secs).range(15..=600));
            ui.end_row();

            ui.label(self.tr.t("set_fps"));
            ui.add(egui::DragValue::new(&mut cfg.video.fps).range(15..=144));
            ui.end_row();

            ui.label(self.tr.t("set_bitrate"));
            ui.add(
                egui::DragValue::new(&mut cfg.video.bitrate_kbps)
                    .range(500..=200_000)
                    .speed(100),
            );
            ui.end_row();
        });
        ui.checkbox(&mut cfg.audio.system, self.tr.t("set_audio_system"));
        ui.checkbox(&mut cfg.audio.microphone, self.tr.t("set_audio_mic"));
        ui.add_space(8.0);
        if ui.button(self.tr.t("set_save")).clicked() {
            self.settings_msg = Some(match self.config.save(&self.config_path) {
                Ok(()) => self.tr.t("set_saved").to_owned(),
                Err(e) => e.to_string(),
            });
        }
        if let Some(m) = &self.settings_msg {
            ui.label(m);
        }
    }
}

/// Opens a file with its default program (Windows only; no-op elsewhere).
fn open_file(path: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix seconds (civil-from-days algorithm).
fn format_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn main() -> eframe::Result {
    // Path or config problems must never prevent the window from opening.
    let paths = AppPaths::detect().ok();
    let config = paths
        .as_ref()
        .and_then(|p| Config::load_or_default(&p.config_file()).ok())
        .unwrap_or_default();
    let lang_dir = paths
        .as_ref()
        .map_or_else(|| PathBuf::from("lang"), |p| p.lang_dir());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 420.0])
            .with_title("Hyper Clips"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Hyper Clips",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                &cc.egui_ctx,
                &config,
                paths.as_ref(),
                &lang_dir,
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::format_date;

    #[test]
    fn formats_unix_dates() {
        assert_eq!(format_date(0), "1970-01-01 00:00");
        assert_eq!(format_date(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_date(951_782_400), "2000-02-29 00:00");
    }
}
