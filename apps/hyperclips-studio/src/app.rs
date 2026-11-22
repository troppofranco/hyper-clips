//! Studio shell, laid out after the user's sketch: a sidebar with the logo
//! and three entries (Library, Editor, Settings), and a rounded top bar with
//! search, the save-clip key, the replay length and the resolution.

use std::time::{Duration, Instant};

use eframe::egui::{self, Margin, RichText, Stroke, vec2};
use hc_core::config::{Config, StartPage};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_core::sys;

use crate::core_link::CoreLink;
use crate::editor_page::EditorPage;
use crate::icons::Ic;
use crate::library_page::LibraryPage;
use crate::settings::{Category, SettingsPage};
use crate::theme::{self, palette};
use crate::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Editor,
    Settings,
}

const REPLAY_CHOICES: [u32; 7] = [15, 30, 60, 120, 180, 300, 600];
const HEIGHT_CHOICES: [u32; 5] = [720, 1080, 1440, 2160, 0];

pub struct StudioApp {
    paths: AppPaths,
    i18n: Localizer,
    page: Page,
    settings: SettingsPage,
    library: LibraryPage,
    editor: EditorPage,
    vault: crate::vault_page::VaultPage,
    core: CoreLink,
    disk: Option<(u64, u64)>,
    disk_at: Option<Instant>,
    notice: Option<(String, Instant)>,
    update_popup: crate::update_popup::UpdatePopup,
    screen: crate::anim::Screen<Page>,
}

fn clips_root(paths: &AppPaths, config: &Config) -> std::path::PathBuf {
    config
        .general
        .clips_dir
        .clone()
        .unwrap_or_else(|| paths.default_clips_dir.clone())
}

fn height_label(i: &Localizer, h: u32) -> String {
    if h == 0 {
        i.tr("cap-height-source")
    } else {
        format!("{h}p")
    }
}

fn seconds_label(s: u32) -> String {
    if s < 60 || !s.is_multiple_of(60) {
        format!("{s}s")
    } else {
        format!("{} min", s / 60)
    }
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths, config: Config) -> Self {
        theme::apply(&cc.egui_ctx, &config);
        let i18n = Localizer::new(&config.general.language, Some(&paths.exe_dir));
        let page = match config.appearance.start_page {
            StartPage::Library | StartPage::Highlights => Page::Library,
            StartPage::Editor => Page::Editor,
        };
        Self {
            core: CoreLink::start(cc.egui_ctx.clone()),
            library: LibraryPage::new(&paths, &config),
            editor: EditorPage::new(&paths, clips_root(&paths, &config)),
            vault: crate::vault_page::VaultPage::new(&paths, clips_root(&paths, &config)),
            update_popup: crate::update_popup::UpdatePopup::start(&paths, &config),
            settings: SettingsPage::new(config, &paths),
            page,
            i18n,
            paths,
            disk: None,
            disk_at: None,
            notice: None,
            screen: crate::anim::Screen::new(),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        self.sidebar_bottom(ui);
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            // Logo: hexagon mark on a dark rounded tile, as Hyper Launcher.
            let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
            let tile = if p.dark {
                egui::Color32::from_rgb(0x14, 0x17, 0x20)
            } else {
                p.card
            };
            ui.painter().rect_filled(r, 11.0, tile);
            crate::icons::paint(ui.painter(), r.shrink(6.0), Ic::Logo, p.accent_line);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = -2.0;
                ui.label(
                    RichText::new("HYPER")
                        .size(15.0)
                        .strong()
                        .extra_letter_spacing(1.5),
                );
                ui.label(
                    RichText::new("CLIPS")
                        .size(15.0)
                        .strong()
                        .extra_letter_spacing(1.5)
                        .color(p.accent_line),
                );
            });
        });
        ui.add_space(24.0);
        for (page, icon, key) in [
            (Page::Library, Ic::Library, "nav-library"),
            (Page::Editor, Ic::Editor, "nav-editor"),
            (Page::Settings, Ic::Settings, "nav-settings"),
        ] {
            if widgets::nav_item(ui, icon, &self.i18n.tr(key), self.page == page).clicked() {
                self.page = page;
            }
            ui.add_space(4.0);
        }
    }

    /// Capture status, background service and storage, pinned to the bottom.
    fn sidebar_bottom(&mut self, ui: &mut egui::Ui) {
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
        let status = self.core.status();
        egui::Panel::bottom("side-bottom")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                widgets::divider(ui);
                ui.add_space(10.0);
                let (color, key) = match &status {
                    Some(s) if s.recording => (p.danger, "top-recording"),
                    Some(s) if s.replay_active => (p.ok, "top-replay-on"),
                    Some(_) => (p.faint, "top-not-recording"),
                    None => (p.danger, "side-service-off"),
                };
                // Pulse only while recording: a constant animation would keep the UI repainting.
                let live = status.as_ref().is_some_and(|s| s.recording);
                widgets::status_line(ui, color, &self.i18n.tr(key), live);
                if status.is_none()
                    && ui
                        .small_button(self.i18n.tr("side-service-start"))
                        .clicked()
                {
                    let _ = std::process::Command::new(self.paths.sibling_exe(hc_core::CORE_EXE))
                        .arg("--background")
                        .spawn();
                }
                let free = self
                    .disk
                    .map(|(f, _)| sys::format_bytes(f))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(self.i18n.tr_args(
                        "side-storage",
                        &[
                            ("count", &count.to_string()),
                            ("used", &sys::format_bytes(used)),
                            ("free", &free),
                        ],
                    ))
                    .color(p.faint)
                    .size(11.5),
                );
                ui.add_space(14.0);
            });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = palette(ui.ctx());
        let cfg = self.settings.saved().clone();
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(p.radius())
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                // Quick controls on the right; the search (or title) takes all
                // the remaining width, so the bar scales with the window.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Resolution.
                        let r = widgets::bar_pill(
                            ui,
                            &height_label(&self.i18n, cfg.capture.height),
                            None,
                            true,
                        )
                        .on_hover_text(self.i18n.tr("top-resolution-hint"));
                        let mut pick_h = None;
                        egui::Popup::menu(&r).show(|ui| {
                            ui.set_min_width(140.0);
                            for h in HEIGHT_CHOICES {
                                if ui
                                    .selectable_label(
                                        cfg.capture.height == h,
                                        height_label(&self.i18n, h),
                                    )
                                    .clicked()
                                {
                                    pick_h = Some(h);
                                }
                            }
                        });
                        if let Some(h) = pick_h {
                            self.quick_edit(|c| c.capture.height = h);
                        }
                        // Replay length.
                        let r = widgets::bar_pill(
                            ui,
                            &seconds_label(cfg.capture.replay_seconds),
                            None,
                            true,
                        )
                        .on_hover_text(self.i18n.tr("top-replay-hint"));
                        let mut pick_s = None;
                        egui::Popup::menu(&r).show(|ui| {
                            ui.set_min_width(140.0);
                            for s in REPLAY_CHOICES {
                                if ui
                                    .selectable_label(
                                        cfg.capture.replay_seconds == s,
                                        seconds_label(s),
                                    )
                                    .clicked()
                                {
                                    pick_s = Some(s);
                                }
                            }
                        });
                        if let Some(s) = pick_s {
                            self.quick_edit(|c| c.capture.replay_seconds = s);
                        }
                        // Save-clip key.
                        let key = if cfg.hotkeys.save_clip.is_empty() {
                            "-"
                        } else {
                            cfg.hotkeys.save_clip.as_str()
                        };
                        if widgets::bar_pill(ui, &self.i18n.tr("top-keybind"), Some(key), false)
                            .on_hover_text(self.i18n.tr("top-keybind-hint"))
                            .clicked()
                        {
                            self.page = Page::Settings;
                            self.settings.open_category(Category::Hotkeys);
                        }
                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if self.page == Page::Library {
                                let w = ui.available_width();
                                self.library.search_box(ui, &self.i18n, w);
                            } else {
                                let title = match self.page {
                                    Page::Editor => "nav-editor",
                                    _ => "nav-settings",
                                };
                                ui.add_space(6.0);
                                ui.label(RichText::new(self.i18n.tr(title)).size(18.0).strong());
                            }
                        });
                    });
                });
            });
    }

    /// Change a capture setting from the top bar: saved at once and applied
    /// by the core.
    fn quick_edit(&mut self, f: impl FnOnce(&mut Config)) {
        match self.settings.quick_edit(&self.paths, &self.core, f) {
            Ok(()) => self.notice = Some((self.i18n.tr("settings-saved"), Instant::now())),
            Err(e) => self.notice = Some((e, Instant::now())),
        }
    }

    fn notice_toast(&mut self, ctx: &egui::Context) {
        if let Some((msg, at)) = &self.notice {
            if at.elapsed() > Duration::from_secs(3) {
                self.notice = None;
                return;
            }
            // Slides up and fades in, fades out at the end.
            let t = crate::anim::toast(ctx, *at, 3.0);
            egui::Area::new(egui::Id::unique("app-notice"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -30.0 + 14.0 * (1.0 - t)])
                .show(ctx, |ui| {
                    ui.multiply_opacity(t);
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg.as_str()));
                });
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.page != Page::Settings {
            self.settings.end_preview(&ctx);
        }
        theme::paint_background(&ctx);
        let p = palette(&ctx);
        // Sidebar as a floating rounded card, like every other surface.
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(236.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin {
                left: 14,
                right: 0,
                top: 14,
                bottom: 14,
            }))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(p.sidebar)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(p.radius())
                    .inner_margin(Margin::symmetric(12, 0))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.sidebar(ui);
                    });
            });
        egui::Panel::top("top")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 14,
                bottom: 12,
            }))
            .show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin {
                left: 18,
                right: 18,
                top: 6,
                bottom: 14,
            }))
            .show(ui, |ui| {
                // Each page fades and rises in when it opens.
                let t = self.screen.show(ui.ctx(), self.page);
                crate::anim::apply_enter(ui, t);
                match self.page {
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
                            self.page = Page::Settings;
                            self.settings.open_category(Category::Vault);
                        }
                        if self.library.take_settings_request() {
                            self.page = Page::Settings;
                            self.settings.open_category(Category::Library);
                        }
                    }
                    Page::Editor => {
                        self.editor.ui(ui, &self.i18n);
                        if self.editor.take_library_request() {
                            self.page = Page::Library;
                        }
                    }
                    Page::Settings => {
                        let vault = &mut self.vault;
                        let i18n = &self.i18n;
                        let mut embedded = |c: Category, ui: &mut egui::Ui| {
                            if c == Category::Vault {
                                vault.ui(ui, i18n);
                            }
                        };
                        if let Some(saved) =
                            self.settings
                                .ui(ui, &self.i18n, &self.paths, &self.core, &mut embedded)
                        {
                            theme::apply(ui.ctx(), &saved);
                            self.i18n =
                                Localizer::new(&saved.general.language, Some(&self.paths.exe_dir));
                        }
                    }
                }
            });
        self.notice_toast(&ctx);
        if let Some(crate::update_popup::PopupAction::Ignore(v)) =
            self.update_popup.ui(&ctx, &self.i18n, &self.paths)
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
