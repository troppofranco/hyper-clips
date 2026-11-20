use eframe::egui;
use hc_core::config::Config;
use hc_core::i18n::Localizer;
use hc_core::paths::{AppPaths, InstallMode};
use hc_core::sys;

use crate::core_link::CoreLink;
use crate::library_page::LibraryPage;
use crate::settings::SettingsPage;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Editor,
    Settings,
    Privacy,
}

pub struct StudioApp {
    paths: AppPaths,
    i18n: Localizer,
    page: Page,
    settings: SettingsPage,
    library: LibraryPage,
    core: CoreLink,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths, config: Config) -> Self {
        theme::apply(&cc.egui_ctx, config.general.theme);
        let i18n = Localizer::new(&config.general.language, Some(&paths.exe_dir));
        Self {
            core: CoreLink::start(cc.egui_ctx.clone()),
            library: LibraryPage::new(&paths, &config),
            settings: SettingsPage::new(config),
            page: Page::Library,
            i18n,
            paths,
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading(hc_core::APP_NAME);
        ui.add_space(12.0);
        for (page, key) in [
            (Page::Library, "nav-library"),
            (Page::Editor, "nav-editor"),
            (Page::Settings, "nav-settings"),
            (Page::Privacy, "nav-privacy"),
        ] {
            let label = self.i18n.tr(key);
            if ui
                .add_sized(
                    [ui.available_width(), 32.0],
                    egui::Button::selectable(self.page == page, label),
                )
                .clicked()
            {
                self.page = page;
            }
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let ui_mem = sys::format_bytes(sys::current_memory().working_set);
        let text = match self.core.status() {
            Some(s) => self.i18n.tr_args(
                "status-core-connected",
                &[
                    ("version", &s.version),
                    ("core_mem", &sys::format_bytes(s.working_set_bytes)),
                    ("ui_mem", &ui_mem),
                ],
            ),
            None => self.i18n.tr("status-core-missing"),
        };
        ui.horizontal(|ui| {
            ui.small(text);
        });
    }

    fn privacy(&self, ui: &mut egui::Ui) {
        let i = &self.i18n;
        ui.heading(i.tr("privacy-title"));
        ui.add_space(8.0);
        ui.label(i.tr("privacy-network"));
        ui.add_space(8.0);
        let mode = match self.paths.mode {
            InstallMode::Installed => i.tr("mode-installed"),
            InstallMode::Portable => i.tr("mode-portable"),
        };
        ui.label(i.tr_args("privacy-mode", &[("mode", &mode)]));
        ui.add_space(8.0);
        ui.strong(i.tr("privacy-folders"));
        egui::Grid::new("privacy-folders")
            .striped(true)
            .show(ui, |ui| {
                for dir in self.paths.owned_dirs() {
                    ui.monospace(hc_core::paths::redact(dir));
                    ui.label(sys::format_bytes(dir_size(dir)));
                    ui.end_row();
                }
            });
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav")
            .resizable(false)
            .default_size(180.0)
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Library => {
                let cfg = self.settings.saved().clone();
                self.library.ui(ui, &self.i18n, &cfg);
            }
            Page::Editor => {
                ui.heading(self.i18n.tr("nav-editor"));
                ui.label(self.i18n.tr("placeholder-editor"));
            }
            Page::Settings => {
                let outcome = self.settings.ui(ui, &self.i18n, &self.paths, &self.core);
                if let Some(saved) = outcome {
                    theme::apply(ui.ctx(), saved.general.theme);
                    self.i18n = Localizer::new(&saved.general.language, Some(&self.paths.exe_dir));
                }
            }
            Page::Privacy => self.privacy(ui),
        });
    }
}

fn dir_size(dir: &std::path::Path) -> u64 {
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
