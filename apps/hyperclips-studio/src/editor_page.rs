//! Editor page. Preview and timeline in the middle; on the right the tools
//! are split into five simple steps (Trim, Frame, Text, Audio, Export) so
//! only the options of the current step are visible. The Export button is
//! always at hand; background exports are listed at the bottom.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, ColorImage, RichText, Stroke, TextureHandle, TextureOptions, vec2,
};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_editor::queue::{self, Job, State};
use hc_editor::{
    Aspect, Format, Framing, Output, Overlay, OverlayKind, Preset, Project, Segment, TrackMix,
    TransitionKind, VideoCodecChoice,
};

use crate::icons::Ic;
use crate::library_page::{Icon, icon_button};
use crate::player::Player;
use crate::theme::palette;
use crate::widgets::{card, divider, primary_button, row, segmented, toggle};

const SPEEDS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
const HEIGHTS: [u32; 6] = [0, 360, 720, 1080, 1440, 1920];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Trim,
    Frame,
    Text,
    Audio,
    Export,
}

pub struct EditorPage {
    project: Project,
    durations: Vec<u64>,
    audio_tracks: usize,
    selected: usize,
    /// Playhead inside the selected segment (source ms).
    playhead_ms: f64,
    preview: Option<(PathBuf, Player)>,
    frame: Option<TextureHandle>,
    name: String,
    srt_path: String,
    image_path: String,
    queue_dir: PathBuf,
    clips_root: PathBuf,
    library_db: PathBuf,
    thumbs_dir: PathBuf,
    last_queue_poll: Instant,
    jobs: Vec<(Job, queue::Status)>,
    message: Option<String>,
    tab: Tab,
    preset: Option<Preset>,
    library_request: bool,
    screen: crate::anim::Screen<Tab>,
}

/// Output shape of a preset: (aspect width/height, height, format label, size cap).
fn preset_shape(pr: Preset) -> (f32, u32, &'static str, Option<f64>) {
    let mut o = Output::default();
    let mut f = Framing::default();
    pr.apply(&mut o, &mut f);
    (
        aspect_ratio(f.aspect),
        o.height,
        format_label(o.format),
        o.target_size_mb,
    )
}

fn aspect_ratio(a: Aspect) -> f32 {
    match a {
        Aspect::Vertical => 9.0 / 16.0,
        Aspect::Square => 1.0,
        Aspect::Landscape | Aspect::Source => 16.0 / 9.0,
    }
}

fn format_label(f: Format) -> &'static str {
    match f {
        Format::Mp4 => "MP4",
        Format::WebM => "WebM",
        Format::Gif => "GIF",
        Format::WebP => "WebP",
    }
}

/// Draw a frame of the given aspect, centred in `rect` (the "shape" of a result).
fn paint_shape(
    painter: &egui::Painter,
    rect: egui::Rect,
    aspect: f32,
    fill: Color32,
    stroke: Color32,
) {
    let max = rect.size();
    let size = if aspect >= max.x / max.y {
        vec2(max.x, max.x / aspect)
    } else {
        vec2(max.y * aspect, max.y)
    };
    let r = egui::Rect::from_center_size(rect.center(), size);
    painter.rect(
        r,
        3.0,
        fill,
        Stroke::new(1.5, stroke),
        egui::StrokeKind::Inside,
    );
    // A tiny play mark so it reads as a video.
    let c = r.center();
    let s = (size.min_elem() * 0.18).clamp(3.0, 7.0);
    painter.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-s * 0.6, -s),
            c + vec2(s, 0.0),
            c + vec2(-s * 0.6, s),
        ],
        stroke,
        Stroke::NONE,
    ));
}

impl EditorPage {
    pub fn new(paths: &AppPaths, clips_root: PathBuf) -> Self {
        Self {
            project: Project::default(),
            durations: Vec::new(),
            audio_tracks: 0,
            selected: 0,
            playhead_ms: 0.0,
            preview: None,
            frame: None,
            name: "Montage".into(),
            srt_path: String::new(),
            image_path: String::new(),
            queue_dir: paths.cache_dir.join("exports"),
            clips_root,
            library_db: paths.library_db(),
            thumbs_dir: paths.thumbs_dir(),
            last_queue_poll: Instant::now() - Duration::from_secs(10),
            jobs: Vec::new(),
            message: None,
            tab: Tab::Trim,
            preset: None,
            library_request: false,
            screen: crate::anim::Screen::new(),
        }
    }

    /// The user asked to go and pick a clip in the library.
    pub fn take_library_request(&mut self) -> bool {
        std::mem::take(&mut self.library_request)
    }

    /// Append a clip from the library.
    pub fn add_clip(&mut self, path: &Path, title: &str) {
        let dur = hc_encode::probe::probe(path)
            .map(|r| (r.duration_ticks / 10_000).max(0) as u64)
            .unwrap_or(0);
        if dur == 0 {
            self.message = Some(format!("{}: ?", path.display()));
            return;
        }
        if self.project.segments.is_empty() {
            self.name = hc_library::sanitize(title);
            self.audio_tracks = hc_editor::decode::audio_stream_count(path);
            self.project.tracks = (0..self.audio_tracks)
                .map(|k| TrackMix {
                    volume: 1.0,
                    muted: k != 0,
                })
                .collect();
            self.tab = Tab::Trim;
        }
        self.project
            .segments
            .push(Segment::new(path.to_path_buf(), dur));
        self.durations.push(dur);
        self.selected = self.project.segments.len() - 1;
        self.playhead_ms = 0.0;
        self.preview = None;
    }

    fn spawn_worker(&self) {
        // One worker drains the queue; it survives the studio window.
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let _ = std::process::Command::new(exe)
            .arg("--export-worker")
            .arg(&self.queue_dir)
            .spawn();
    }

    fn enqueue(&mut self) {
        let ext = self.project.output.format.extension();
        let dir = self.clips_root.join("Exports");
        let _ = std::fs::create_dir_all(&dir);
        let base = hc_library::sanitize(&self.name);
        let mut out = dir.join(format!("{base}.{ext}"));
        let mut n = 2;
        while out.exists() {
            out = dir.join(format!("{base} ({n}).{ext}"));
            n += 1;
        }
        let id = format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let job = Job {
            id,
            project: self.project.clone(),
            output: out,
            library_db: Some(self.library_db.clone()),
            thumbs_dir: Some(self.thumbs_dir.clone()),
        };
        match queue::enqueue(&self.queue_dir, &job) {
            Ok(()) => {
                let running = self.jobs.iter().any(|(_, s)| s.state == State::Running);
                if !running {
                    self.spawn_worker();
                }
                self.last_queue_poll = Instant::now() - Duration::from_secs(10);
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        if self.last_queue_poll.elapsed() > Duration::from_millis(500) {
            self.jobs = queue::list(&self.queue_dir);
            self.last_queue_poll = Instant::now();
        }
        if self
            .jobs
            .iter()
            .any(|(_, s)| matches!(s.state, State::Running | State::Queued))
        {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        let p = palette(ui.ctx());
        let mut has_jobs = !self.jobs.is_empty();
        egui::Panel::bottom("queue")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 12,
                bottom: 0,
            }))
            .show_collapsible(ui, &mut has_jobs, |ui| {
                card(ui, Some(&i.tr("ed-queue")), |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .show(ui, |ui| self.queue_ui(ui, i));
                });
            });
        if self.project.segments.is_empty() {
            self.empty_ui(ui, i);
            return;
        }
        egui::Panel::right("ed-tools")
            .resizable(false)
            .exact_size(380.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 16,
                right: 0,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(p.radius())
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.tools_ui(ui, i);
                    });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.preview_ui(ui, i);
                    ui.add_space(10.0);
                    self.timeline_ui(ui, i);
                    if let Some(m) = &self.message {
                        ui.colored_label(ui.visuals().error_fg_color, m);
                    }
                });
            });
    }

    fn empty_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            crate::icons::icon(ui, Ic::Editor, 56.0, p.accent_line);
            ui.add_space(14.0);
            ui.label(RichText::new(i.tr("ed-empty-title")).size(22.0).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(i.tr("ed-empty-body")).color(p.weak));
            ui.add_space(16.0);
            if primary_button(ui, &i.tr("ed-go-library")).clicked() {
                self.library_request = true;
            }
        });
    }

    /// Step tabs, the current step's options, and the export bar.
    fn tools_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        let tabs = [
            (Tab::Trim, i.tr("ed-tab-trim")),
            (Tab::Frame, i.tr("ed-tab-frame")),
            (Tab::Text, i.tr("ed-tab-text")),
            (Tab::Audio, i.tr("ed-tab-audio")),
            (Tab::Export, i.tr("ed-tab-export")),
        ];
        ui.vertical_centered(|ui| {
            segmented(ui, &mut self.tab, &tabs);
        });
        ui.add_space(12.0);
        // Always-visible export bar at the bottom of the tools.
        egui::Panel::bottom("ed-export-bar")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 10,
                bottom: 0,
            }))
            .show(ui, |ui| {
                divider(ui);
                ui.add_space(10.0);
                ui.label(RichText::new(self.summary(i)).color(p.weak).size(12.5));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .desired_width(ui.available_width() - 120.0),
                    );
                    if primary_button(ui, &i.tr("ed-export")).clicked() {
                        self.enqueue();
                    }
                });
            });
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let t = self.screen.show(ui.ctx(), self.tab);
                crate::anim::apply_enter(ui, t);
                match self.tab {
                    Tab::Trim => self.trim_tab(ui, i),
                    Tab::Frame => self.frame_tab(ui, i),
                    Tab::Text => self.text_tab(ui, i),
                    Tab::Audio => self.audio_tab(ui, i),
                    Tab::Export => self.export_tab(ui, i),
                }
            });
    }

    /// "1280×720 · MP4 · H.264 · max 10 MB · 0:42"
    fn summary(&self, i: &Localizer) -> String {
        let o = &self.project.output;
        let mut parts = vec![if o.height == 0 {
            i.tr("ed-aspect-source")
        } else {
            format!("{}p", o.height)
        }];
        parts.push(format_label(o.format).to_owned());
        if matches!(o.format, Format::Mp4) {
            parts.push(
                match o.codec {
                    VideoCodecChoice::H264 => "H.264",
                    VideoCodecChoice::Hevc => "HEVC",
                    VideoCodecChoice::Av1 => "AV1",
                }
                .to_owned(),
            );
        }
        if let Some(mb) = o.target_size_mb {
            parts.push(i.tr_args("ed-max-mb", &[("mb", &format!("{mb:.0}"))]));
        }
        parts.push(fmt_ms(self.project.duration_ms()));
        parts.join(" · ")
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let Some(seg) = self.project.segments.get(self.selected).cloned() else {
            return;
        };
        let p = palette(ui.ctx());
        if self
            .preview
            .as_ref()
            .is_none_or(|(path, _)| *path != seg.path)
        {
            self.preview = Player::open(&seg.path, 960, 540).ok().map(|mut pl| {
                pl.pause();
                pl.seek(seg.in_ms as f64 / 1000.0);
                (seg.path.clone(), pl)
            });
            self.playhead_ms = seg.in_ms as f64;
        }
        let mut playing = false;
        if let Some((_, pl)) = self.preview.as_mut() {
            if let Some(f) = pl.poll_frame() {
                let rgba: Vec<u8> = f
                    .bgra
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|&[b, g, r, a]| [r, g, b, a])
                    .collect();
                let img = ColorImage::from_rgba_unmultiplied(
                    [f.width as usize, f.height as usize],
                    &rgba,
                );
                match &mut self.frame {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        self.frame = Some(ui.ctx().load_texture(
                            "editor-preview",
                            img,
                            TextureOptions::LINEAR,
                        ))
                    }
                }
            }
            playing = !pl.is_paused();
            if playing {
                self.playhead_ms = pl.position() * 1000.0;
                if self.playhead_ms >= seg.out_ms as f64 {
                    pl.pause();
                }
                ui.ctx().request_repaint_after(Duration::from_millis(15));
            }
        }
        card(ui, None, |ui| {
            if let Some(tex) = &self.frame {
                let size = tex.size_vec2();
                let max_h = (ui.ctx().content_rect().height() * 0.45).max(240.0);
                let scale = (ui.available_width() / size.x).min(max_h / size.y);
                let (area, _) = ui.allocate_exact_size(
                    vec2(ui.available_width(), size.y * scale),
                    egui::Sense::hover(),
                );
                let r = egui::Rect::from_center_size(area.center(), size * scale);
                egui::Image::new((tex.id(), r.size()))
                    .corner_radius(p.radius_sm())
                    .paint_at(ui, r);
                // Crop/zoom frame and overlay positions, previewed on top.
                let uv = self.project.framing.source_uv(size.x as u32, size.y as u32);
                let crop = egui::Rect::from_min_max(
                    egui::pos2(r.left() + uv[0] * r.width(), r.top() + uv[1] * r.height()),
                    egui::pos2(r.left() + uv[2] * r.width(), r.top() + uv[3] * r.height()),
                );
                let painter = ui.painter_at(r);
                let dim = Color32::from_black_alpha(150);
                for band in [
                    egui::Rect::from_min_max(r.min, egui::pos2(r.right(), crop.top())),
                    egui::Rect::from_min_max(egui::pos2(r.left(), crop.bottom()), r.max),
                    egui::Rect::from_min_max(
                        egui::pos2(r.left(), crop.top()),
                        egui::pos2(crop.left(), crop.bottom()),
                    ),
                    egui::Rect::from_min_max(
                        egui::pos2(crop.right(), crop.top()),
                        egui::pos2(r.right(), crop.bottom()),
                    ),
                ] {
                    painter.rect_filled(band, 0.0, dim);
                }
                painter.rect_stroke(
                    crop,
                    2.0,
                    Stroke::new(1.5, p.accent_line),
                    egui::StrokeKind::Inside,
                );
                for o in &self.project.overlays {
                    let pos = egui::pos2(
                        crop.left() + o.x * crop.width(),
                        crop.top() + o.y * crop.height(),
                    );
                    let label = match &o.kind {
                        OverlayKind::Text { text, .. } => {
                            text.lines().next().unwrap_or("").to_owned()
                        }
                        OverlayKind::Image { path, .. } => path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    };
                    painter.text(
                        pos,
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(15.0),
                        Color32::WHITE,
                    );
                }
            } else {
                ui.add_space(120.0);
                ui.vertical_centered(|ui| ui.spinner());
                ui.add_space(120.0);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let icon = if playing { Icon::Pause } else { Icon::Play };
                if icon_button(ui, icon, 30.0).clicked()
                    && let Some((_, pl)) = self.preview.as_mut()
                {
                    if !playing && self.playhead_ms >= seg.out_ms as f64 - 50.0 {
                        pl.seek(seg.in_ms as f64 / 1000.0);
                    }
                    pl.toggle();
                }
                ui.label(RichText::new(fmt_ms(self.playhead_ms as u64)).monospace());
                ui.spacing_mut().slider_width = (ui.available_width() - 20.0).max(60.0);
                let mut ph = self.playhead_ms;
                if ui
                    .add(
                        egui::Slider::new(&mut ph, 0.0..=self.durations[self.selected] as f64)
                            .show_value(false),
                    )
                    .changed()
                    && let Some((_, pl)) = self.preview.as_mut()
                {
                    self.playhead_ms = ph;
                    pl.seek(ph / 1000.0);
                }
            });
        });
        let _ = i;
    }

    /// Clip strip: one rounded block per segment, width ∝ length.
    fn timeline_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        ui.horizontal(|ui| {
            ui.label(RichText::new(i.tr("ed-timeline")).size(16.0).strong());
            ui.label(
                RichText::new(
                    i.tr_args("ed-duration", &[("d", &fmt_ms(self.project.duration_ms()))]),
                )
                .color(p.weak)
                .size(12.5),
            );
        });
        ui.add_space(6.0);
        let total: u64 = self
            .project
            .segments
            .iter()
            .map(Segment::output_ms)
            .sum::<u64>()
            .max(1);
        let width = ui.available_width();
        let mut select = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let gaps = 6.0 * self.project.segments.len().saturating_sub(1) as f32;
            for (k, seg) in self.project.segments.iter().enumerate() {
                let w = (seg.output_ms() as f32 / total as f32 * (width - gaps)).max(70.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 52.0), egui::Sense::click());
                let sel = crate::anim::flag(ui.ctx(), resp.id.with("sel"), k == self.selected, 0.2);
                let hover =
                    crate::anim::flag(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.15);
                let fill = crate::anim::mix(
                    crate::anim::mix(p.card, p.card_hover, hover),
                    p.accent_soft(),
                    sel,
                );
                let stroke = crate::anim::mix(p.border, p.accent_line, sel);
                ui.painter().rect(
                    rect,
                    p.radius_sm(),
                    fill,
                    Stroke::new(1.0 + sel, stroke),
                    egui::StrokeKind::Inside,
                );
                let name = seg
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let painter = ui.painter_at(rect.shrink(8.0));
                painter.text(
                    rect.left_top() + vec2(10.0, 9.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(13.0),
                    p.text,
                );
                let speed = if (seg.speed - 1.0).abs() > f32::EPSILON {
                    format!(" · {}×", seg.speed)
                } else {
                    String::new()
                };
                painter.text(
                    rect.left_bottom() + vec2(10.0, -9.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}{speed}", fmt_ms(seg.output_ms())),
                    egui::FontId::monospace(11.5),
                    p.weak,
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    select = Some(k);
                }
            }
        });
        ui.label(
            RichText::new(i.tr("ed-timeline-hint"))
                .color(p.faint)
                .size(12.0),
        );
        if let Some(k) = select {
            self.selected = k;
            self.preview = None;
            self.tab = Tab::Trim;
        }
    }

    fn trim_tab(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let ph = self.playhead_ms.max(0.0) as u64;
        let k = self.selected;
        let Some(dur) = self.durations.get(k).copied() else {
            return;
        };
        let ph = ph.min(dur);
        let mut action = None;
        card(ui, Some(&i.tr("ed-clip")), |ui| {
            let seg = &mut self.project.segments[k];
            row(ui, &i.tr("ed-start"), &fmt_ms(seg.in_ms), |ui| {
                if ui.button(i.tr("ed-from-playhead")).clicked() && ph + 100 < seg.out_ms {
                    seg.in_ms = ph;
                }
            });
            divider(ui);
            row(ui, &i.tr("ed-end"), &fmt_ms(seg.out_ms), |ui| {
                if ui.button(i.tr("ed-from-playhead")).clicked() && ph > seg.in_ms + 100 {
                    seg.out_ms = ph;
                }
            });
            divider(ui);
            ui.add_space(6.0);
            ui.label(RichText::new(i.tr("ed-speed")).strong());
            ui.add_space(4.0);
            let mut speed = seg.speed;
            let opts: Vec<(f32, String)> = SPEEDS.iter().map(|s| (*s, format!("{s}×"))).collect();
            if !SPEEDS.contains(&speed) {
                speed = 1.0;
            }
            if segmented(ui, &mut speed, &opts) {
                seg.speed = speed;
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(i.tr("ed-split"))
                    .on_hover_text(i.tr("ed-split-hint"))
                    .clicked()
                {
                    action = Some("split");
                }
                if ui.button("<").on_hover_text(i.tr("ed-move-left")).clicked() {
                    action = Some("left");
                }
                if ui
                    .button(">")
                    .on_hover_text(i.tr("ed-move-right"))
                    .clicked()
                {
                    action = Some("right");
                }
                if ui.button(i.tr("ed-remove")).clicked() {
                    action = Some("remove");
                }
            });
        });
        if self.project.segments.len() > 1 {
            card(ui, Some(&i.tr("ed-transition")), |ui| {
                let tr = &mut self.project.transition;
                segmented(
                    ui,
                    &mut tr.kind,
                    &[
                        (TransitionKind::Cut, i.tr("tr-cut")),
                        (TransitionKind::Crossfade, i.tr("tr-crossfade")),
                        (TransitionKind::FadeBlack, i.tr("tr-fade")),
                    ],
                );
                if tr.kind != TransitionKind::Cut {
                    ui.add(egui::Slider::new(&mut tr.duration_ms, 100..=3000).suffix(" ms"));
                }
            });
        }
        match action {
            Some("split") => {
                let s = self.project.segments[k].clone();
                if ph > s.in_ms + 100 && ph + 100 < s.out_ms {
                    self.project.segments[k].out_ms = ph;
                    self.project
                        .segments
                        .insert(k + 1, Segment { in_ms: ph, ..s });
                    self.durations.insert(k + 1, dur);
                }
            }
            Some("left") if k > 0 => {
                self.project.segments.swap(k, k - 1);
                self.durations.swap(k, k - 1);
                self.selected = k - 1;
            }
            Some("right") if k + 1 < self.project.segments.len() => {
                self.project.segments.swap(k, k + 1);
                self.durations.swap(k, k + 1);
                self.selected = k + 1;
            }
            Some("remove") => {
                self.project.segments.remove(k);
                self.durations.remove(k);
                self.selected = k.saturating_sub(1);
                self.preview = None;
            }
            _ => {}
        }
    }

    fn frame_tab(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        let fr = &mut self.project.framing;
        card(ui, Some(&i.tr("ed-format-shape")), |ui| {
            ui.columns(4, |cols| {
                for (k, (a, label)) in [
                    (Aspect::Source, i.tr("ed-aspect-source")),
                    (Aspect::Landscape, "16:9".to_owned()),
                    (Aspect::Vertical, "9:16".to_owned()),
                    (Aspect::Square, "1:1".to_owned()),
                ]
                .into_iter()
                .enumerate()
                {
                    let ui = &mut cols[k];
                    let on = fr.aspect == a;
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), 74.0),
                        egui::Sense::click(),
                    );
                    let sel = crate::anim::flag(ui.ctx(), resp.id.with("sel"), on, 0.2);
                    ui.painter().rect(
                        rect,
                        p.radius_sm(),
                        crate::anim::mix(p.sidebar, p.accent_soft(), sel),
                        Stroke::new(1.0, crate::anim::mix(p.line_strong, p.accent_line, sel)),
                        egui::StrokeKind::Inside,
                    );
                    let shape = egui::Rect::from_center_size(
                        rect.center() - vec2(0.0, 9.0),
                        vec2(34.0, 30.0),
                    );
                    paint_shape(
                        ui.painter(),
                        shape,
                        aspect_ratio(a),
                        p.card_hover,
                        if on { p.accent_line } else { p.weak },
                    );
                    ui.painter().text(
                        rect.center_bottom() - vec2(0.0, 10.0),
                        egui::Align2::CENTER_BOTTOM,
                        label,
                        egui::FontId::proportional(12.5),
                        p.text,
                    );
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        fr.aspect = a;
                    }
                }
            });
        });
        card(ui, Some(&i.tr("ed-zoom")), |ui| {
            ui.add(egui::Slider::new(&mut fr.zoom, 1.0..=4.0).suffix("×"));
            ui.add_space(6.0);
            egui::CollapsingHeader::new(i.tr("ed-position")).show(ui, |ui| {
                ui.add(egui::Slider::new(&mut fr.center_x, 0.0..=1.0).text(i.tr("ed-pan-x")));
                ui.add(egui::Slider::new(&mut fr.center_y, 0.0..=1.0).text(i.tr("ed-pan-y")));
            });
            ui.label(
                RichText::new(i.tr("ed-frame-hint"))
                    .color(p.faint)
                    .size(12.0),
            );
        });
    }

    fn text_tab(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        let dur = self.project.duration_ms();
        if primary_button(ui, &i.tr("ed-add-text")).clicked() {
            self.project.overlays.push(Overlay {
                kind: OverlayKind::Text {
                    text: "GG".into(),
                    size: 0.08,
                    color: [255, 255, 255, 255],
                    outline: true,
                },
                start_ms: 0,
                end_ms: dur,
                x: 0.5,
                y: 0.15,
            });
        }
        ui.add_space(10.0);
        let mut remove = None;
        for (k, o) in self.project.overlays.iter_mut().enumerate() {
            card(ui, None, |ui| {
                match &mut o.kind {
                    OverlayKind::Text {
                        text,
                        size,
                        color,
                        outline,
                    } => {
                        ui.add(
                            egui::TextEdit::multiline(text)
                                .desired_rows(1)
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            let mut c = Color32::from_rgba_unmultiplied(
                                color[0], color[1], color[2], color[3],
                            );
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                *color = c.to_srgba_unmultiplied();
                            }
                            ui.add(
                                egui::Slider::new(size, 0.02..=0.25)
                                    .show_value(false)
                                    .text(i.tr("ed-size")),
                            );
                            ui.checkbox(outline, i.tr("ed-outline"));
                        });
                    }
                    OverlayKind::Image { path, width } => {
                        ui.label(
                            RichText::new(
                                path.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            )
                            .strong(),
                        );
                        ui.add(
                            egui::Slider::new(width, 0.05..=1.0)
                                .show_value(false)
                                .text(i.tr("ed-size")),
                        );
                    }
                }
                egui::CollapsingHeader::new(i.tr("ed-position-time"))
                    .id_salt(("ov", k))
                    .show(ui, |ui| {
                        ui.add(egui::Slider::new(&mut o.x, 0.0..=1.0).text("x"));
                        ui.add(egui::Slider::new(&mut o.y, 0.0..=1.0).text("y"));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut o.start_ms)
                                    .speed(100.0)
                                    .prefix(format!("{} ", i.tr("ed-from")))
                                    .suffix(" ms"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut o.end_ms)
                                    .speed(100.0)
                                    .prefix(format!("{} ", i.tr("ed-to")))
                                    .suffix(" ms"),
                            );
                        });
                    });
                if ui.small_button(i.tr("ed-remove")).clicked() {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            self.project.overlays.remove(k);
        }
        if self.project.overlays.is_empty() {
            ui.label(
                RichText::new(i.tr("ed-text-empty"))
                    .color(p.faint)
                    .size(12.5),
            );
        }
        ui.add_space(6.0);
        egui::CollapsingHeader::new(i.tr("ed-more-overlays")).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.image_path)
                        .hint_text(i.tr("ed-image-path"))
                        .desired_width(200.0),
                );
                if ui.button(i.tr("ed-add-image")).clicked()
                    && Path::new(self.image_path.trim()).is_file()
                {
                    self.project.overlays.push(Overlay {
                        kind: OverlayKind::Image {
                            path: PathBuf::from(self.image_path.trim()),
                            width: 0.2,
                        },
                        start_ms: 0,
                        end_ms: dur,
                        x: 0.85,
                        y: 0.15,
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.srt_path)
                        .hint_text(i.tr("ed-srt-path"))
                        .desired_width(200.0),
                );
                if ui.button(i.tr("ed-import-srt")).clicked()
                    && let Ok(text) = std::fs::read_to_string(self.srt_path.trim())
                {
                    for cue in hc_editor::srt::parse(&text) {
                        self.project.overlays.push(Overlay {
                            kind: OverlayKind::Text {
                                text: cue.text,
                                size: 0.05,
                                color: [255, 255, 255, 255],
                                outline: true,
                            },
                            start_ms: cue.start_ms,
                            end_ms: cue.end_ms,
                            x: 0.5,
                            y: 0.9,
                        });
                    }
                }
            });
        });
    }

    fn audio_tab(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        card(ui, Some(&i.tr("ed-audio")), |ui| {
            if self.project.tracks.is_empty() {
                ui.label(RichText::new(i.tr("ed-no-audio")).color(p.weak));
            }
            let n = self.project.tracks.len();
            for (k, t) in self.project.tracks.iter_mut().enumerate() {
                let name = if k == 0 {
                    i.tr("ed-track-mix")
                } else {
                    i.tr_args("ed-track-n", &[("n", &k.to_string())])
                };
                row(ui, &name, "", |ui| {
                    let mut on = !t.muted;
                    if toggle(ui, &mut on).changed() {
                        t.muted = !on;
                    }
                    ui.add_enabled(
                        on,
                        egui::Slider::new(&mut t.volume, 0.0..=2.0).show_value(false),
                    );
                });
                if k + 1 < n {
                    divider(ui);
                }
            }
        });
        card(ui, None, |ui| {
            row(
                ui,
                &i.tr("ed-separate-audio"),
                &i.tr("ed-separate-audio-hint"),
                |ui| {
                    toggle(ui, &mut self.project.output.separate_audio);
                },
            );
        });
    }

    fn export_tab(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        card(ui, Some(&i.tr("ed-presets")), |ui| {
            ui.label(
                RichText::new(i.tr("ed-presets-hint"))
                    .color(p.weak)
                    .size(12.5),
            );
            ui.add_space(6.0);
            let presets = Preset::ALL;
            for pair in presets.chunks(2) {
                ui.columns(2, |cols| {
                    for (k, pr) in pair.iter().enumerate() {
                        if self.preset_tile(&mut cols[k], *pr, i) {
                            pr.apply(&mut self.project.output, &mut self.project.framing);
                            self.preset = Some(*pr);
                        }
                    }
                });
                ui.add_space(6.0);
            }
        });
        egui::CollapsingHeader::new(RichText::new(i.tr("ed-advanced")).strong()).show(ui, |ui| {
            let o = &mut self.project.output;
            let before = o.clone();
            row(ui, &i.tr("ed-format"), "", |ui| {
                egui::ComboBox::from_id_salt("fmt")
                    .selected_text(format_label(o.format))
                    .show_ui(ui, |ui| {
                        for f in [Format::Mp4, Format::WebM, Format::Gif, Format::WebP] {
                            ui.selectable_value(&mut o.format, f, format_label(f));
                        }
                    });
            });
            if o.format == Format::Mp4 {
                row(ui, &i.tr("settings-codec"), "", |ui| {
                    segmented(
                        ui,
                        &mut o.codec,
                        &[
                            (VideoCodecChoice::H264, "H.264".into()),
                            (VideoCodecChoice::Hevc, "HEVC".into()),
                            (VideoCodecChoice::Av1, "AV1".into()),
                        ],
                    );
                });
            }
            row(ui, &i.tr("ed-height"), "", |ui| {
                egui::ComboBox::from_id_salt("h")
                    .selected_text(if o.height == 0 {
                        i.tr("ed-aspect-source")
                    } else {
                        format!("{}p", o.height)
                    })
                    .show_ui(ui, |ui| {
                        for h in HEIGHTS {
                            ui.selectable_value(
                                &mut o.height,
                                h,
                                if h == 0 {
                                    i.tr("ed-aspect-source")
                                } else {
                                    format!("{h}p")
                                },
                            );
                        }
                    });
            });
            row(ui, &i.tr("settings-fps"), "", |ui| {
                ui.add(
                    egui::DragValue::new(&mut o.fps)
                        .range(5..=120)
                        .suffix(" fps"),
                );
            });
            row(ui, &i.tr("settings-bitrate"), "", |ui| {
                ui.add(
                    egui::DragValue::new(&mut o.bitrate_kbps)
                        .range(300..=150_000)
                        .speed(100.0)
                        .suffix(" kbps"),
                );
            });
            let mut on = o.target_size_mb.is_some();
            row(ui, &i.tr("ed-target"), &i.tr("ed-target-hint"), |ui| {
                if let Some(mb) = o.target_size_mb.as_mut() {
                    ui.add(egui::DragValue::new(mb).range(1.0..=4000.0).suffix(" MB"));
                }
                if toggle(ui, &mut on).changed() {
                    o.target_size_mb = on.then_some(10.0);
                }
            });
            if o.format == Format::WebM {
                ui.label(
                    RichText::new(i.tr("ed-webm-note"))
                        .color(p.faint)
                        .size(12.0),
                );
            }
            if *o != before {
                self.preset = None;
            }
        });
        ui.add_space(8.0);
        if ui.button(i.tr("ed-clear")).clicked() {
            *self = Self {
                jobs: std::mem::take(&mut self.jobs),
                ..Self::new_like(self)
            };
        }
    }

    /// One preset: the shape of the result, its name and what it produces.
    fn preset_tile(&self, ui: &mut egui::Ui, pr: Preset, i: &Localizer) -> bool {
        let p = palette(ui.ctx());
        let (aspect, height, fmt, cap) = preset_shape(pr);
        let on = self.preset == Some(pr);
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 64.0), egui::Sense::click());
        let sel = crate::anim::flag(ui.ctx(), resp.id.with("sel"), on, 0.2);
        let hover = crate::anim::flag(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.15);
        ui.painter().rect(
            rect,
            p.radius_sm(),
            crate::anim::mix(
                crate::anim::mix(p.sidebar, p.card_hover, hover),
                p.accent_soft(),
                sel,
            ),
            Stroke::new(
                1.0,
                crate::anim::mix(p.line_strong, p.accent_line, sel.max(hover * 0.6)),
            ),
            egui::StrokeKind::Inside,
        );
        let shape_box =
            egui::Rect::from_center_size(rect.left_center() + vec2(28.0, 0.0), vec2(36.0, 40.0));
        paint_shape(
            ui.painter(),
            shape_box,
            aspect,
            p.card_hover,
            if on { p.accent_line } else { p.weak },
        );
        let x = rect.left() + 54.0;
        let painter = ui.painter_at(rect.shrink(4.0));
        painter.text(
            egui::pos2(x, rect.center().y - 9.0),
            egui::Align2::LEFT_CENTER,
            i.tr(&format!("preset-{}", pr.id())),
            egui::FontId::proportional(13.5),
            p.text,
        );
        let res = if height == 0 {
            i.tr("ed-aspect-source")
        } else {
            format!("{height}p")
        };
        let detail = match cap {
            Some(mb) => format!(
                "{res} · {fmt} · {}",
                i.tr_args("ed-max-mb", &[("mb", &format!("{mb:.0}"))])
            ),
            None => format!("{res} · {fmt}"),
        };
        painter.text(
            egui::pos2(x, rect.center().y + 10.0),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(11.5),
            p.weak,
        );
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    }

    fn new_like(o: &Self) -> Self {
        Self {
            project: Project::default(),
            durations: Vec::new(),
            audio_tracks: 0,
            selected: 0,
            playhead_ms: 0.0,
            preview: None,
            frame: None,
            name: "Montage".into(),
            srt_path: String::new(),
            image_path: String::new(),
            queue_dir: o.queue_dir.clone(),
            clips_root: o.clips_root.clone(),
            library_db: o.library_db.clone(),
            thumbs_dir: o.thumbs_dir.clone(),
            last_queue_poll: o.last_queue_poll,
            jobs: Vec::new(),
            message: None,
            tab: Tab::Trim,
            preset: None,
            library_request: false,
            screen: crate::anim::Screen::new(),
        }
    }

    fn queue_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = palette(ui.ctx());
        let mut remove = None;
        for (job, st) in self.jobs.iter().rev() {
            ui.horizontal(|ui| {
                let name = job
                    .output
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.label(RichText::new(name).strong());
                match st.state {
                    State::Queued => {
                        ui.label(RichText::new(i.tr("job-queued")).color(p.weak));
                    }
                    State::Running => {
                        ui.add(
                            egui::ProgressBar::new(st.progress)
                                .desired_width(200.0)
                                .animate(true)
                                .text(format!("{:.0}%", st.progress * 100.0)),
                        );
                        if ui.button(i.tr("job-cancel")).clicked() {
                            let _ =
                                std::fs::write(queue::cancel_path(&self.queue_dir, &job.id), b"");
                        }
                    }
                    State::Done => {
                        ui.label(RichText::new(hc_core::sys::format_bytes(st.bytes)).color(p.ok));
                        if ui.button(i.tr("lib-open-folder")).clicked() {
                            let _ = hc_library::fsops::reveal_in_explorer(&job.output);
                        }
                        if ui.button(i.tr("lib-copy")).clicked() {
                            let _ = hc_library::fsops::copy_files_to_clipboard(
                                std::slice::from_ref(&job.output),
                            );
                        }
                    }
                    State::Failed | State::Cancelled => {
                        ui.colored_label(ui.visuals().error_fg_color, &st.message);
                    }
                }
                if !matches!(st.state, State::Running)
                    && ui.small_button(i.tr("ed-dismiss")).clicked()
                {
                    remove = Some(job.id.clone());
                }
            });
        }
        if let Some(id) = remove {
            queue::remove(&self.queue_dir, &id);
            self.last_queue_poll = Instant::now() - Duration::from_secs(10);
        }
    }
}

/// `hyperclips-studio.exe --export-worker <queue dir>`
pub fn run_worker(dir: &Path) {
    // Exports should not compete with the game for CPU.
    // SAFETY: lowering our own process priority.
    unsafe {
        use windows::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
        };
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
    let _guard = hc_core::sys::SingleInstance::acquire("HyperClips.ExportWorker");
    if _guard.is_none() {
        return; // another worker is already draining the queue
    }
    hc_editor::queue::run_worker(dir, &|job, out| {
        if let (Some(db), Some(thumbs)) = (&job.library_db, &job.thumbs_dir)
            && let Ok(lib) = hc_library::Library::open(db)
        {
            let _ = hc_library::index_file(&lib, out, Some("Exports".into()), thumbs);
        }
    });
}

fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}.{}", s / 60, s % 60, (ms % 1000) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_shapes() {
        assert!((preset_shape(Preset::Shorts).0 - 9.0 / 16.0).abs() < 1e-6);
        assert!((preset_shape(Preset::YouTube).0 - 16.0 / 9.0).abs() < 1e-6);
        assert_eq!(preset_shape(Preset::Gif).2, "GIF");
        assert!(preset_shape(Preset::Discord).3.is_some());
    }
}
