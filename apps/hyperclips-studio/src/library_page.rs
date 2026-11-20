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

const CARD_W: f32 = 208.0;
const THUMB: Vec2 = Vec2::new(208.0, 117.0);
const MAX_ITEMS: usize = 2000;
const THUMB_CACHE: usize = 300;
const THUMB_LOADS_PER_FRAME: usize = 12;
const RELOAD_EVERY: Duration = Duration::from_secs(3);
const SESSION_GAP_SECS: i64 = 45 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    None,
    Game,
    Session,
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
    thumbs: HashMap<i64, (TextureHandle, u64)>,
    tick: u64,
    detail: Option<Detail>,
    scan: Option<Receiver<Result<ScanReport, String>>>,
    toast: Option<(String, Instant)>,
    edit_request: Option<(std::path::PathBuf, String)>,
    vault_request: Option<(i64, std::path::PathBuf)>,
    share: Option<crate::share_ui::ShareWindow>,
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
            grouping: Grouping::None,
            items: Vec::new(),
            games: Vec::new(),
            tags: Vec::new(),
            total_size: 0,
            dirty: true,
            last_reload: Instant::now(),
            thumbs: HashMap::new(),
            tick: 0,
            detail: None,
            scan: None,
            toast: None,
            edit_request: None,
            vault_request: None,
            share: None,
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

    /// A clip the user asked to open in the editor.
    pub fn take_edit_request(&mut self) -> Option<(std::path::PathBuf, String)> {
        self.edit_request.take()
    }

    fn reload(&mut self) {
        let Some(lib) = &self.lib else { return };
        match lib.list(&self.filter, self.sort, MAX_ITEMS, 0) {
            Ok(items) => self.items = items,
            Err(e) => self.error = Some(e.to_string()),
        }
        self.games = lib.games().unwrap_or_default();
        self.tags = lib.tags().unwrap_or_default();
        self.total_size = lib.total_size().unwrap_or(0);
        self.dirty = false;
        self.last_reload = Instant::now();
    }

    fn thumb(
        &mut self,
        ctx: &egui::Context,
        item: &Item,
        budget: &mut usize,
    ) -> Option<TextureHandle> {
        self.tick += 1;
        if let Some((tex, used)) = self.thumbs.get_mut(&item.id) {
            *used = self.tick;
            return Some(tex.clone());
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
            if let Some((&old, _)) = self.thumbs.iter().min_by_key(|(_, (_, u))| *u) {
                self.thumbs.remove(&old);
            }
        }
        self.thumbs.insert(item.id, (tex.clone(), self.tick));
        Some(tex)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, i: &Localizer, config: &Config) {
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
        self.toolbar(ui, i);
        ui.separator();

        if self.detail.is_some() {
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(460.0)
                .show(ui, |ui| {
                    self.detail_ui(ui, i, config);
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if self.items.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(i.tr_args("lib-empty", &[("key", &config.hotkeys.save_clip)]));
                    if self.scan.is_some() {
                        ui.spinner();
                        ui.label(i.tr("lib-scanning"));
                    }
                });
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| self.gallery(ui));
        });
        if let Some(w) = self.share.as_mut()
            && !w.ui(ui.ctx(), i)
        {
            self.share = None; // dropping the share stops the server
        }
        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(3) {
                egui::Area::new(ui.scope_id().with("lib-toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0])
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg));
                    });
            } else {
                self.toast = None;
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let before = (self.filter.clone(), self.sort);
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter.text)
                    .hint_text(i.tr("lib-search"))
                    .desired_width(220.0),
            );
            let game_label = self
                .filter
                .game
                .clone()
                .unwrap_or_else(|| i.tr("lib-all-games"));
            egui::ComboBox::from_id_salt("game")
                .selected_text(game_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter.game, None, i.tr("lib-all-games"));
                    for (g, n) in &self.games {
                        ui.selectable_value(
                            &mut self.filter.game,
                            Some(g.clone()),
                            format!("{g} ({n})"),
                        );
                    }
                });
            let tag_label = self
                .filter
                .tag
                .clone()
                .unwrap_or_else(|| i.tr("lib-all-tags"));
            egui::ComboBox::from_id_salt("tag")
                .selected_text(tag_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter.tag, None, i.tr("lib-all-tags"));
                    for (t, n) in &self.tags {
                        ui.selectable_value(
                            &mut self.filter.tag,
                            Some(t.clone()),
                            format!("{t} ({n})"),
                        );
                    }
                });
            let kinds = [
                (None, "lib-all-kinds"),
                (Some(Kind::Clip), "kind-clip"),
                (Some(Kind::Recording), "kind-recording"),
                (Some(Kind::Screenshot), "kind-screenshot"),
            ];
            let kind_label = kinds
                .iter()
                .find(|(k, _)| *k == self.filter.kind)
                .map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("kind")
                .selected_text(i.tr(kind_label))
                .show_ui(ui, |ui| {
                    for (k, l) in kinds {
                        ui.selectable_value(&mut self.filter.kind, k, i.tr(l));
                    }
                });
            ui.toggle_value(&mut self.filter.favorites_only, i.tr("lib-favorites"));
            let sorts = [
                (Sort::Newest, "sort-newest"),
                (Sort::Oldest, "sort-oldest"),
                (Sort::Longest, "sort-longest"),
                (Sort::Shortest, "sort-shortest"),
                (Sort::Largest, "sort-largest"),
                (Sort::Title, "sort-title"),
            ];
            let sort_label = sorts
                .iter()
                .find(|(s, _)| *s == self.sort)
                .map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("sort")
                .selected_text(format!("{}: {}", i.tr("lib-sort"), i.tr(sort_label)))
                .show_ui(ui, |ui| {
                    for (s, l) in sorts {
                        ui.selectable_value(&mut self.sort, s, i.tr(l));
                    }
                });
            let groups = [
                (Grouping::None, "group-none"),
                (Grouping::Game, "group-game"),
                (Grouping::Session, "group-session"),
            ];
            let group_label = groups
                .iter()
                .find(|(g, _)| *g == self.grouping)
                .map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("group")
                .selected_text(format!("{}: {}", i.tr("lib-group"), i.tr(group_label)))
                .show_ui(ui, |ui| {
                    for (g, l) in groups {
                        ui.selectable_value(&mut self.grouping, g, i.tr(l));
                    }
                });
            ui.label(i.tr_args(
                "lib-summary",
                &[
                    ("count", &self.items.len().to_string()),
                    ("size", &hc_core::sys::format_bytes(self.total_size)),
                ],
            ));
            if self.scan.is_some() {
                ui.spinner();
            }
        });
        if (self.filter.clone(), self.sort) != before {
            self.dirty = true;
        }
    }

    fn gallery(&mut self, ui: &mut egui::Ui) {
        let groups = match self.grouping {
            Grouping::None => vec![hc_library::Group {
                label: String::new(),
                items: (0..self.items.len()).collect(),
            }],
            Grouping::Game => group_by_game(&self.items),
            Grouping::Session => group_by_session(&self.items, SESSION_GAP_SECS),
        };
        let mut budget = THUMB_LOADS_PER_FRAME;
        let mut clicked = None;
        let mut fav_toggle = None;
        for g in groups {
            if !g.label.is_empty() {
                let first = g
                    .items
                    .first()
                    .map(|&i| self.items[i].created_at)
                    .unwrap_or(0);
                let header = match self.grouping {
                    Grouping::Session => {
                        format!("{} · {}", g.label, hc_core::sys::format_local(first))
                    }
                    _ => g.label.clone(),
                };
                ui.add_space(6.0);
                ui.heading(format!("{header}  ({})", g.items.len()));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
                for idx in g.items {
                    let item = self.items[idx].clone();
                    // Only decode thumbnails for cards that are on screen.
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(CARD_W, THUMB.y + 46.0), Sense::hover());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let tex = self.thumb(ui.ctx(), &item, &mut budget);
                    let selected = self.detail.as_ref().is_some_and(|d| d.id == item.id);
                    let (card_clicked, star_clicked) =
                        card_ui(ui, rect, &item, tex.as_ref(), selected);
                    if card_clicked {
                        clicked = Some(item.id);
                    }
                    if star_clicked {
                        fav_toggle = Some((item.id, !item.favorite));
                    }
                }
            });
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
            if !p.is_paused() {
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

/// Vector icons (the default egui fonts lack ★ ☆ ⏸).
#[derive(Clone, Copy)]
pub enum Icon {
    Star(bool),
    Play,
    Pause,
}

pub fn paint_icon(p: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.45;
    match icon {
        Icon::Star(filled) => {
            let pts: Vec<egui::Pos2> = (0..10)
                .map(|k| {
                    let ang = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                    let rad = if k % 2 == 0 { r } else { r * 0.45 };
                    egui::pos2(c.x + rad * ang.cos(), c.y + rad * ang.sin())
                })
                .collect();
            if filled {
                // Fill as a fan of triangles (the star is not convex).
                for k in 0..10 {
                    let a = pts[k];
                    let b = pts[(k + 1) % 10];
                    p.add(egui::Shape::convex_polygon(
                        vec![c, a, b],
                        color,
                        egui::Stroke::NONE,
                    ));
                }
            } else {
                p.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.5, color)));
            }
        }
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
    let color = match icon {
        Icon::Star(true) => Color32::from_rgb(250, 204, 21),
        _ => visuals.fg_stroke.color,
    };
    paint_icon(ui.painter(), rect.shrink(4.0), icon, color);
    resp
}

/// Draw one gallery card in `rect`. Returns (card clicked, star clicked).
fn card_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    item: &Item,
    tex: Option<&TextureHandle>,
    selected: bool,
) -> (bool, bool) {
    let id = ui.make_persistent_id(("card", item.id));
    let resp = ui.interact(rect, id, Sense::click());
    let painter = ui.painter_at(rect);
    let v = ui.visuals();
    let bg = if resp.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        v.faint_bg_color
    };
    painter.rect_filled(rect, 6.0, bg);
    let thumb = egui::Rect::from_min_size(rect.min, THUMB);
    match tex {
        Some(t) => {
            egui::Image::new((t.id(), THUMB))
                .corner_radius(6.0)
                .paint_at(ui, thumb);
        }
        None => {
            painter.rect_filled(thumb, 6.0, v.extreme_bg_color);
        }
    }
    if item.duration_ms > 0 {
        let s = item.duration_ms / 1000;
        let galley = painter.layout_no_wrap(
            format!("{}:{:02}", s / 60, s % 60),
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
        let pos = thumb.right_bottom() - Vec2::new(6.0, 6.0) - galley.size();
        painter.rect_filled(
            egui::Rect::from_min_size(pos - Vec2::splat(3.0), galley.size() + Vec2::splat(6.0)),
            3.0,
            Color32::from_black_alpha(170),
        );
        painter.galley(pos, galley, Color32::WHITE);
    }
    // Star toggle in the thumbnail's top-left corner.
    let star_rect = egui::Rect::from_min_size(thumb.min + Vec2::splat(6.0), Vec2::splat(22.0));
    let star = ui.interact(star_rect, id.with("star"), Sense::click());
    painter.circle_filled(star_rect.center(), 11.0, Color32::from_black_alpha(140));
    let star_color = if item.favorite {
        Color32::from_rgb(250, 204, 21)
    } else {
        Color32::WHITE
    };
    paint_icon(
        &painter,
        star_rect.shrink(3.0),
        Icon::Star(item.favorite),
        star_color,
    );
    // Title + subtitle, clipped to the card.
    let text_color = v.text_color();
    painter.text(
        egui::pos2(rect.left() + 8.0, thumb.bottom() + 6.0),
        egui::Align2::LEFT_TOP,
        &item.title,
        egui::FontId::proportional(13.0),
        text_color,
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, thumb.bottom() + 24.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{} · {}",
            item.game.clone().unwrap_or_default(),
            hc_core::sys::format_local(item.created_at)
        ),
        egui::FontId::proportional(11.0),
        v.weak_text_color(),
    );
    if selected {
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(2.0, v.selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    (resp.clicked() && !star.clicked(), star.clicked())
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
