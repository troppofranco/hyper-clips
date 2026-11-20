use eframe::egui;
use hc_core::config::Config;
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_core::sys;

use crate::core_link::CoreLink;
use crate::editor_page::EditorPage;
use crate::library_page::LibraryPage;
use crate::settings::SettingsPage;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Editor,
    Vault,
    Settings,
    Privacy,
}

pub struct StudioApp {
    paths: AppPaths,
    i18n: Localizer,
    page: Page,
    settings: SettingsPage,
    library: LibraryPage,
    editor: EditorPage,
    vault: crate::vault_page::VaultPage,
    privacy: crate::privacy_page::PrivacyPage,
    core: CoreLink,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths, config: Config) -> Self {
        theme::apply(&cc.egui_ctx, &config.general);
        let i18n = Localizer::new(&config.general.language, Some(&paths.exe_dir));
        Self {
            core: CoreLink::start(cc.egui_ctx.clone()),
            library: LibraryPage::new(&paths, &config),
            editor: EditorPage::new(
                &paths,
                config
                    .general
                    .clips_dir
                    .clone()
                    .unwrap_or_else(|| paths.default_clips_dir.clone()),
            ),
            vault: crate::vault_page::VaultPage::new(
                &paths,
                config
                    .general
                    .clips_dir
                    .clone()
                    .unwrap_or_else(|| paths.default_clips_dir.clone()),
            ),
            privacy: crate::privacy_page::PrivacyPage::new(),
            settings: SettingsPage::new(config, &paths),
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
            (Page::Vault, "nav-vault"),
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
                if let Some((path, title)) = self.library.take_edit_request() {
                    self.editor.add_clip(&path, &title);
                    self.page = Page::Editor;
                }
                if let Some((id, path)) = self.library.take_vault_request() {
                    self.library.close_detail();
                    self.vault.request_add(id, path);
                    self.page = Page::Vault;
                }
            }
            Page::Editor => self.editor.ui(ui, &self.i18n),
            Page::Vault => self.vault.ui(ui, &self.i18n),
            Page::Settings => {
                let outcome = self.settings.ui(ui, &self.i18n, &self.paths, &self.core);
                if let Some(saved) = outcome {
                    theme::apply(ui.ctx(), &saved.general);
                    self.i18n = Localizer::new(&saved.general.language, Some(&self.paths.exe_dir));
                }
            }
            Page::Privacy => {
                let cfg = self.settings.saved().clone();
                self.privacy
                    .ui(ui, &self.i18n, &self.paths, &cfg, &self.core);
            }
        });
    }
}
