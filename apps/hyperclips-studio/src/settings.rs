//! Settings page: categories on the left, one explained option per row on
//! the right. Edits a draft copy of the config; appearance changes preview
//! live; a bar at the bottom saves (atomically) or discards the draft, then
//! the core reloads (re-registering hotkeys and reporting conflicts).

use eframe::egui::{self, Align, Layout, RichText, Stroke, vec2};
use hc_core::config::{
    BufferStorage, CaptureMode, Codec, Config, Hotkeys, MonitorChoice, OverlayPosition,
    REPLAY_MAX_SECS, REPLAY_MIN_SECS, RateControl, Region, ScreenshotFormat, StartPage, Theme,
    UpdateChannel, UpdateCheck,
};
use hc_core::i18n::{Localizer, available_languages};
use hc_core::ipc::{HotkeyConflict, Request, Response};
use hc_core::paths::{AppPaths, InstallMode};
use hc_hotkeys::{Hotkey, find_duplicates};

use crate::core_link::CoreLink;
use crate::icons::Ic;
use crate::theme::palette;
use crate::widgets::{card, divider, nav_item, row, toggle};

const FPS_CHOICES: [u32; 6] = [30, 60, 120, 144, 165, 240];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Appearance,
    Capture,
    Audio,
    Overlay,
    Hotkeys,
    Library,
    Performance,
    Profiles,
    Sharing,
    Plugins,
    Updates,
    Backup,
    Vault,
}

impl Category {
    const ALL: [Category; 14] = [
        Category::General,
        Category::Appearance,
        Category::Capture,
        Category::Audio,
        Category::Overlay,
        Category::Hotkeys,
        Category::Library,
        Category::Performance,
        Category::Profiles,
        Category::Sharing,
        Category::Plugins,
        Category::Updates,
        Category::Backup,
        Category::Vault,
    ];

    fn key(self) -> &'static str {
        match self {
            Category::General => "cat-general",
            Category::Appearance => "cat-appearance",
            Category::Capture => "cat-capture",
            Category::Audio => "cat-audio",
            Category::Overlay => "cat-overlay",
            Category::Hotkeys => "cat-hotkeys",
            Category::Library => "cat-library",
            Category::Performance => "cat-performance",
            Category::Profiles => "cat-profiles",
            Category::Sharing => "cat-sharing",
            Category::Plugins => "cat-plugins",
            Category::Updates => "cat-updates",
            Category::Backup => "cat-backup",
            Category::Vault => "cat-vault",
        }
    }

    fn icon(self) -> Ic {
        match self {
            Category::General => Ic::Globe,
            Category::Appearance => Ic::Palette,
            Category::Capture => Ic::Monitor,
            Category::Audio => Ic::Mic,
            Category::Overlay => Ic::Bell,
            Category::Hotkeys => Ic::Keyboard,
            Category::Library => Ic::Folder,
            Category::Performance => Ic::Bolt,
            Category::Profiles => Ic::Gamepad,
            Category::Sharing => Ic::Share,
            Category::Plugins => Ic::Plug,
            Category::Updates => Ic::Download,
            Category::Backup => Ic::Save,
            Category::Vault => Ic::Vault,
        }
    }
}

type Look = (Theme, f32, bool, hc_core::config::Appearance);

pub struct SettingsPage {
    saved: Config,
    draft: Config,
    message: Option<(bool, String)>,
    conflicts: Vec<HotkeyConflict>,
    cameras: Vec<String>,
    update_ui: crate::updates_ui::UpdateUi,
    extra: crate::settings_extra::ExtraState,
    category: Category,
    /// Look last applied to the UI (for the live preview).
    previewed: Option<Look>,
    screen: crate::anim::Screen<Category>,
}

/// Per-action validation result for the hotkey editor (empty = valid).
pub fn hotkey_problems(hk: &Hotkeys) -> Vec<(&'static str, String)> {
    let entries = hk.entries();
    let mut out = Vec::new();
    for (id, binding) in entries {
        if let Err(e) = Hotkey::parse_optional(binding) {
            out.push((id, e.to_string()));
        }
    }
    for (a, b) in find_duplicates(&entries) {
        out.push((b, format!("dup:{a}")));
    }
    out
}

impl SettingsPage {
    pub fn new(config: Config, paths: &AppPaths) -> Self {
        Self {
            saved: config.clone(),
            draft: config,
            message: None,
            conflicts: Vec::new(),
            cameras: hc_capture::webcam::list_cameras(),
            update_ui: crate::updates_ui::UpdateUi::new(),
            extra: crate::settings_extra::ExtraState::new(paths),
            category: Category::General,
            previewed: None,
            screen: crate::anim::Screen::new(),
        }
    }

    pub fn saved(&self) -> &Config {
        &self.saved
    }

    /// Remember that the user skipped `version` (saved immediately).
    pub fn ignore_version(&mut self, version: &str, paths: &AppPaths) {
        for cfg in [&mut self.saved, &mut self.draft] {
            if !cfg.updates.ignored_versions.iter().any(|v| v == version) {
                cfg.updates.ignored_versions.push(version.to_owned());
            }
        }
        if let Err(e) = self.saved.save(&paths.config_file()) {
            tracing::warn!(error = %e, "cannot save ignored version");
        }
    }

    /// Change and save a setting right away (top-bar quick controls), keeping
    /// any unsaved draft edits.
    pub fn quick_edit(
        &mut self,
        paths: &AppPaths,
        core: &CoreLink,
        f: impl Fn(&mut Config),
    ) -> Result<(), String> {
        let mut next = self.saved.clone();
        f(&mut next);
        next.sanitize();
        next.save(&paths.config_file()).map_err(|e| e.to_string())?;
        // Carry the same change into the draft (keeping other unsaved edits).
        f(&mut self.draft);
        self.saved = next;
        let _ = core.request(&Request::ReloadConfig);
        Ok(())
    }

    pub fn open_category(&mut self, c: Category) {
        self.category = c;
    }

    /// Undo any live appearance preview (when leaving the page unsaved).
    pub fn end_preview(&mut self, ctx: &egui::Context) {
        if self.previewed.take().is_some() {
            crate::theme::apply(ctx, &self.saved);
        }
    }

    /// Draw the page; returns the new config when it was saved.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        core: &CoreLink,
        embedded: &mut dyn FnMut(Category, &mut egui::Ui),
    ) -> Option<Config> {
        let problems = hotkey_problems(&self.draft.hotkeys);
        let mut saved = None;
        // Live preview of the look being edited.
        let look: Look = (
            self.draft.general.theme,
            self.draft.general.ui_scale,
            self.draft.general.reduce_motion,
            self.draft.appearance.clone(),
        );
        if self.previewed.as_ref() != Some(&look) {
            crate::theme::apply(ui.ctx(), &self.draft);
            self.previewed = Some(look);
        }
        let p = palette(ui.ctx());
        let mut dirty = self.draft != self.saved;
        {
            // The save bar slides up when there are unsaved changes.
            egui::Panel::bottom("settings-save")
                .frame(
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(p.radius())
                        .inner_margin(egui::Margin::symmetric(14, 10)),
                )
                .show_collapsible(ui, &mut dirty, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(i.tr("settings-unsaved")).strong());
                        if !problems.is_empty() {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                i.tr("settings-fix-hotkeys"),
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_enabled_ui(problems.is_empty(), |ui| {
                                if crate::widgets::primary_button(ui, &i.tr("settings-save"))
                                    .clicked()
                                {
                                    saved = self.save(i, paths, core);
                                }
                            });
                            if ui.button(i.tr("settings-revert")).clicked() {
                                self.draft = self.saved.clone();
                            }
                        });
                    });
                });
        }
        egui::Panel::left("settings-cats")
            .resizable(false)
            .exact_size(210.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 0,
                right: 14,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for c in Category::ALL {
                        if c == Category::Vault {
                            ui.add_space(6.0);
                            crate::widgets::divider(ui);
                            ui.add_space(6.0);
                        }
                        if nav_item(ui, c.icon(), &i.tr(c.key()), self.category == c).clicked() {
                            self.category = c;
                        }
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        // Leave room for the floating scroll bar.
                        ui.set_max_width((ui.available_width() - 18.0).min(860.0));
                        let t = self.screen.show(ui.ctx(), self.category);
                        crate::anim::apply_enter(ui, t);
                        if self.category == Category::Vault {
                            embedded(self.category, ui);
                            return;
                        }
                        crate::widgets::page_header(
                            ui,
                            &i.tr(self.category.key()),
                            &i.tr(&format!("{}-desc", self.category.key())),
                        );
                        if let Some((ok, msg)) = &self.message {
                            let color = if *ok {
                                p.ok
                            } else {
                                ui.visuals().error_fg_color
                            };
                            ui.colored_label(color, msg);
                            ui.add_space(6.0);
                        }
                        match self.category {
                            Category::General => self.general(ui, i, paths),
                            Category::Appearance => self.appearance(ui, i),
                            Category::Capture => self.capture(ui, i),
                            Category::Audio => self.audio(ui, i),
                            Category::Overlay => self.overlay(ui, i),
                            Category::Hotkeys => self.hotkeys(ui, i, &problems),
                            Category::Library => {
                                self.library(ui, i);
                                crate::settings_extra::highlights(&mut self.draft, ui, i);
                            }
                            Category::Performance => {
                                crate::settings_extra::performance(&mut self.draft, ui, i)
                            }
                            Category::Profiles => {
                                crate::settings_extra::profiles(&mut self.draft, ui, i)
                            }
                            Category::Sharing => {
                                crate::settings_extra::share_and_api(
                                    &mut self.extra,
                                    &mut self.draft,
                                    ui,
                                    i,
                                    paths,
                                );
                            }
                            Category::Plugins => {
                                crate::settings_extra::plugins(
                                    &mut self.extra,
                                    &mut self.draft,
                                    ui,
                                    i,
                                    paths,
                                );
                            }
                            Category::Updates => self.updates(ui, i, paths),
                            Category::Vault => {}
                            Category::Backup => {
                                if let Some(crate::settings_extra::ExtraOutcome::Restored(cfg)) =
                                    crate::settings_extra::backup(&mut self.extra, ui, i, paths)
                                {
                                    self.saved = (*cfg).clone();
                                    self.draft = *cfg;
                                    let _ = core.request(&Request::ReloadConfig);
                                    saved = Some(self.saved.clone());
                                }
                            }
                        }
                        ui.add_space(20.0);
                    });
            });
        if saved.is_some() {
            self.previewed = None;
        }
        saved
    }

    fn save(&mut self, i: &Localizer, paths: &AppPaths, core: &CoreLink) -> Option<Config> {
        self.draft.sanitize();
        if let Err(e) = self.draft.save(&paths.config_file()) {
            self.message = Some((
                false,
                i.tr_args("settings-save-failed", &[("error", &e.to_string())]),
            ));
            return None;
        }
        self.saved = self.draft.clone();
        self.conflicts = match core.request(&Request::ReloadConfig) {
            Ok(Response::Reloaded { conflicts }) => conflicts,
            _ => Vec::new(),
        };
        self.message = Some((true, i.tr("settings-saved")));
        Some(self.saved.clone())
    }

    fn general(&mut self, ui: &mut egui::Ui, i: &Localizer, paths: &AppPaths) {
        let g = &mut self.draft.general;
        card(ui, None, |ui| {
            row(
                ui,
                &i.tr("settings-language"),
                &i.tr("settings-language-hint"),
                |ui| {
                    let auto = i.tr("settings-language-auto");
                    let shown = if g.language == "auto" {
                        auto.clone()
                    } else {
                        g.language.clone()
                    };
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut g.language, "auto".into(), auto);
                            for l in available_languages(Some(&paths.exe_dir)) {
                                ui.selectable_value(&mut g.language, l.clone(), l);
                            }
                        });
                },
            );
            divider(ui);
            let portable = paths.mode == InstallMode::Portable;
            let hint = if portable {
                i.tr("settings-autostart-portable")
            } else {
                i.tr("settings-autostart-hint")
            };
            row(ui, &i.tr("settings-autostart"), &hint, |ui| {
                ui.add_enabled_ui(!portable, |ui| toggle(ui, &mut g.start_with_windows));
            });
            divider(ui);
            let folder = g
                .clips_dir
                .clone()
                .unwrap_or_else(|| paths.default_clips_dir.clone());
            row(
                ui,
                &i.tr("settings-clips-folder"),
                &folder.display().to_string(),
                |ui| {
                    if ui.button(i.tr("audit-open")).clicked() {
                        let _ = std::fs::create_dir_all(&folder);
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(&folder)
                            .spawn();
                    }
                },
            );
        });
    }

    fn appearance(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        use hc_core::config::{ACCENTS, Animations, Background, ButtonStyle, GradientDirection};
        let d = &mut self.draft;
        let pal = palette(ui.ctx());
        card(ui, Some(&i.tr("look-theme")), |ui| {
            ui.columns(4, |cols| {
                for (k, (t, key)) in [
                    (Theme::Dark, "theme-dark"),
                    (Theme::Light, "theme-light"),
                    (Theme::Oled, "theme-oled"),
                    (Theme::HighContrast, "theme-high-contrast"),
                ]
                .into_iter()
                .enumerate()
                {
                    let tp = crate::theme::palette_for(t, &d.appearance);
                    if choice(&mut cols[k], &i.tr(key), d.general.theme == t, |pt, r| {
                        // Mini swatch: background / surface split diagonally.
                        pt.rect_filled(r, 7.0, tp.card_hover);
                        let tri = vec![r.left_top(), r.right_top(), r.left_bottom()];
                        pt.add(egui::Shape::convex_polygon(
                            tri,
                            if t == Theme::Light {
                                egui::Color32::WHITE
                            } else {
                                tp.sidebar
                            },
                            Stroke::NONE,
                        ));
                        pt.rect_stroke(
                            r,
                            7.0,
                            Stroke::new(1.0, pal.line_strong),
                            egui::StrokeKind::Inside,
                        );
                    })
                    .clicked()
                    {
                        d.general.theme = t;
                    }
                }
            });
        });
        card(ui, Some(&i.tr("look-accent")), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for hex in ACCENTS {
                    let rgb = hc_core::config::Appearance {
                        accent: hex.into(),
                        ..Default::default()
                    }
                    .accent_rgb();
                    let col = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    let on = d.appearance.accent.eq_ignore_ascii_case(hex);
                    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), egui::Sense::click());
                    let grow = if resp.hovered() { 1.0 } else { 0.0 };
                    ui.painter().circle_filled(r.center(), 16.0 + grow, col);
                    if on {
                        ui.painter()
                            .circle_stroke(r.center(), 17.5, Stroke::new(3.0, pal.text));
                    }
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        d.appearance.accent = hex.into();
                    }
                }
                let custom = !ACCENTS
                    .iter()
                    .any(|h| d.appearance.accent.eq_ignore_ascii_case(h));
                let mut rgb = d.appearance.accent_rgb();
                ui.add_space(4.0);
                if ui
                    .color_edit_button_srgb(&mut rgb)
                    .on_hover_text(i.tr("look-accent-custom"))
                    .changed()
                {
                    d.appearance.accent = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
                ui.label(RichText::new(i.tr("look-custom")).color(if custom {
                    pal.text
                } else {
                    pal.weak
                }));
            });
            if d.general.theme == Theme::HighContrast {
                ui.label(
                    RichText::new(i.tr("look-accent-hc"))
                        .color(pal.weak)
                        .size(12.5),
                );
            }
        });
        card(ui, Some(&i.tr("look-background")), |ui| {
            let a = &mut d.appearance;
            let (gf, gt) = a.gradient_rgb();
            let g0 = egui::Color32::from_rgb(gf[0], gf[1], gf[2]);
            let g1 = egui::Color32::from_rgb(gt[0], gt[1], gt[2]);
            ui.columns(2, |cols| {
                if choice(
                    &mut cols[0],
                    &i.tr("look-bg-solid"),
                    a.background == Background::Solid,
                    |pt, r| {
                        pt.rect_filled(r, 7.0, pal.bg_deep);
                        pt.rect_stroke(
                            r,
                            7.0,
                            Stroke::new(1.0, pal.line_strong),
                            egui::StrokeKind::Inside,
                        );
                    },
                )
                .clicked()
                {
                    a.background = Background::Solid;
                }
                if choice(
                    &mut cols[1],
                    &i.tr("look-bg-gradient"),
                    a.background == Background::Gradient,
                    |pt, r| {
                        let mut mesh = egui::Mesh::default();
                        mesh.colored_vertex(r.left_top(), g0);
                        mesh.colored_vertex(r.right_top(), g0);
                        mesh.colored_vertex(r.right_bottom(), g1);
                        mesh.colored_vertex(r.left_bottom(), g1);
                        mesh.add_triangle(0, 1, 2);
                        mesh.add_triangle(0, 2, 3);
                        pt.add(egui::Shape::mesh(mesh));
                        pt.rect_stroke(
                            r,
                            7.0,
                            Stroke::new(1.0, pal.line_strong),
                            egui::StrokeKind::Inside,
                        );
                    },
                )
                .clicked()
                {
                    a.background = Background::Gradient;
                }
            });
            if a.background == Background::Gradient {
                ui.add_space(8.0);
                row(ui, &i.tr("look-gradient-colors"), "", |ui| {
                    if ui.button(i.tr("look-swap")).clicked() {
                        std::mem::swap(&mut a.gradient_from, &mut a.gradient_to);
                    }
                    for v in [&mut a.gradient_to, &mut a.gradient_from] {
                        let mut rgb = hc_core::config::Appearance {
                            accent: v.clone(),
                            ..Default::default()
                        }
                        .accent_rgb();
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            *v = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                        }
                    }
                });
                divider(ui);
                row(ui, &i.tr("look-direction"), "", |ui| {
                    crate::widgets::segmented(
                        ui,
                        &mut a.gradient_direction,
                        &[
                            (GradientDirection::Down, i.tr("dir-down")),
                            (GradientDirection::Up, i.tr("dir-up")),
                            (GradientDirection::Right, i.tr("dir-right")),
                            (GradientDirection::Left, i.tr("dir-left")),
                        ],
                    );
                });
                divider(ui);
                row(ui, &i.tr("look-panel-opacity"), "", |ui| {
                    ui.add(egui::Slider::new(&mut a.panel_opacity, 20..=100).suffix(" %"));
                });
            }
        });
        card(ui, Some(&i.tr("look-shape")), |ui| {
            let a = &mut d.appearance;
            row(ui, &i.tr("look-radius"), "", |ui| {
                crate::widgets::segmented(
                    ui,
                    &mut a.corner_radius,
                    &[
                        (6, i.tr("radius-sharp")),
                        (14, i.tr("radius-normal")),
                        (20, i.tr("radius-round")),
                    ],
                );
            });
            divider(ui);
            row(
                ui,
                &i.tr("look-buttons"),
                &i.tr("look-buttons-hint"),
                |ui| {
                    crate::widgets::segmented(
                        ui,
                        &mut a.buttons,
                        &[
                            (ButtonStyle::Standard, i.tr("buttons-standard")),
                            (ButtonStyle::Liquid, i.tr("buttons-liquid")),
                        ],
                    );
                },
            );
            divider(ui);
            row(ui, &i.tr("look-animations"), "", |ui| {
                if crate::widgets::segmented(
                    ui,
                    &mut a.animations,
                    &[
                        (Animations::Full, i.tr("anim-full")),
                        (Animations::Reduced, i.tr("anim-reduced")),
                        (Animations::Off, i.tr("anim-off")),
                    ],
                ) {
                    d.general.reduce_motion = false;
                }
            });
            divider(ui);
            row(
                ui,
                &i.tr("look-compact"),
                &i.tr("look-compact-hint"),
                |ui| {
                    toggle(ui, &mut a.compact);
                },
            );
        });
        card(ui, Some(&i.tr("look-layout")), |ui| {
            let a = &mut d.appearance;
            row(
                ui,
                &i.tr("look-card-size"),
                &i.tr("look-card-size-hint"),
                |ui| {
                    ui.add(egui::Slider::new(&mut a.card_size, 160..=420).suffix(" px"));
                },
            );
            divider(ui);
            row(ui, &i.tr("look-show-badge"), "", |ui| {
                toggle(ui, &mut a.show_game_badge);
            });
            divider(ui);
            row(
                ui,
                &i.tr("look-show-details"),
                &i.tr("look-show-details-hint"),
                |ui| {
                    toggle(ui, &mut a.show_details);
                },
            );
            divider(ui);
            row(ui, &i.tr("look-start-page"), "", |ui| {
                crate::widgets::segmented(
                    ui,
                    &mut a.start_page,
                    &[
                        (StartPage::Library, i.tr("nav-library")),
                        (StartPage::Editor, i.tr("nav-editor")),
                    ],
                );
            });
            divider(ui);
            row(
                ui,
                &i.tr("settings-ui-scale"),
                &i.tr("look-scale-hint"),
                |ui| {
                    ui.add(egui::Slider::new(&mut d.general.ui_scale, 0.75..=2.0).step_by(0.05));
                },
            );
        });
        if ui.button(i.tr("look-reset")).clicked() {
            d.appearance = hc_core::config::Appearance::default();
            d.general.theme = Theme::Dark;
            d.general.ui_scale = 1.0;
            d.general.reduce_motion = false;
        }
    }
    fn capture(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let c = &mut self.draft.capture;
        card(ui, Some(&i.tr("cap-replay")), |ui| {
            row(
                ui,
                &i.tr("settings-replay-enabled"),
                &i.tr("settings-replay-hint"),
                |ui| {
                    toggle(ui, &mut c.replay_enabled);
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-replay-seconds"),
                &i.tr("settings-replay-seconds-hint"),
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut c.replay_seconds, REPLAY_MIN_SECS..=REPLAY_MAX_SECS)
                            .suffix(" s"),
                    );
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-buffer-storage"),
                &i.tr("settings-buffer-storage-hint"),
                |ui| {
                    egui::ComboBox::from_id_salt("buf")
                        .selected_text(i.tr(match c.buffer_storage {
                            BufferStorage::Auto => "storage-auto",
                            BufferStorage::Memory => "storage-memory",
                            BufferStorage::Disk => "storage-disk",
                        }))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut c.buffer_storage,
                                BufferStorage::Auto,
                                i.tr("storage-auto"),
                            );
                            ui.selectable_value(
                                &mut c.buffer_storage,
                                BufferStorage::Memory,
                                i.tr("storage-memory"),
                            );
                            ui.selectable_value(
                                &mut c.buffer_storage,
                                BufferStorage::Disk,
                                i.tr("storage-disk"),
                            );
                        });
                },
            );
        });
        card(ui, Some(&i.tr("cap-quality")), |ui| {
            row(ui, &i.tr("settings-fps"), "", |ui| {
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(format!("{} FPS", c.fps))
                    .show_ui(ui, |ui| {
                        for f in FPS_CHOICES {
                            ui.selectable_value(&mut c.fps, f, format!("{f} FPS"));
                        }
                    });
            });
            divider(ui);
            row(
                ui,
                &i.tr("profile-height"),
                &i.tr("cap-height-hint"),
                |ui| {
                    egui::ComboBox::from_id_salt("height")
                        .selected_text(if c.height == 0 {
                            i.tr("cap-height-source")
                        } else {
                            format!("{}p", c.height)
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut c.height, 0, i.tr("cap-height-source"));
                            for h in [720, 1080, 1440, 2160] {
                                ui.selectable_value(&mut c.height, h, format!("{h}p"));
                            }
                        });
                },
            );
            divider(ui);
            row(ui, &i.tr("settings-codec"), &i.tr("cap-codec-hint"), |ui| {
                ui.radio_value(&mut c.codec, Codec::Av1, "AV1");
                ui.radio_value(&mut c.codec, Codec::Hevc, "HEVC");
                ui.radio_value(&mut c.codec, Codec::H264, "H.264");
            });
            divider(ui);
            row(
                ui,
                &i.tr("settings-bitrate"),
                &i.tr("cap-bitrate-hint"),
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut c.bitrate_kbps)
                            .range(1_000..=200_000)
                            .speed(250)
                            .suffix(" kbps"),
                    );
                },
            );
            divider(ui);
            row(ui, &i.tr("settings-rate-control"), "", |ui| {
                if c.rate_control == RateControl::Cqp {
                    ui.add(egui::DragValue::new(&mut c.cqp).range(1..=51));
                }
                ui.radio_value(&mut c.rate_control, RateControl::Cqp, "CQP");
                ui.radio_value(&mut c.rate_control, RateControl::Cbr, "CBR");
                ui.radio_value(&mut c.rate_control, RateControl::Vbr, "VBR");
            });
        });
        card(ui, Some(&i.tr("cap-source")), |ui| {
            row(
                ui,
                &i.tr("settings-capture-mode"),
                &i.tr("cap-mode-hint"),
                |ui| {
                    ui.radio_value(&mut c.mode, CaptureMode::Region, i.tr("mode-region"));
                    ui.radio_value(&mut c.mode, CaptureMode::Window, i.tr("mode-window"));
                    ui.radio_value(&mut c.mode, CaptureMode::Monitor, i.tr("mode-monitor"));
                },
            );
            if c.mode == CaptureMode::Region {
                let r = c.region.get_or_insert(Region {
                    x: 0,
                    y: 0,
                    width: 1280,
                    height: 720,
                });
                row(ui, &i.tr("settings-region"), "", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut r.height)
                            .range(16..=16384)
                            .prefix("h "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut r.width)
                            .range(16..=16384)
                            .prefix("w "),
                    );
                    ui.add(egui::DragValue::new(&mut r.y).prefix("y "));
                    ui.add(egui::DragValue::new(&mut r.x).prefix("x "));
                });
            }
            divider(ui);
            row(ui, &i.tr("settings-monitor"), "", |ui| {
                ui.radio_value(
                    &mut c.monitor,
                    MonitorChoice::Primary,
                    i.tr("monitor-primary"),
                );
                ui.radio_value(&mut c.monitor, MonitorChoice::Game, i.tr("monitor-game"));
            });
            divider(ui);
            row(ui, &i.tr("settings-show-cursor"), "", |ui| {
                toggle(ui, &mut c.show_cursor);
            });
            divider(ui);
            row(
                ui,
                &i.tr("settings-sdr-white"),
                &i.tr("cap-sdr-hint"),
                |ui| {
                    ui.add(egui::Slider::new(&mut c.sdr_white_nits, 80..=1000).suffix(" nit"));
                },
            );
            divider(ui);
            row(ui, &i.tr("settings-screenshot-format"), "", |ui| {
                ui.radio_value(&mut c.screenshot_format, ScreenshotFormat::Webp, "WebP");
                ui.radio_value(&mut c.screenshot_format, ScreenshotFormat::Jpg, "JPG");
                ui.radio_value(&mut c.screenshot_format, ScreenshotFormat::Png, "PNG");
            });
        });
    }

    fn audio(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let a = &mut self.draft.audio;
        card(ui, None, |ui| {
            row(
                ui,
                &i.tr("settings-audio-system"),
                &i.tr("aud-system-hint"),
                |ui| {
                    ui.add_enabled(
                        a.system,
                        egui::Slider::new(&mut a.system_volume, 0..=200).suffix(" %"),
                    );
                    toggle(ui, &mut a.system);
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-audio-mic"),
                &i.tr("aud-mic-hint"),
                |ui| {
                    ui.add_enabled(
                        a.microphone,
                        egui::Slider::new(&mut a.microphone_volume, 0..=200).suffix(" %"),
                    );
                    toggle(ui, &mut a.microphone);
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-separate-tracks"),
                &i.tr("aud-tracks-hint"),
                |ui| {
                    toggle(ui, &mut a.separate_tracks);
                },
            );
        });
        card(ui, Some(&i.tr("aud-mic-processing")), |ui| {
            row(ui, &i.tr("settings-ptt"), &i.tr("aud-ptt-hint"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut a.push_to_talk).desired_width(140.0));
            });
            divider(ui);
            row(
                ui,
                &i.tr("settings-noise-gate"),
                &i.tr("aud-gate-hint"),
                |ui| {
                    ui.add_enabled(
                        a.noise_gate,
                        egui::Slider::new(&mut a.noise_gate_db, -90..=0).suffix(" dB"),
                    );
                    toggle(ui, &mut a.noise_gate);
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-noise-suppression"),
                &i.tr("aud-rnnoise-hint"),
                |ui| {
                    toggle(ui, &mut a.noise_suppression);
                },
            );
        });
    }

    fn overlay(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let d = &mut self.draft;
        card(ui, Some(&i.tr("settings-notifications")), |ui| {
            let n = &mut d.notifications;
            row(ui, &i.tr("settings-notify-enabled"), "", |ui| {
                toggle(ui, &mut n.enabled);
            });
            divider(ui);
            row(
                ui,
                &i.tr("settings-notify-in-game"),
                &i.tr("ov-in-game-hint"),
                |ui| {
                    toggle(ui, &mut n.in_game);
                },
            );
            divider(ui);
            row(ui, &i.tr("settings-notify-sound"), "", |ui| {
                toggle(ui, &mut n.sound);
            });
            divider(ui);
            row(ui, &i.tr("settings-position"), "", |ui| {
                position_combo(ui, i, "npos", &mut n.position)
            });
        });
        card(ui, Some(&i.tr("settings-webcam")), |ui| {
            let w = &mut d.webcam;
            row(
                ui,
                &i.tr("settings-webcam-enabled"),
                &i.tr("ov-webcam-hint"),
                |ui| {
                    toggle(ui, &mut w.enabled);
                },
            );
            ui.add_enabled_ui(w.enabled, |ui| {
                divider(ui);
                row(ui, &i.tr("settings-webcam-device"), "", |ui| {
                    let shown = w.device.clone().unwrap_or_else(|| i.tr("ov-first-camera"));
                    egui::ComboBox::from_id_salt("cam")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut w.device, None, i.tr("ov-first-camera"));
                            for name in &self.cameras {
                                ui.selectable_value(&mut w.device, Some(name.clone()), name);
                            }
                        });
                });
                divider(ui);
                row(ui, &i.tr("settings-webcam-size"), "", |ui| {
                    ui.add(egui::Slider::new(&mut w.size_percent, 5..=60).suffix(" %"));
                });
                divider(ui);
                row(ui, &i.tr("settings-position"), "", |ui| {
                    position_combo(ui, i, "wpos", &mut w.position)
                });
            });
        });
    }

    fn library(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let l = &mut self.draft.library;
        card(ui, Some(&i.tr("lib-space")), |ui| {
            row(
                ui,
                &i.tr("settings-auto-delete"),
                &i.tr("lib-auto-delete-hint"),
                |ui| {
                    toggle(ui, &mut l.auto_delete);
                },
            );
            divider(ui);
            row(ui, &i.tr("settings-quota"), "", |ui| {
                ui.add_enabled(
                    l.auto_delete,
                    egui::DragValue::new(&mut l.max_storage_gb)
                        .range(0.0..=100_000.0)
                        .speed(1.0)
                        .suffix(" GB"),
                );
            });
        });
        card(ui, Some(&i.tr("settings-extra-folders")), |ui| {
            ui.label(
                RichText::new(i.tr("lib-extra-hint"))
                    .color(palette(ui.ctx()).weak)
                    .size(12.5),
            );
            let mut text = l
                .extra_folders
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                l.extra_folders = text
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(std::path::PathBuf::from)
                    .collect();
            }
        });
    }

    fn hotkeys(&mut self, ui: &mut egui::Ui, i: &Localizer, problems: &[(&str, String)]) {
        let conflicts = &self.conflicts;
        let hk = &mut self.draft.hotkeys;
        card(ui, None, |ui| {
            let fields = [
                ("save-clip", &mut hk.save_clip),
                ("toggle-recording", &mut hk.toggle_recording),
                ("screenshot", &mut hk.screenshot),
                ("add-marker", &mut hk.add_marker),
                ("open-studio", &mut hk.open_studio),
            ];
            let n = fields.len();
            for (k, (id, field)) in fields.into_iter().enumerate() {
                let problem = problems
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, m)| m.clone());
                let conflict = conflicts
                    .iter()
                    .find(|c| c.action == id && c.binding == *field);
                let err = match (problem, conflict) {
                    (Some(m), _) if m.starts_with("dup:") => {
                        let other = i.tr(&format!("action-{}", &m[4..]));
                        i.tr_args("hotkey-duplicate", &[("other", &other)])
                    }
                    (Some(m), _) => i.tr_args("hotkey-invalid", &[("error", &m)]),
                    (None, Some(c)) => i.tr_args("hotkey-conflict", &[("reason", &c.reason)]),
                    _ => String::new(),
                };
                row(ui, &i.tr(&format!("action-{id}")), &err, |ui| {
                    // Right-to-left row: the "off" button first, then the key-cap.
                    if !field.is_empty()
                        && ui
                            .small_button(i.tr("key-clear"))
                            .on_hover_text(i.tr("key-clear-hint"))
                            .clicked()
                    {
                        field.clear();
                    }
                    // Click, then press the combination (Esc cancels).
                    if let crate::keycapture::Capture::Set(combo) =
                        crate::keycapture::key_capture(ui, id, None, field, &i.tr("key-press-now"))
                    {
                        *field = combo;
                    }
                });
                if k + 1 < n {
                    divider(ui);
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(i.tr("settings-hotkeys-hint"))
                    .color(palette(ui.ctx()).weak)
                    .size(12.5),
            );
        });
    }

    fn updates(&mut self, ui: &mut egui::Ui, i: &Localizer, paths: &AppPaths) {
        let u = &mut self.draft.updates;
        card(ui, None, |ui| {
            row(
                ui,
                &i.tr("settings-update-check"),
                &i.tr("upd-check-hint"),
                |ui| {
                    if u.check == UpdateCheck::EveryHours {
                        ui.add(
                            egui::DragValue::new(&mut u.interval_hours)
                                .range(1..=720)
                                .suffix(" h"),
                        );
                    }
                    egui::ComboBox::from_id_salt("updcheck")
                        .selected_text(i.tr(match u.check {
                            UpdateCheck::OnStartup => "update-on-startup",
                            UpdateCheck::EveryHours => "update-every-hours",
                            UpdateCheck::Never => "update-never",
                        }))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut u.check,
                                UpdateCheck::OnStartup,
                                i.tr("update-on-startup"),
                            );
                            ui.selectable_value(
                                &mut u.check,
                                UpdateCheck::EveryHours,
                                i.tr("update-every-hours"),
                            );
                            ui.selectable_value(
                                &mut u.check,
                                UpdateCheck::Never,
                                i.tr("update-never"),
                            );
                        });
                },
            );
            divider(ui);
            row(
                ui,
                &i.tr("settings-update-channel"),
                &i.tr("upd-channel-hint"),
                |ui| {
                    ui.radio_value(&mut u.channel, UpdateChannel::Beta, i.tr("channel-beta"));
                    ui.radio_value(
                        &mut u.channel,
                        UpdateChannel::Stable,
                        i.tr("channel-stable"),
                    );
                },
            );
        });
        let saved_cfg = self.saved.clone();
        card(ui, Some(&i.tr("upd-title")), |ui| {
            if let Some(crate::updates_ui::UpdateAction::Ignore(v)) =
                self.update_ui.ui(ui, i, paths, &saved_cfg)
            {
                self.draft.updates.ignored_versions.push(v);
            }
        });
    }
}

/// Launcher-style choice button: a small preview, the label, selected state.
fn choice(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    preview: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    let p = palette(ui.ctx());
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 56.0), egui::Sense::click());
    let (fill, stroke) = if on {
        (p.accent_soft(), Stroke::new(1.5, p.accent_line))
    } else if resp.hovered() {
        (p.sidebar, Stroke::new(1.0, p.accent_line))
    } else {
        (p.sidebar, Stroke::new(1.0, p.line_strong))
    };
    ui.painter()
        .rect(rect, p.radius_sm(), fill, stroke, egui::StrokeKind::Inside);
    let thumb =
        egui::Rect::from_center_size(rect.left_center() + vec2(26.0, 0.0), vec2(30.0, 30.0));
    preview(ui.painter(), thumb);
    ui.painter().text(
        rect.left_center() + vec2(52.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        p.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn position_combo(ui: &mut egui::Ui, i: &Localizer, id: &str, p: &mut OverlayPosition) {
    let opts = [
        (OverlayPosition::TopLeft, "pos-top-left"),
        (OverlayPosition::TopRight, "pos-top-right"),
        (OverlayPosition::BottomLeft, "pos-bottom-left"),
        (OverlayPosition::BottomRight, "pos-bottom-right"),
    ];
    let cur = opts.iter().find(|(v, _)| v == p).map_or("", |(_, k)| k);
    egui::ComboBox::from_id_salt(id)
        .selected_text(i.tr(cur))
        .show_ui(ui, |ui| {
            for (v, key) in opts {
                ui.selectable_value(p, v, i.tr(key));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkeys_are_valid() {
        assert!(hotkey_problems(&Hotkeys::default()).is_empty());
    }

    #[test]
    fn detects_invalid_and_duplicate() {
        let hk = Hotkeys {
            save_clip: "F8".into(),
            toggle_recording: "Ctrl+Nope".into(),
            screenshot: "f8".into(),
            ..Hotkeys::default()
        };
        let p = hotkey_problems(&hk);
        assert!(p.iter().any(|(id, _)| *id == "toggle-recording"));
        assert!(
            p.iter()
                .any(|(id, m)| *id == "screenshot" && m == "dup:save-clip")
        );
    }

    #[test]
    fn every_category_has_a_distinct_key() {
        let mut keys: Vec<_> = Category::ALL.iter().map(|c| c.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Category::ALL.len());
    }
}
