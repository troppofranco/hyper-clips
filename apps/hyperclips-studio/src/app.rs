//! Studio shell: sidebar navigation, top bar (capture status, search,
//! quick share) and the pages.

use std::time::{Duration, Instant};

use eframe::egui::{self, Margin, RichText, Stroke, vec2};
use hc_core::config::{Config, StartPage};
use hc_core::i18n::Localizer;
use hc_core::ipc::{Action, Request};
use hc_core::paths::AppPaths;
use hc_core::sys;

use crate::core_link::CoreLink;
use crate::editor_page::EditorPage;
use crate::icons::Ic;
use crate::library_page::{LibraryPage, View};
use crate::settings::SettingsPage;
use crate::theme::{self, palette};
use crate::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Highlights,
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
    disk: Option<(u64, u64)>,
    disk_at: Option<Instant>,
    notice: Option<(String, Instant)>,
    update_popup: crate::update_popup::UpdatePopup,
}

fn clips_root(paths: &AppPaths, config: &Config) -> std::path::PathBuf {
    config
        .general
        .clips_dir
        .clone()
        .unwrap_or_else(|| paths.default_clips_dir.clone())
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths, config: Config) -> Self {
        theme::apply(&cc.egui_ctx, &config);
        let i18n = Localizer::new(&config.general.language, Some(&paths.exe_dir));
        let page = match config.appearance.start_page {
            StartPage::Library => Page::Library,
            StartPage::Highlights => Page::Highlights,
            StartPage::Editor => Page::Editor,
        };
        Self {
            core: CoreLink::start(cc.egui_ctx.clone()),
            library: LibraryPage::new(&paths, &config),
            editor: EditorPage::new(&paths, clips_root(&paths, &config)),
            vault: crate::vault_page::VaultPage::new(&paths, clips_root(&paths, &config)),
            privacy: crate::privacy_page::PrivacyPage::new(),
            update_popup: crate::update_popup::UpdatePopup::start(&paths, &config),
            settings: SettingsPage::new(config, &paths),
            page,
            i18n,
            paths,
            disk: None,
            disk_at: None,
            notice: None,
        }
    }

    fn title(&self) -> String {
        let key = match self.page {
            Page::Library => "nav-library",
            Page::Highlights => "nav-highlights",
            Page::Editor => "nav-editor",
            Page::Vault => "nav-vault",
            Page::Settings => "nav-settings",
            Page::Privacy => "nav-privacy",
        };
        self.i18n.tr(key)
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        self.sidebar_bottom(ui);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            // Logo: accent rounded square with a play triangle.
            let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 8.0, p.accent);
            crate::icons::paint(ui.painter(), r.shrink(8.0), Ic::Play, p.on_accent);
            ui.label(RichText::new(hc_core::APP_NAME).size(19.0).strong());
        });
        ui.add_space(18.0);
        let pages = [
            (Page::Library, Ic::Library, "nav-library"),
            (Page::Highlights, Ic::Highlight, "nav-highlights"),
            (Page::Editor, Ic::Editor, "nav-editor"),
            (Page::Vault, Ic::Vault, "nav-vault"),
        ];
        for (page, icon, key) in pages {
            if widgets::nav_item(ui, icon, &self.i18n.tr(key), self.page == page).clicked() {
                self.page = page;
            }
        }
        ui.add_space(8.0);
        widgets::divider(ui);
        ui.add_space(8.0);
        for (page, icon, key) in [
            (Page::Settings, Ic::Settings, "nav-settings"),
            (Page::Privacy, Ic::Privacy, "nav-privacy"),
        ] {
            if widgets::nav_item(ui, icon, &self.i18n.tr(key), self.page == page).clicked() {
                self.page = page;
            }
        }
    }

    /// Storage and background-service status, pinned to the bottom.
    fn sidebar_bottom(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("side-bottom")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.storage_card(ui);
                ui.add_space(8.0);
                self.service_status(ui);
                ui.add_space(12.0);
            });
    }

    fn storage_card(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let cfg = self.settings.saved().clone();
        if self
            .disk_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(15))
        {
            self.disk = disk_space(&clips_root(&self.paths, &cfg));
            self.disk_at = Some(Instant::now());
        }
        let (count, used) = self.library.stats();
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(p.radius())
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        crate::icons::icon(ui, Ic::Monitor, 15.0, p.weak);
                        ui.label(RichText::new(self.i18n.tr("side-on-this-pc")).strong());
                    });
                    let (free, total) = self.disk.unwrap_or((0, 0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(sys::format_bytes(used))
                                .color(p.accent)
                                .strong(),
                        );
                        if total > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "/ {} {}",
                                    sys::format_bytes(free),
                                    self.i18n.tr("side-free")
                                ))
                                .color(p.weak)
                                .size(12.0),
                            );
                        }
                    });
                    if total > 0 {
                        let frac = (total - free) as f32 / total as f32;
                        let (r, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 6.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(r, 3.0, p.border);
                        let mut fill = r;
                        fill.set_width(r.width() * frac.clamp(0.0, 1.0));
                        ui.painter().rect_filled(
                            fill,
                            3.0,
                            if frac > 0.9 { p.danger } else { p.accent },
                        );
                    }
                    ui.label(
                        RichText::new(
                            self.i18n
                                .tr_args("side-items", &[("count", &count.to_string())]),
                        )
                        .color(p.weak)
                        .size(12.0),
                    );
                });
            });
    }

    fn service_status(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        match self.core.status() {
            Some(s) => {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, p.ok);
                    ui.label(
                        RichText::new(self.i18n.tr_args(
                            "side-service-on",
                            &[("mem", &sys::format_bytes(s.working_set_bytes))],
                        ))
                        .color(p.weak)
                        .size(12.0),
                    )
                    .on_hover_text(format!("Hyper Clips {}", s.version));
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, p.danger);
                    ui.label(
                        RichText::new(self.i18n.tr("side-service-off"))
                            .color(p.weak)
                            .size(12.0),
                    );
                    if ui
                        .small_button(self.i18n.tr("side-service-start"))
                        .clicked()
                    {
                        let _ =
                            std::process::Command::new(self.paths.sibling_exe(hc_core::CORE_EXE))
                                .arg("--background")
                                .spawn();
                    }
                });
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(self.title()).size(22.0).strong());
            ui.add_space(10.0);
            self.capture_pill(ui);
            if matches!(self.page, Page::Library | Page::Highlights) {
                ui.add_space(8.0);
                self.library.search_box(ui, &self.i18n.tr("lib-search"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if matches!(self.page, Page::Library | Page::Highlights)
                    && widgets::icon_label_button(ui, Ic::Share, &self.i18n.tr("top-share-last"))
                        .on_hover_text(self.i18n.tr("top-share-last-hint"))
                        .clicked()
                {
                    let cfg = self.settings.saved().clone();
                    if let Err(e) = self.library.share_latest(&cfg) {
                        self.notice = Some((e, Instant::now()));
                    }
                }
            });
        });
        ui.add_space(10.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, p.border);
    }

    /// "Replay on / Recording / Not recording" pill with a quick-action menu.
    fn capture_pill(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let status = self.core.status();
        let (color, key) = match &status {
            Some(s) if s.recording => (p.danger, "top-recording"),
            Some(s) if s.replay_active => (p.ok, "top-replay-on"),
            Some(_) => (p.weak, "top-not-recording"),
            None => (p.weak, "side-service-off"),
        };
        let resp = widgets::status_pill(ui, color, &self.i18n.tr(key));
        let i = &self.i18n;
        let hk = self.settings.saved().hotkeys.clone();
        let mut run = None;
        let mut open_settings = false;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(230.0);
            let item = |ui: &mut egui::Ui, label: String, key: &str| {
                let text = if key.is_empty() {
                    label
                } else {
                    format!("{label}    {key}")
                };
                ui.button(text).clicked()
            };
            if item(ui, i.tr("action-save-clip"), &hk.save_clip) {
                run = Some(Action::SaveClip);
            }
            let rec = if status.as_ref().is_some_and(|s| s.recording) {
                i.tr("top-stop-recording")
            } else {
                i.tr("top-start-recording")
            };
            if item(ui, rec, &hk.toggle_recording) {
                run = Some(Action::ToggleRecording);
            }
            if item(ui, i.tr("action-screenshot"), &hk.screenshot) {
                run = Some(Action::Screenshot);
            }
            if item(ui, i.tr("action-add-marker"), &hk.add_marker) {
                run = Some(Action::AddMarker);
            }
            ui.separator();
            if ui.button(i.tr("top-capture-settings")).clicked() {
                open_settings = true;
            }
        });
        if let Some(a) = run {
            match self.core.request(&Request::Run(a)) {
                Ok(hc_core::ipc::Response::Unavailable(m) | hc_core::ipc::Response::Error(m)) => {
                    self.notice = Some((m, Instant::now()));
                }
                Err(_) => self.notice = Some((self.i18n.tr("side-service-off"), Instant::now())),
                Ok(_) => {}
            }
        }
        if open_settings {
            self.page = Page::Settings;
            self.settings
                .open_category(crate::settings::Category::Capture);
        }
    }

    fn notice_toast(&mut self, ctx: &egui::Context) {
        if let Some((msg, at)) = &self.notice {
            if at.elapsed() > Duration::from_secs(4) {
                self.notice = None;
                return;
            }
            egui::Area::new(egui::Id::unique("app-notice"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -30.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg.as_str()));
                });
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = palette(ui.ctx());
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(232.0)
            .frame(
                egui::Frame::new()
                    .fill(p.sidebar)
                    .stroke(Stroke::new(1.0, p.border))
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| self.top_bar(ui));
        let ctx = ui.ctx().clone();
        if self.page != Page::Settings {
            self.settings.end_preview(&ctx);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 14)),
            )
            .show(ui, |ui| match self.page {
                Page::Library | Page::Highlights => {
                    let view = if self.page == Page::Highlights {
                        View::Highlights
                    } else {
                        View::All
                    };
                    let cfg = self.settings.saved().clone();
                    self.library.ui(ui, &self.i18n, &cfg, view);
                    if let Some((path, title)) = self.library.take_edit_request() {
                        self.editor.add_clip(&path, &title);
                        self.page = Page::Editor;
                    }
                    if let Some((id, path)) = self.library.take_vault_request() {
                        self.library.close_detail();
                        self.vault.request_add(id, path);
                        self.page = Page::Vault;
                    }
                    if self.library.take_settings_request() {
                        self.page = Page::Settings;
                        self.settings
                            .open_category(crate::settings::Category::Library);
                    }
                }
                Page::Editor => self.editor.ui(ui, &self.i18n),
                Page::Vault => self.vault.ui(ui, &self.i18n),
                Page::Settings => {
                    if let Some(saved) = self.settings.ui(ui, &self.i18n, &self.paths, &self.core) {
                        theme::apply(ui.ctx(), &saved);
                        self.i18n =
                            Localizer::new(&saved.general.language, Some(&self.paths.exe_dir));
                    }
                }
                Page::Privacy => {
                    let cfg = self.settings.saved().clone();
                    self.privacy
                        .ui(ui, &self.i18n, &self.paths, &cfg, &self.core);
                }
            });
        self.notice_toast(&ctx);
        if let Some(crate::update_popup::PopupAction::Ignore(v)) =
            self.update_popup.ui(&ctx, &self.i18n)
        {
            self.settings.ignore_version(&v, &self.paths);
        }
    }
}

/// (free, total) bytes of the volume holding `dir`.
fn disk_space(dir: &std::path::Path) -> Option<(u64, u64)> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut probe = dir.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: valid NUL-terminated path and out-parameters.
    unsafe {
        GetDiskFreeSpaceExW(
            &windows::core::HSTRING::from(probe.as_os_str()),
            Some(&mut free),
            Some(&mut total),
            None,
        )
        .ok()?;
    }
    Some((free, total))
}
