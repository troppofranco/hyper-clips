//! Library page: gallery with thumbnails, search/filters/sort/grouping and a
//! detail panel with player, markers, metadata editing and file actions.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, ColorImage, RichText, Sense, TextureHandle, TextureOptions, Vec2,
};
use hc_core::config::Config;
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_library::{
    Filter, Item, Kind, Library, ScanReport, Sort, fsops, group_by_game, group_by_session,
};

use crate::player::Player;

const MAX_ITEMS: usize = 2000;
/// Inner padding of a library card (thumbnail inset).
const CARD_PAD: f32 = 8.0;
const THUMB_CACHE: usize = 300;
const THUMB_LOADS_PER_FRAME: usize = 12;
const RELOAD_EVERY: Duration = Duration::from_secs(3);
const SESSION_GAP_SECS: i64 = 45 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    Date,
    None,
    Game,
    Session,
}

/// Quick filter pills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chip {
    All,
    Starred,
    Clips,
    Recordings,
    Screenshots,
}

struct Detail {
    id: i64,
    title: String,
    notes: String,
    tags: String,
    markers: Vec<f64>,
    player: Option<Player>,
    player_error: bool,
    frame: Option<TextureHandle>,
    volume: f64,
    confirm_delete: bool,
    /// Highlight detection running in the background.
    highlights: Option<Receiver<Result<Vec<f64>, String>>>,
}

pub struct LibraryPage {
    lib: Option<Library>,
    error: Option<String>,
    filter: Filter,
    sort: Sort,
    grouping: Grouping,
    items: Vec<Item>,
    games: Vec<(String, usize)>,
    tags: Vec<(String, usize)>,
    total_size: u64,
    dirty: bool,
    last_reload: Instant,
    /// Thumbnail textures: (texture, last use tick, when it was loaded).
    thumbs: HashMap<i64, (TextureHandle, u64, Instant, std::path::PathBuf)>,
    /// When the current set of cards appeared (cascade entrance).
    appear: Instant,
    shown_ids: Vec<i64>,
    tick: u64,
    detail: Option<Detail>,
    scan: Option<Receiver<Result<ScanReport, String>>>,
    toast: Option<(String, Instant)>,
    edit_request: Option<(std::path::PathBuf, String)>,
    vault_request: Option<(i64, std::path::PathBuf)>,
    share: Option<crate::share_ui::ShareWindow>,
    highlights_only: bool,
    chip: Chip,
    total_count: usize,
    settings_request: bool,
    rescan_request: bool,
    paths: AppPaths,
}

impl LibraryPage {
    pub fn new(paths: &AppPaths, config: &Config) -> Self {
        let (lib, error) = match Library::open(&paths.library_db()) {
            Ok(l) => (Some(l), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut page = Self {
            lib,
            error,
            filter: Filter::default(),
            sort: Sort::Newest,
            grouping: Grouping::Date,
            items: Vec::new(),
            games: Vec::new(),
            tags: Vec::new(),
            total_size: 0,
            dirty: true,
            last_reload: Instant::now(),
            thumbs: HashMap::new(),
            appear: Instant::now(),
            shown_ids: Vec::new(),
            tick: 0,
            detail: None,
            scan: None,
            toast: None,
            edit_request: None,
            vault_request: None,
            share: None,
            highlights_only: false,
            chip: Chip::All,
            total_count: 0,
            settings_request: false,
            rescan_request: false,
            paths: paths.clone(),
        };
        page.start_scan(paths, config);
        page
    }

    /// Reconcile the index with the clip folders in the background.
    pub fn start_scan(&mut self, paths: &AppPaths, config: &Config) {
        let mut roots = vec![
            config
                .general
                .clips_dir
                .clone()
                .unwrap_or_else(|| paths.default_clips_dir.clone()),
        ];
        roots.extend(config.library.extra_folders.iter().cloned());
        let db = paths.library_db();
        let thumbs = paths.thumbs_dir();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("library-scan".into())
            .spawn(move || {
                let res = Library::open(&db)
                    .and_then(|lib| hc_library::scan_folders(&lib, &roots, &thumbs))
                    .map_err(|e| e.to_string());
                let _ = tx.send(res);
            });
        if spawned.is_ok() {
            self.scan = Some(rx);
        }
    }

    /// A library item the user asked to move into the encrypted vault.
    pub fn take_vault_request(&mut self) -> Option<(i64, std::path::PathBuf)> {
        self.vault_request.take()
    }

    /// The player must release a file before it is moved or deleted.
    pub fn close_detail(&mut self) {
        self.detail = None;
        self.dirty = true;
    }

    /// (items in the library, bytes they use) for the sidebar.
    pub fn stats(&self) -> (usize, u64) {
        (self.total_count, self.total_size)
    }

    /// The user asked for the library settings (e.g. to enable highlights).
    pub fn take_settings_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_request)
    }

    /// Search field for the top bar, with the filter menu on its right.
    pub fn search_box(&mut self, ui: &mut egui::Ui, i: &Localizer, width: f32) {
        let p = crate::theme::palette(ui.ctx());
        let h = if p.compact { 30.0 } else { 36.0 };
        let mut focused = false;
        let frame = egui::Frame::new()
            .fill(p.card_hover)
            .stroke(egui::Stroke::new(1.0, p.line_strong))
            .corner_radius(h / 2.0)
            .inner_margin(egui::Margin::symmetric(16, 0))
            .show(ui, |ui| {
                ui.set_height(h);
                ui.horizontal_centered(|ui| {
                    // Text vertically centred in the pill; width follows the bar.
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.filter.text)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .align(egui::Align2::LEFT_CENTER)
                            .min_size(Vec2::new(0.0, h))
                            .hint_text(i.tr("lib-search"))
                            .desired_width((width - 32.0 - 60.0).max(120.0)),
                    );
                    focused = r.has_focus();
                    if r.changed() {
                        self.dirty = true;
                    }
                    crate::icons::icon(ui, crate::icons::Ic::Search, 15.0, p.weak);
                    let active = self.chip != Chip::All
                        || self.filter.game.is_some()
                        || self.highlights_only
                        || self.filter.tag.is_some();
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                    if resp.hovered() || active {
                        ui.painter().circle_filled(
                            rect.center(),
                            12.0,
                            if active { p.accent_soft() } else { p.card },
                        );
                    }
                    crate::icons::paint(
                        ui.painter(),
                        rect.shrink(5.0),
                        crate::icons::Ic::Filter,
                        if active { p.accent_line } else { p.weak },
                    );
                    let resp = resp
                        .on_hover_text(i.tr("lib-view-options"))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&resp).show(|ui| self.filter_menu(ui, i));
                });
            });
        // Accent ring while typing (launcher: focus outline).
        let t = crate::anim::flag(ui.ctx(), ui.scope_id().with("search-focus"), focused, 0.18);
        if t > 0.0 {
            ui.painter().rect_stroke(
                frame.response.rect,
                h / 2.0,
                egui::Stroke::new(1.5, crate::anim::mix(p.line_strong, p.accent_line, t)),
                egui::StrokeKind::Inside,
            );
        }
    }
    /// A clip the user asked to open in the editor.
    pub fn take_edit_request(&mut self) -> Option<(std::path::PathBuf, String)> {
        self.edit_request.take()
    }

    fn reload(&mut self) {
        let Some(lib) = &self.lib else { return };
        match lib.list(&self.filter, self.sort, MAX_ITEMS, 0) {
            Ok(items) => {
                // A different set of cards (filter, search, sort) cascades in again.
                let ids: Vec<i64> = items.iter().map(|it| it.id).collect();
                if ids != self.shown_ids {
                    let added_only = ids.len() > self.shown_ids.len()
                        && self.shown_ids.iter().all(|id| ids.contains(id));
                    if !added_only {
                        self.appear = Instant::now();
                    }
                    self.shown_ids = ids;
                }
                self.items = items;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.games = lib.games().unwrap_or_default();
        self.tags = lib.tags().unwrap_or_default();
        self.total_size = lib.total_size().unwrap_or(0);
        self.total_count = lib.count(&Filter::default()).unwrap_or(0);
        self.dirty = false;
        self.last_reload = Instant::now();
    }

    fn thumb(
        &mut self,
        ctx: &egui::Context,
        item: &Item,
        budget: &mut usize,
    ) -> Option<(TextureHandle, Instant)> {
        self.tick += 1;
        // A regenerated thumbnail has a new path: drop the stale texture.
        if self
            .thumbs
            .get(&item.id)
            .is_some_and(|(_, _, _, p)| item.thumb.as_ref() != Some(p))
        {
            self.thumbs.remove(&item.id);
        }
        if let Some((tex, used, at, _)) = self.thumbs.get_mut(&item.id) {
            *used = self.tick;
            return Some((tex.clone(), *at));
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let path = item.thumb.as_ref()?;
        let (w, h, bgra) = hc_capture::screenshot::load_image(path).ok()?;
        let tex = ctx.load_texture(
            format!("thumb-{}", item.id),
            bgra_image(w, h, &bgra),
            TextureOptions::LINEAR,
        );
        if self.thumbs.len() >= THUMB_CACHE {
            // Evict the least recently drawn thumbnail.
            if let Some((&old, _)) = self.thumbs.iter().min_by_key(|(_, (_, u, _, _))| *u) {
                self.thumbs.remove(&old);
            }
        }
        let now = Instant::now();
        self.thumbs
            .insert(item.id, (tex.clone(), self.tick, now, path.clone()));
        Some((tex, now))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, i: &Localizer, config: &Config) {
        if std::mem::take(&mut self.rescan_request) && self.scan.is_none() {
            let paths = self.paths.clone();
            self.start_scan(&paths, config);
        }
        if let Some(rx) = &self.scan
            && let Ok(res) = rx.try_recv()
        {
            match res {
                Ok(r) => tracing::info!(added = r.added, removed = r.removed, "library scan done"),
                Err(e) => tracing::warn!(error = %e, "library scan failed"),
            }
            self.scan = None;
            self.dirty = true;
        }
        // New clips saved by the core appear without a manual refresh.
        if self.dirty || self.last_reload.elapsed() > RELOAD_EVERY {
            self.reload();
        }
        ui.ctx().request_repaint_after(RELOAD_EVERY);

        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }

        {
            // The detail panel slides in from the right.
            let mut open = self.detail.is_some();
            let p = crate::theme::palette(ui.ctx());
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(460.0)
                .frame(
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1.0, p.border))
                        .corner_radius(p.radius())
                        .inner_margin(egui::Margin::same(16)),
                )
                .show_collapsible(ui, &mut open, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.detail_ui(ui, i, config));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if self.items.is_empty() {
                    self.empty_state(ui, i, config);
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.gallery(ui, i, config));
            });
        if let Some(w) = self.share.as_mut()
            && !w.ui(ui.ctx(), i)
        {
            self.share = None; // dropping the share stops the server
        }
        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(3) {
                let t = crate::anim::toast(ui.ctx(), *at, 3.0);
                egui::Area::new(ui.scope_id().with("lib-toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0 + 14.0 * (1.0 - t)])
                    .show(ui.ctx(), |ui| {
                        ui.multiply_opacity(t);
                        egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg));
                    });
            } else {
                self.toast = None;
            }
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, i: &Localizer, config: &Config) {
        let p = crate::theme::palette(ui.ctx());
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            let icon = if self.highlights_only {
                crate::icons::Ic::Highlight
            } else {
                crate::icons::Ic::Logo
            };
            crate::icons::icon(ui, icon, 58.0, p.accent_line);
            ui.add_space(14.0);
            let filtered = !self.filter.text.is_empty()
                || self.filter.game.is_some()
                || self.chip != Chip::All
                || self.filter.tag.is_some();
            let (title, body) = if self.highlights_only {
                let body = if config.highlights.auto_detect {
                    i.tr("empty-hl-body")
                } else {
                    i.tr("hl-auto-off")
                };
                (i.tr("empty-hl-title"), body)
            } else if filtered {
                (i.tr("empty-filtered-title"), i.tr("empty-filtered-body"))
            } else {
                (
                    i.tr("empty-title"),
                    i.tr_args(
                        "empty-body",
                        &[
                            ("key", &config.hotkeys.save_clip),
                            ("seconds", &config.capture.replay_seconds.to_string()),
                        ],
                    ),
                )
            };
            ui.label(RichText::new(title).size(22.0).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(body).color(p.weak));
            ui.add_space(16.0);
            if self.highlights_only && !config.highlights.auto_detect {
                if crate::widgets::primary_button(ui, &i.tr("hl-open-settings")).clicked() {
                    self.settings_request = true;
                }
            } else if filtered || self.highlights_only {
                if crate::widgets::primary_button(ui, &i.tr("empty-clear-filters")).clicked() {
                    self.clear_filters();
                }
            } else {
                ui.label(
                    RichText::new(i.tr_args(
                        "empty-tips",
                        &[
                            ("record", &config.hotkeys.toggle_recording),
                            ("shot", &config.hotkeys.screenshot),
                        ],
                    ))
                    .color(p.faint)
                    .size(12.5),
                );
            }
            if self.scan.is_some() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(i.tr("lib-scanning"));
                });
            }
        });
    }

    fn clear_filters(&mut self) {
        self.filter.text.clear();
        self.filter.game = None;
        self.filter.tag = None;
        self.highlights_only = false;
        self.set_chip(Chip::All);
    }

    fn set_chip(&mut self, chip: Chip) {
        self.chip = chip;
        self.filter.favorites_only = chip == Chip::Starred;
        self.filter.kind = match chip {
            Chip::Clips => Some(Kind::Clip),
            Chip::Recordings => Some(Kind::Recording),
            Chip::Screenshots => Some(Kind::Screenshot),
            Chip::All | Chip::Starred => None,
        };
        self.dirty = true;
    }

    /// Show, game, highlights, tags, sort and grouping (filter menu).
    fn filter_menu(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let before = (
            self.filter.clone(),
            self.sort,
            self.grouping,
            self.highlights_only,
        );
        ui.set_min_width(240.0);
        crate::widgets::label_caps(ui, &i.tr("lib-show"));
        for (chip, key) in [
            (Chip::All, "chip-all"),
            (Chip::Starred, "chip-starred"),
            (Chip::Clips, "chip-clips"),
            (Chip::Recordings, "chip-recordings"),
            (Chip::Screenshots, "chip-screenshots"),
        ] {
            if ui.radio(self.chip == chip, i.tr(key)).clicked() {
                self.set_chip(chip);
            }
        }
        ui.checkbox(&mut self.highlights_only, i.tr("lib-only-highlights"));
        ui.separator();
        crate::widgets::label_caps(ui, &i.tr("chip-by-game"));
        if ui
            .radio(self.filter.game.is_none(), i.tr("lib-all-games"))
            .clicked()
        {
            self.filter.game = None;
        }
        for (g, n) in &self.games {
            if ui
                .radio(self.filter.game.as_ref() == Some(g), format!("{g}  ({n})"))
                .clicked()
            {
                self.filter.game = Some(g.clone());
            }
        }
        if !self.highlights_only && !self.tags.is_empty() {
            ui.separator();
            crate::widgets::label_caps(ui, &i.tr("lib-tags"));
            if ui
                .radio(self.filter.tag.is_none(), i.tr("lib-all-tags"))
                .clicked()
            {
                self.filter.tag = None;
            }
            for (t, n) in &self.tags {
                if ui
                    .radio(self.filter.tag.as_ref() == Some(t), format!("{t}  ({n})"))
                    .clicked()
                {
                    self.filter.tag = Some(t.clone());
                }
            }
        }
        ui.separator();
        crate::widgets::label_caps(ui, &i.tr("lib-sort"));
        for (s, l) in [
            (Sort::Newest, "sort-newest"),
            (Sort::Oldest, "sort-oldest"),
            (Sort::Longest, "sort-longest"),
            (Sort::Largest, "sort-largest"),
            (Sort::Title, "sort-title"),
        ] {
            ui.radio_value(&mut self.sort, s, i.tr(l));
        }
        ui.separator();
        crate::widgets::label_caps(ui, &i.tr("lib-group"));
        for (g, l) in [
            (Grouping::Date, "group-date"),
            (Grouping::Game, "group-game"),
            (Grouping::Session, "group-session"),
            (Grouping::None, "group-none"),
        ] {
            ui.radio_value(&mut self.grouping, g, i.tr(l));
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(i.tr("empty-clear-filters")).clicked() {
                self.clear_filters();
            }
            if ui.button(i.tr("lib-rescan")).clicked() {
                self.rescan_request = true;
            }
        });
        if (
            self.filter.clone(),
            self.sort,
            self.grouping,
            self.highlights_only,
        ) != before
        {
            self.filter.tag = if self.highlights_only {
                Some(hc_library::HIGHLIGHT_TAG.to_owned())
            } else if self.filter.tag.as_deref() == Some(hc_library::HIGHLIGHT_TAG) && before.3 {
                None
            } else {
                self.filter.tag.clone()
            };
            self.dirty = true;
        }
    }
    /// Group indices by local calendar day buckets (today, yesterday, ...).
    fn group_by_date(&self, i: &Localizer) -> Vec<hc_library::Group> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let day = |t: i64| {
            hc_core::sys::format_local(t)
                .get(..10)
                .unwrap_or("")
                .to_owned()
        };
        let today = day(now);
        let yesterday = day(now - 86_400);
        let bucket = |t: i64| -> usize {
            let d = day(t);
            if d == today {
                0
            } else if d == yesterday {
                1
            } else if now - t < 7 * 86_400 {
                2
            } else if now - t < 31 * 86_400 {
                3
            } else {
                4
            }
        };
        let keys = [
            "group-today",
            "group-yesterday",
            "group-week",
            "group-month",
            "group-older",
        ];
        let mut groups: Vec<hc_library::Group> = keys
            .iter()
            .map(|k| hc_library::Group {
                label: i.tr(k),
                items: Vec::new(),
            })
            .collect();
        for (idx, it) in self.items.iter().enumerate() {
            groups[bucket(it.created_at)].items.push(idx);
        }
        groups.retain(|g| !g.items.is_empty());
        groups
    }

    fn gallery(&mut self, ui: &mut egui::Ui, i: &Localizer, config: &Config) {
        let p = crate::theme::palette(ui.ctx());
        let groups = match self.grouping {
            Grouping::Date if self.sort == Sort::Newest || self.sort == Sort::Oldest => {
                self.group_by_date(i)
            }
            Grouping::Game => group_by_game(&self.items),
            Grouping::Session => group_by_session(&self.items, SESSION_GAP_SECS),
            _ => vec![hc_library::Group {
                label: String::new(),
                items: (0..self.items.len()).collect(),
            }],
        };
        let gap = if p.compact { 10.0 } else { 16.0 };
        let avail = ui.available_width() - 4.0;
        let target = config.appearance.card_size as f32;
        let cols = ((avail + gap) / (target + gap)).floor().max(1.0);
        // Fill the row, but never grow far past the chosen size (narrow panes).
        let card_w = ((avail - gap * (cols - 1.0)) / cols)
            .floor()
            .min(target * 1.25);
        // Thumbnail inset by CARD_PAD on every side, then the text lines.
        let thumb_h = ((card_w - 2.0 * CARD_PAD) * 9.0 / 16.0).round();
        let lines = 1.0
            + f32::from(u8::from(config.appearance.show_game_badge))
            + f32::from(u8::from(config.appearance.show_details));
        let card_h = CARD_PAD + thumb_h + 10.0 + lines * 20.0 + CARD_PAD;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut budget = THUMB_LOADS_PER_FRAME;
        let rich = crate::anim::rich(ui.ctx());
        let mut order = 0usize;
        let mut clicked = None;
        let mut fav_toggle = None;
        for g in groups {
            if !g.label.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let header = match self.grouping {
                        Grouping::Session => {
                            let first = g
                                .items
                                .first()
                                .map(|&k| self.items[k].created_at)
                                .unwrap_or(0);
                            format!("{} · {}", g.label, hc_core::sys::format_local(first))
                        }
                        _ => g.label.clone(),
                    };
                    ui.label(RichText::new(header).size(19.0).strong());
                    ui.label(
                        RichText::new(
                            i.tr_args("group-count", &[("count", &g.items.len().to_string())]),
                        )
                        .color(p.weak)
                        .size(12.5),
                    );
                });
                ui.add_space(4.0);
            }
            for row in g.items.chunks(cols as usize) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
                    for &idx in row {
                        let item = self.items[idx].clone();
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(card_w, card_h), Sense::hover());
                        // Only decode thumbnails for cards that are on screen.
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        // Cascade: each visible card starts a little later (max 12 steps).
                        let delay = if rich {
                            order.min(12) as f32 * 0.045
                        } else {
                            0.0
                        };
                        order += 1;
                        let appear = crate::anim::enter(ui.ctx(), self.appear, delay, 0.45);
                        let thumb = self.thumb(ui.ctx(), &item, &mut budget);
                        let thumb_t = thumb
                            .as_ref()
                            .map_or(0.0, |(_, at)| crate::anim::enter(ui.ctx(), *at, 0.0, 0.3));
                        let loading = thumb.is_none() && item.thumb.is_some();
                        let selected = self.detail.as_ref().is_some_and(|d| d.id == item.id);
                        let meta = card_meta(i, &item, now);
                        let (card_clicked, star_clicked) = card_ui(
                            ui,
                            rect,
                            thumb_h,
                            &item,
                            thumb.as_ref().map(|(t, _)| t),
                            CardAnim {
                                appear,
                                thumb: thumb_t,
                                loading,
                            },
                            selected,
                            &config.appearance,
                            &meta,
                        );
                        if card_clicked {
                            clicked = Some(item.id);
                        }
                        if star_clicked {
                            fav_toggle = Some((item.id, !item.favorite));
                        }
                    }
                });
                ui.add_space(gap - ui.spacing().item_spacing.y);
            }
            ui.add_space(8.0);
        }
        if budget == 0 {
            ui.ctx().request_repaint();
        }
        if let Some((id, fav)) = fav_toggle
            && let Some(lib) = &self.lib
        {
            let _ = lib.set_favorite(id, fav);
            self.dirty = true;
        }
        if let Some(id) = clicked {
            self.open_detail(id);
        }
    }
    fn open_detail(&mut self, id: i64) {
        let Some(lib) = &self.lib else { return };
        let Ok(Some(item)) = lib.get(id) else { return };
        let player = match item.kind {
            Kind::Screenshot => None,
            _ => Player::open(&item.path, 960, 540)
                .inspect_err(|e| tracing::warn!(error = %e, "player failed"))
                .ok(),
        };
        let player_error = item.kind != Kind::Screenshot && player.is_none();
        self.detail = Some(Detail {
            id,
            title: item.title.clone(),
            notes: item.notes.clone(),
            tags: item.tags.join(", "),
            markers: lib.markers(id).unwrap_or_default(),
            player,
            player_error,
            frame: None,
            volume: 1.0,
            confirm_delete: false,
            highlights: None,
        });
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui, i: &Localizer, config: &Config) {
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        let Some(item) = self
            .items
            .iter()
            .find(|it| it.id == d.id)
            .cloned()
            .or_else(|| self.lib.as_ref().and_then(|l| l.get(d.id).ok().flatten()))
        else {
            self.detail = None;
            return;
        };
        let mut close = false;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.heading(&item.title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(i.tr("lib-close")).clicked() {
                    close = true;
                }
            });
        });
        ui.small(format!(
            "{} · {} · {}x{} · {}",
            item.game.clone().unwrap_or_default(),
            hc_core::sys::format_local(item.created_at),
            item.width,
            item.height,
            hc_core::sys::format_bytes(item.size_bytes)
        ));

        // ---- Media view
        let avail = ui.available_width();
        if let Some(p) = d.player.as_mut() {
            if p.error().is_some() {
                d.player_error = true;
            }
            if let Some(f) = p.poll_frame() {
                let img = bgra_image(f.width, f.height, &f.bgra);
                match &mut d.frame {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        d.frame = Some(ui.ctx().load_texture("player", img, TextureOptions::LINEAR))
                    }
                }
            }
            // Repaint for new frames only while really playing.
            if !p.is_paused() && !p.ended() && p.error().is_none() {
                ui.ctx().request_repaint_after(Duration::from_millis(15));
            }
        }
        if item.kind == Kind::Screenshot
            && d.frame.is_none()
            && let Ok((w, h, bgra)) = hc_capture::screenshot::load_image(&item.path)
        {
            let (w, h, small) = hc_capture::screenshot::downscale(w, h, w * 4, &bgra, 1280, 720);
            d.frame = Some(ui.ctx().load_texture(
                "player",
                bgra_image(w, h, &small),
                TextureOptions::LINEAR,
            ));
        }
        if let Some(tex) = &d.frame {
            let size = tex.size_vec2();
            let scale = (avail / size.x).min(1.0);
            ui.image((tex.id(), size * scale));
        } else if d.player_error {
            ui.colored_label(ui.visuals().error_fg_color, i.tr("lib-player-error"));
        }

        // ---- Transport with markers
        if let Some(p) = d.player.as_mut() {
            let dur = p.duration();
            let mut pos = p.position();
            ui.horizontal(|ui| {
                let icon = if p.is_paused() || p.ended() {
                    Icon::Play
                } else {
                    Icon::Pause
                };
                if icon_button(ui, icon, 26.0).clicked() {
                    p.toggle();
                }
                ui.label(format!("{} / {}", fmt_time(pos), fmt_time(dur)));
                if ui
                    .add(egui::Slider::new(&mut d.volume, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    p.set_volume(d.volume);
                }
            });
            ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(50.0);
            let resp = ui.add(egui::Slider::new(&mut pos, 0.0..=dur.max(0.001)).show_value(false));
            if resp.changed() {
                p.seek(pos);
            }
            // Marker ticks on the timeline.
            if dur > 0.0 {
                let r = resp.rect;
                for m in &d.markers {
                    let x = r.left() + (*m / dur) as f32 * r.width();
                    ui.painter().line_segment(
                        [egui::pos2(x, r.top()), egui::pos2(x, r.bottom())],
                        egui::Stroke::new(2.0, Color32::from_rgb(250, 204, 21)),
                    );
                }
            }
            if ui.input(|inp| inp.key_pressed(egui::Key::Space))
                && !ui.ctx().egui_wants_keyboard_input()
            {
                p.toggle();
            }
        }
        if let Some(rx) = &d.highlights {
            match rx.try_recv() {
                Ok(Ok(found)) => {
                    d.highlights = None;
                    if let Some(l) = self.lib.as_mut() {
                        let added = l.add_highlights(d.id, &found).unwrap_or(0);
                        d.markers = l.markers(d.id).unwrap_or_default();
                        if let Some(it) = l.get(d.id).ok().flatten() {
                            d.tags = it.tags.join(", ");
                        }
                        self.toast = Some((
                            i.tr_args("lib-highlights-found", &[("count", &added.to_string())]),
                            Instant::now(),
                        ));
                        self.dirty = true;
                    }
                }
                Ok(Err(e)) => {
                    d.highlights = None;
                    self.toast = Some((e, Instant::now()));
                }
                Err(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(i.tr("lib-highlights-running"));
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        if !d.markers.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(i.tr("lib-markers"));
                for m in d.markers.clone() {
                    if ui.small_button(fmt_time(m)).clicked()
                        && let Some(p) = d.player.as_mut()
                    {
                        p.seek((m - 2.0).max(0.0));
                    }
                }
            });
        }
        ui.separator();

        // ---- Metadata
        let lib = self.lib.as_mut();
        egui::Grid::new("meta")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label(i.tr("lib-title"));
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut d.title);
                    if ui.button(i.tr("lib-rename")).clicked()
                        && let Some(l) = &lib
                        && d.title.trim() != item.title
                    {
                        if let Err(e) = l.rename(d.id, &d.title) {
                            tracing::warn!(error = %e, "rename failed");
                        }
                        changed = true;
                    }
                });
                ui.end_row();
                ui.label(i.tr("lib-favorite"));
                let mut fav = item.favorite;
                if ui.checkbox(&mut fav, "").changed()
                    && let Some(l) = &lib
                {
                    let _ = l.set_favorite(d.id, fav);
                    changed = true;
                }
                ui.end_row();
                ui.label(i.tr("lib-tags"));
                if ui.text_edit_singleline(&mut d.tags).lost_focus()
                    && let Some(l) = lib
                {
                    let tags: Vec<String> =
                        d.tags.split(',').map(|t| t.trim().to_owned()).collect();
                    let _ = l.set_tags(d.id, &tags);
                    changed = true;
                }
                ui.end_row();
            });
        ui.label(i.tr("lib-notes"));
        if ui
            .add(
                egui::TextEdit::multiline(&mut d.notes)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .lost_focus()
            && let Some(l) = &self.lib
        {
            let _ = l.set_notes(d.id, &d.notes);
            changed = true;
        }
        ui.separator();

        // ---- Actions
        let mut toast = None;
        let mut deleted = false;
        ui.horizontal_wrapped(|ui| {
            if item.kind != Kind::Screenshot && ui.button(i.tr("lib-edit")).clicked() {
                if let Some(p) = &d.player {
                    p.pause();
                }
                self.edit_request = Some((item.path.clone(), item.title.clone()));
            }
            if item.kind != Kind::Screenshot
                && d.highlights.is_none()
                && ui.button(i.tr("lib-find-highlights")).clicked()
            {
                let (tx, rx) = channel();
                let (path, dur) = (item.path.clone(), item.duration_ms);
                let params = hc_editor::highlight::Params {
                    margin_db: config.highlights.margin_db as f32,
                    ..Default::default()
                };
                let _ = std::thread::Builder::new()
                    .name("highlights".into())
                    .spawn(move || {
                        hc_encode::mf::init_thread();
                        let r = hc_editor::highlight::detect_file(&path, dur, &params)
                            .map(|v| v.iter().map(|h| h.at_secs).collect())
                            .map_err(|e| e.to_string());
                        let _ = tx.send(r);
                    });
                d.highlights = Some(rx);
            }
            if ui.button(i.tr("lib-share-lan")).clicked() {
                match crate::share_ui::ShareWindow::start(&item.path, &item.title, &config.share) {
                    Ok(w) => self.share = Some(w),
                    Err(e) => toast = Some(e),
                }
            }
            if ui.button(i.tr("lib-move-to-vault")).clicked() {
                if let Some(p) = &d.player {
                    p.pause();
                }
                self.vault_request = Some((item.id, item.path.clone()));
            }
            if ui.button(i.tr("lib-open-folder")).clicked() {
                let _ = fsops::reveal_in_explorer(&item.path);
            }
            if ui.button(i.tr("lib-copy")).clicked()
                && fsops::copy_files_to_clipboard(std::slice::from_ref(&item.path)).is_ok()
            {
                toast = Some(i.tr("lib-copied"));
            }
            if ui.button(i.tr("lib-open")).clicked() {
                if let Some(p) = &d.player {
                    p.pause();
                }
                let _ = fsops::open_default(&item.path);
            }
            if d.confirm_delete {
                ui.label(RichText::new(i.tr("lib-delete-confirm")).strong());
                if ui.button(i.tr("lib-yes")).clicked() {
                    d.player = None; // release the file before deleting it
                    if let Some(l) = &self.lib {
                        match l.delete(d.id, false) {
                            Ok(()) => deleted = true,
                            Err(e) => tracing::warn!(error = %e, "delete failed"),
                        }
                    }
                }
                if ui.button(i.tr("lib-no")).clicked() {
                    d.confirm_delete = false;
                }
            } else if ui.button(i.tr("lib-delete")).clicked() {
                d.confirm_delete = true;
            }
        });
        if let Some(t) = toast {
            self.toast = Some((t, Instant::now()));
        }
        if changed || deleted {
            self.dirty = true;
        }
        if close || deleted {
            if let Some(id) = self.detail.as_ref().map(|d| d.id) {
                self.thumbs.remove(&id);
            }
            self.detail = None;
        }
    }
}

/// Player transport icons.
#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
}

pub fn paint_icon(p: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.45;
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x - r * 0.6, c.y - r),
                    egui::pos2(c.x + r, c.y),
                    egui::pos2(c.x - r * 0.6, c.y + r),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            let w = r * 0.45;
            for x in [c.x - r * 0.55, c.x + r * 0.1] {
                p.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x, c.y - r), Vec2::new(w, 2.0 * r)),
                    1.0,
                    color,
                );
            }
        }
    }
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let visuals = ui.style().interact(&resp);
    ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
    let color = visuals.fg_stroke.color;
    paint_icon(ui.painter(), rect.shrink(4.0), icon, color);
    resp
}

/// Duration line under a card ("1:22", "60s") or the kind for images.
fn card_meta(i: &Localizer, item: &Item, _now: i64) -> String {
    match item.kind {
        Kind::Screenshot => i.tr("kind-screenshot"),
        // Formats without a known length (e.g. GIF exports): show the format.
        _ if item.duration_ms == 0 => item
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default(),
        _ => {
            let s = item.duration_ms / 1000;
            if s < 60 {
                format!("{s}s")
            } else {
                format!("{}:{:02}", s / 60, s % 60)
            }
        }
    }
}

/// Draw one gallery card in `rect` (layout of the user's sketch: inset
/// thumbnail with a play badge, then title, game and length). Returns
/// (card clicked, star clicked).
/// Animation state of one card.
struct CardAnim {
    /// Entrance progress (cascade), 0..1.
    appear: f32,
    /// Thumbnail fade-in, 0..1.
    thumb: f32,
    /// Thumbnail still being decoded (shimmer placeholder).
    loading: bool,
}

#[allow(clippy::too_many_arguments)]
fn card_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    thumb_h: f32,
    item: &Item,
    tex: Option<&TextureHandle>,
    anim: CardAnim,
    selected: bool,
    look: &hc_core::config::Appearance,
    meta: &str,
) -> (bool, bool) {
    use crate::icons::{Ic, paint as paint_ic};
    let p = crate::theme::palette(ui.ctx());
    let id = ui.make_persistent_id(("card", item.id));
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let ctx = ui.ctx().clone();
    // Hover lift, as the launcher's cards (translateY(-3px) + shadow).
    let lift = crate::anim::flag(&ctx, id.with("lift"), hovered, 0.22);
    let sel = crate::anim::flag(&ctx, id.with("sel"), selected, 0.2);
    // Entrance: rise 12 px while fading in.
    let rect = rect.translate(Vec2::new(0.0, 12.0 * (1.0 - anim.appear) - 3.0 * lift));
    let mut painter = ui.painter_at(rect.expand(12.0));
    painter.multiply_opacity(anim.appear);
    if lift > 0.0 {
        painter.rect_filled(
            rect.translate(Vec2::new(0.0, 8.0)),
            p.radius,
            Color32::from_black_alpha((40.0 * lift) as u8),
        );
    }
    let border = crate::anim::mix(
        crate::anim::mix(p.border, p.line_strong, lift),
        p.accent_line,
        sel,
    );
    painter.rect(
        rect,
        p.radius,
        p.card,
        egui::Stroke::new(1.0 + sel, border),
        egui::StrokeKind::Inside,
    );
    let thumb = egui::Rect::from_min_size(
        rect.min + Vec2::splat(CARD_PAD),
        Vec2::new(rect.width() - 2.0 * CARD_PAD, thumb_h),
    );
    let tr = p.radius_sm();
    // Placeholder (hatched, with a shimmer while decoding) under the image,
    // which fades in once loaded.
    if anim.thumb < 1.0 {
        painter.rect_filled(thumb, tr, p.card_hover);
        {
            // Diagonal hatching like the sketch's placeholder.
            let mut hatch = ui.painter_at(thumb);
            hatch.multiply_opacity(anim.appear * (1.0 - anim.thumb));
            let s = egui::Stroke::new(1.0, p.border);
            let mut x = thumb.left() - thumb.height();
            while x < thumb.right() {
                hatch.line_segment(
                    [
                        egui::pos2(x, thumb.bottom()),
                        egui::pos2(x + thumb.height(), thumb.top()),
                    ],
                    s,
                );
                x += 14.0;
            }
            if anim.loading && crate::anim::rich(&ctx) {
                // Skeleton shimmer: a soft light band sweeping across.
                let t = crate::anim::pulse(&ctx, 1.3);
                let band = thumb.width() * 0.45;
                let x0 = thumb.left() - band + (thumb.width() + band) * t;
                let light = egui::Color32::from_white_alpha(if p.dark { 14 } else { 60 });
                let mut mesh = egui::Mesh::default();
                let (top, bottom) = (thumb.top(), thumb.bottom());
                for (x, c) in [
                    (x0, egui::Color32::TRANSPARENT),
                    (x0 + band / 2.0, light),
                    (x0 + band, egui::Color32::TRANSPARENT),
                ] {
                    mesh.colored_vertex(egui::pos2(x, top), c);
                    mesh.colored_vertex(egui::pos2(x, bottom), c);
                }
                for k in 0..2u32 {
                    let b = k * 2;
                    mesh.add_triangle(b, b + 1, b + 2);
                    mesh.add_triangle(b + 1, b + 2, b + 3);
                }
                hatch.add(egui::Shape::mesh(mesh));
            }
        }
    }
    if let Some(t) = tex {
        let alpha = anim.thumb * anim.appear;
        let img = t.size_vec2();
        if img.x < img.y {
            // Vertical clips (9:16 exports): show the whole frame, centred.
            let h = thumb.height();
            let w = (h * img.x / img.y).min(thumb.width());
            let r = egui::Rect::from_center_size(thumb.center(), Vec2::new(w, h));
            egui::Image::new((t.id(), r.size()))
                .tint(egui::Color32::WHITE.gamma_multiply(alpha))
                .paint_at(ui, r);
        } else {
            egui::Image::new((t.id(), thumb.size()))
                .uv(cover_uv(img, thumb.size()))
                .corner_radius(tr)
                .tint(egui::Color32::WHITE.gamma_multiply(alpha))
                .paint_at(ui, thumb);
        }
    }
    painter.rect_stroke(
        thumb,
        tr,
        egui::Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    // Play badge bottom-left (camera for screenshots, red dot for recordings).
    let badge = egui::Rect::from_center_size(
        egui::pos2(thumb.left() + 20.0, thumb.bottom() - 20.0),
        Vec2::splat(28.0),
    );
    // The play badge takes the accent and grows a little on hover.
    let badge_fill = crate::anim::mix(p.badge, p.primary_fill(false), lift);
    painter.circle_filled(badge.center(), 14.0 + 2.0 * lift, badge_fill);
    let (ic, ic_color) = match item.kind {
        Kind::Screenshot => (Ic::Camera, Color32::WHITE),
        Kind::Recording => (Ic::Record, p.danger),
        Kind::Clip => (Ic::Play, Color32::WHITE),
    };
    let ic_rect = if ic == Ic::Play {
        badge.shrink(8.0).translate(Vec2::new(1.0, 0.0))
    } else {
        badge.shrink(7.0)
    };
    paint_ic(&painter, ic_rect, ic, ic_color);
    // Star: always visible when starred, otherwise on hover.
    let star_rect = egui::Rect::from_min_size(
        egui::pos2(thumb.right() - 34.0, thumb.top() + 6.0),
        Vec2::splat(28.0),
    );
    let star = ui.interact(star_rect, id.with("star"), Sense::click());
    if item.favorite || hovered || star.hovered() {
        painter.circle_filled(star_rect.center(), 14.0, p.badge);
        let (ic, c) = if item.favorite {
            (Ic::StarFilled, Color32::from_rgb(250, 204, 21))
        } else {
            (Ic::Star, Color32::WHITE)
        };
        paint_ic(&painter, star_rect.shrink(6.0), ic, c);
    }
    // Text: title, then game and length with their icons.
    let x = rect.left() + CARD_PAD + 4.0;
    let max_w = rect.width() - 2.0 * CARD_PAD - 8.0;
    let mut y = thumb.bottom() + 10.0;
    let line = |text: &str, font: egui::FontId, color: Color32, indent: f32| {
        let mut job = egui::text::LayoutJob::single_section(
            text.to_owned(),
            egui::TextFormat::simple(font, color),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w - indent);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let title = line(&item.title, egui::FontId::proportional(14.5), p.text, 0.0);
    painter.galley(egui::pos2(x, y), title, p.text);
    y += 22.0;
    let mut info = |icon: Ic, text: &str| {
        paint_ic(
            &painter,
            egui::Rect::from_min_size(egui::pos2(x, y + 1.0), Vec2::splat(14.0)),
            icon,
            p.weak,
        );
        let g = line(text, egui::FontId::proportional(12.5), p.weak, 20.0);
        painter.galley(egui::pos2(x + 20.0, y), g, p.weak);
        y += 20.0;
    };
    if look.show_game_badge {
        let game = item
            .game
            .as_deref()
            .filter(|g| !g.is_empty())
            .unwrap_or("Desktop");
        info(Ic::Gamepad, game);
    }
    if look.show_details {
        info(Ic::Clock, meta);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp.clicked() && !star.clicked(), star.clicked())
}
/// UV rect that fills a `frame`-sized box with an image of `img` size without
/// stretching it: the image is scaled to cover the box and centred, the
/// overflowing side is cropped (CSS `object-fit: cover`).
fn cover_uv(img: Vec2, frame: Vec2) -> egui::Rect {
    if img.x <= 0.0 || img.y <= 0.0 || frame.x <= 0.0 || frame.y <= 0.0 {
        return egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    }
    let (ia, fa) = (img.x / img.y, frame.x / frame.y);
    if ia > fa {
        // Image wider than the box: crop left and right.
        let w = fa / ia;
        egui::Rect::from_min_max(
            egui::pos2((1.0 - w) / 2.0, 0.0),
            egui::pos2((1.0 + w) / 2.0, 1.0),
        )
    } else {
        let h = ia / fa;
        egui::Rect::from_min_max(
            egui::pos2(0.0, (1.0 - h) / 2.0),
            egui::pos2(1.0, (1.0 + h) / 2.0),
        )
    }
}

fn bgra_image(w: u32, h: u32, bgra: &[u8]) -> ColorImage {
    let rgba: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r, g, b, a])
        .collect();
    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba)
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_instead_of_stretching() {
        let uv = cover_uv(Vec2::new(400.0, 300.0), Vec2::new(160.0, 90.0));
        assert!((uv.width() - 1.0).abs() < 1e-6);
        assert!((uv.height() - 0.75).abs() < 1e-4);
        let uv = cover_uv(Vec2::new(320.0, 90.0), Vec2::new(160.0, 90.0));
        assert!((uv.width() - 0.5).abs() < 1e-6 && (uv.center().x - 0.5).abs() < 1e-6);
    }
}
