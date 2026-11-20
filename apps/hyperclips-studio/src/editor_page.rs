//! Editor page: timeline of segments (trim, split, speed, order), framing
//! (9:16 / 1:1 / zoom / pan), transitions, audio tracks, text & image
//! overlays, subtitles (.srt), output presets and the background export queue.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_editor::queue::{self, Job, State};
use hc_editor::{
    Aspect, Format, Overlay, OverlayKind, Preset, Project, Segment, TrackMix, TransitionKind,
    VideoCodecChoice,
};

use crate::library_page::{Icon, icon_button};
use crate::player::Player;

const SPEEDS: [f32; 7] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 4.0];
const HEIGHTS: [u32; 6] = [0, 360, 720, 1080, 1440, 1920];

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
        }
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
        egui::Panel::bottom("queue")
            .resizable(true)
            .default_size(140.0)
            .show(ui, |ui| self.queue_ui(ui, i));
        egui::Panel::right("output")
            .resizable(true)
            .default_size(330.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.side_ui(ui, i));
            });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.project.segments.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(i.tr("ed-empty")));
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.preview_ui(ui, i);
                ui.separator();
                self.timeline_ui(ui, i);
            });
        });
        if let Some(m) = &self.message {
            ui.colored_label(ui.visuals().error_fg_color, m);
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let Some(seg) = self.project.segments.get(self.selected).cloned() else {
            return;
        };
        if self.preview.as_ref().is_none_or(|(p, _)| *p != seg.path) {
            self.preview = Player::open(&seg.path, 960, 540).ok().map(|mut pl| {
                pl.pause();
                pl.seek(seg.in_ms as f64 / 1000.0);
                (seg.path.clone(), pl)
            });
            self.playhead_ms = seg.in_ms as f64;
        }
        let mut playing = false;
        if let Some((_, p)) = self.preview.as_mut() {
            if let Some(f) = p.poll_frame() {
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
            playing = !p.is_paused();
            if playing {
                self.playhead_ms = p.position() * 1000.0;
                if self.playhead_ms >= seg.out_ms as f64 {
                    p.pause();
                }
                ui.ctx().request_repaint_after(Duration::from_millis(15));
            }
        }
        if let Some(tex) = &self.frame {
            let size = tex.size_vec2();
            let scale = (ui.available_width() / size.x).min(420.0 / size.y).min(1.0);
            let resp = ui.image((tex.id(), size * scale));
            let r = resp.rect;
            // Crop/zoom frame and overlay positions, previewed on top.
            let uv = self.project.framing.source_uv(size.x as u32, size.y as u32);
            let crop = egui::Rect::from_min_max(
                egui::pos2(r.left() + uv[0] * r.width(), r.top() + uv[1] * r.height()),
                egui::pos2(r.left() + uv[2] * r.width(), r.top() + uv[3] * r.height()),
            );
            let dim = Color32::from_black_alpha(140);
            let p = ui.painter();
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
                p.rect_filled(band, 0.0, dim);
            }
            p.rect_stroke(
                crop,
                0.0,
                egui::Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
            for o in &self.project.overlays {
                let pos = egui::pos2(
                    crop.left() + o.x * crop.width(),
                    crop.top() + o.y * crop.height(),
                );
                let label = match &o.kind {
                    OverlayKind::Text { text, .. } => text.lines().next().unwrap_or("").to_owned(),
                    OverlayKind::Image { path, .. } => path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                };
                p.text(
                    pos,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(14.0),
                    Color32::YELLOW,
                );
            }
        }
        ui.horizontal(|ui| {
            let icon = if playing { Icon::Pause } else { Icon::Play };
            if icon_button(ui, icon, 26.0).clicked()
                && let Some((_, p)) = self.preview.as_mut()
            {
                if !playing && self.playhead_ms >= seg.out_ms as f64 - 50.0 {
                    p.seek(seg.in_ms as f64 / 1000.0);
                }
                p.toggle();
            }
            ui.spacing_mut().slider_width = (ui.available_width() - 80.0).max(60.0);
            let mut ph = self.playhead_ms;
            if ui
                .add(
                    egui::Slider::new(&mut ph, 0.0..=self.durations[self.selected] as f64)
                        .show_value(false),
                )
                .changed()
                && let Some((_, p)) = self.preview.as_mut()
            {
                self.playhead_ms = ph;
                p.seek(ph / 1000.0);
            }
            ui.label(fmt_ms(self.playhead_ms as u64));
        });
        let _ = i;
    }

    fn timeline_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        ui.heading(i.tr("ed-timeline"));
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
            ui.spacing_mut().item_spacing.x = 2.0;
            for (k, seg) in self.project.segments.iter().enumerate() {
                let w = (seg.output_ms() as f32 / total as f32 * width).max(40.0);
                let name = seg
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let label = format!("{name}\n{} · {}x", fmt_ms(seg.output_ms()), seg.speed);
                let fill = if k == self.selected {
                    ui.visuals().selection.bg_fill
                } else {
                    ui.visuals().widgets.inactive.weak_bg_fill
                };
                if ui
                    .add_sized(
                        [w, 44.0],
                        egui::Button::new(egui::RichText::new(label).small()).fill(fill),
                    )
                    .clicked()
                {
                    select = Some(k);
                }
            }
        });
        if let Some(k) = select {
            self.selected = k;
            self.preview = None;
        }
        ui.label(i.tr_args("ed-duration", &[("d", &fmt_ms(self.project.duration_ms()))]));
        let Some(seg) = self.project.segments.get_mut(self.selected) else {
            return;
        };
        let dur = self.durations[self.selected];
        let ph = self.playhead_ms.clamp(0.0, dur as f64) as u64;
        egui::Grid::new("seg")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label(i.tr("ed-in"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut seg.in_ms)
                            .range(0..=seg.out_ms.saturating_sub(100))
                            .speed(50.0)
                            .suffix(" ms"),
                    );
                    if ui.button(i.tr("ed-set-playhead")).clicked() && ph + 100 < seg.out_ms {
                        seg.in_ms = ph;
                    }
                });
                ui.end_row();
                ui.label(i.tr("ed-out"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut seg.out_ms)
                            .range(seg.in_ms + 100..=dur)
                            .speed(50.0)
                            .suffix(" ms"),
                    );
                    if ui.button(i.tr("ed-set-playhead")).clicked() && ph > seg.in_ms + 100 {
                        seg.out_ms = ph;
                    }
                });
                ui.end_row();
                ui.label(i.tr("ed-speed"));
                ui.horizontal(|ui| {
                    for s in SPEEDS {
                        ui.selectable_value(&mut seg.speed, s, format!("{s}x"));
                    }
                });
                ui.end_row();
            });
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button(i.tr("ed-split")).clicked() {
                action = Some("split");
            }
            if ui.button(i.tr("ed-move-left")).clicked() {
                action = Some("left");
            }
            if ui.button(i.tr("ed-move-right")).clicked() {
                action = Some("right");
            }
            if ui.button(i.tr("ed-remove")).clicked() {
                action = Some("remove");
            }
        });
        let k = self.selected;
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

    fn side_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let p = &mut self.project;
        ui.strong(i.tr("ed-presets"));
        ui.horizontal_wrapped(|ui| {
            for pr in Preset::ALL {
                if ui.button(i.tr(&format!("preset-{}", pr.id()))).clicked() {
                    pr.apply(&mut p.output, &mut p.framing);
                }
            }
        });
        ui.separator();
        ui.strong(i.tr("ed-framing"));
        ui.horizontal_wrapped(|ui| {
            for (a, l) in [
                (Aspect::Source, "ed-aspect-source"),
                (Aspect::Landscape, "16:9"),
                (Aspect::Vertical, "9:16"),
                (Aspect::Square, "1:1"),
            ] {
                let text = if l.starts_with("ed-") {
                    i.tr(l)
                } else {
                    l.to_owned()
                };
                ui.radio_value(&mut p.framing.aspect, a, text);
            }
        });
        ui.add(egui::Slider::new(&mut p.framing.zoom, 1.0..=4.0).text(i.tr("ed-zoom")));
        ui.add(egui::Slider::new(&mut p.framing.center_x, 0.0..=1.0).text(i.tr("ed-pan-x")));
        ui.add(egui::Slider::new(&mut p.framing.center_y, 0.0..=1.0).text(i.tr("ed-pan-y")));
        ui.separator();
        ui.strong(i.tr("ed-transition"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut p.transition.kind, TransitionKind::Cut, i.tr("tr-cut"));
            ui.radio_value(
                &mut p.transition.kind,
                TransitionKind::Crossfade,
                i.tr("tr-crossfade"),
            );
            ui.radio_value(
                &mut p.transition.kind,
                TransitionKind::FadeBlack,
                i.tr("tr-fade"),
            );
        });
        ui.add(egui::Slider::new(&mut p.transition.duration_ms, 100..=3000).suffix(" ms"));
        ui.separator();
        ui.strong(i.tr("ed-audio"));
        for (k, t) in p.tracks.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let mut on = !t.muted;
                let name = if k == 0 {
                    i.tr("ed-track-mix")
                } else {
                    i.tr_args("ed-track-n", &[("n", &k.to_string())])
                };
                if ui.checkbox(&mut on, name).changed() {
                    t.muted = !on;
                }
                ui.add_enabled(on, egui::Slider::new(&mut t.volume, 0.0..=2.0));
            });
        }
        ui.checkbox(&mut p.output.separate_audio, i.tr("ed-separate-audio"));
        ui.separator();
        ui.strong(i.tr("ed-overlays"));
        let mut remove = None;
        for (k, o) in p.overlays.iter_mut().enumerate() {
            ui.group(|ui| {
                match &mut o.kind {
                    OverlayKind::Text {
                        text,
                        size,
                        color,
                        outline,
                    } => {
                        ui.add(egui::TextEdit::multiline(text).desired_rows(1));
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(size, 0.02..=0.25).text(i.tr("ed-size")));
                            let mut c = Color32::from_rgba_unmultiplied(
                                color[0], color[1], color[2], color[3],
                            );
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                *color = c.to_srgba_unmultiplied();
                            }
                            ui.checkbox(outline, i.tr("ed-outline"));
                        });
                    }
                    OverlayKind::Image { path, width } => {
                        ui.label(path.display().to_string());
                        ui.add(egui::Slider::new(width, 0.05..=1.0).text(i.tr("ed-size")));
                    }
                }
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
                    if ui.button(i.tr("ed-remove")).clicked() {
                        remove = Some(k);
                    }
                });
            });
        }
        if let Some(k) = remove {
            p.overlays.remove(k);
        }
        let dur = p.duration_ms();
        ui.horizontal_wrapped(|ui| {
            if ui.button(i.tr("ed-add-text")).clicked() {
                p.overlays.push(Overlay {
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
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.image_path)
                    .hint_text(i.tr("ed-image-path"))
                    .desired_width(180.0),
            );
            if ui.button(i.tr("ed-add-image")).clicked()
                && Path::new(self.image_path.trim()).is_file()
            {
                p.overlays.push(Overlay {
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
                    .desired_width(180.0),
            );
            if ui.button(i.tr("ed-import-srt")).clicked()
                && let Ok(text) = std::fs::read_to_string(self.srt_path.trim())
            {
                for cue in hc_editor::srt::parse(&text) {
                    p.overlays.push(Overlay {
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
        ui.separator();
        ui.strong(i.tr("ed-output"));
        let o = &mut p.output;
        egui::Grid::new("out")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(i.tr("ed-format"));
                ui.horizontal(|ui| {
                    for (f, l) in [
                        (Format::Mp4, "MP4"),
                        (Format::WebM, "WebM"),
                        (Format::Gif, "GIF"),
                        (Format::WebP, "WebP"),
                    ] {
                        ui.radio_value(&mut o.format, f, l);
                    }
                });
                ui.end_row();
                if o.format == Format::Mp4 {
                    ui.label(i.tr("settings-codec"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut o.codec, VideoCodecChoice::H264, "H.264");
                        ui.radio_value(&mut o.codec, VideoCodecChoice::Hevc, "HEVC");
                        ui.radio_value(&mut o.codec, VideoCodecChoice::Av1, "AV1");
                    });
                    ui.end_row();
                }
                ui.label(i.tr("ed-height"));
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
                ui.end_row();
                ui.label(i.tr("settings-fps"));
                ui.add(egui::DragValue::new(&mut o.fps).range(5..=120));
                ui.end_row();
                ui.label(i.tr("settings-bitrate"));
                ui.add(
                    egui::DragValue::new(&mut o.bitrate_kbps)
                        .range(300..=150_000)
                        .speed(100.0),
                );
                ui.end_row();
                ui.label(i.tr("ed-target"));
                let mut on = o.target_size_mb.is_some();
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, "").changed() {
                        o.target_size_mb = on.then_some(10.0);
                    }
                    if let Some(mb) = o.target_size_mb.as_mut() {
                        ui.add(egui::DragValue::new(mb).range(1.0..=4000.0).suffix(" MB"));
                        for v in [10.0, 25.0, 50.0, 500.0] {
                            if ui.small_button(format!("{v}")).clicked() {
                                *mb = v;
                            }
                        }
                    }
                });
                ui.end_row();
            });
        if o.format == Format::WebM {
            ui.small(i.tr("ed-webm-note"));
        }
        ui.horizontal(|ui| {
            ui.label(i.tr("ed-name"));
            ui.text_edit_singleline(&mut self.name);
        });
        let can_export = !self.project.segments.is_empty();
        if ui
            .add_enabled(
                can_export,
                egui::Button::new(egui::RichText::new(i.tr("ed-export")).strong()),
            )
            .clicked()
        {
            self.enqueue();
        }
        if ui.button(i.tr("ed-clear")).clicked() {
            *self = Self {
                jobs: std::mem::take(&mut self.jobs),
                ..Self::new_like(self)
            };
        }
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
        }
    }

    fn queue_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        ui.strong(i.tr("ed-queue"));
        if self.jobs.is_empty() {
            ui.weak(i.tr("ed-queue-empty"));
        }
        let mut remove = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (job, st) in self.jobs.iter().rev() {
                ui.horizontal(|ui| {
                    let name = job
                        .output
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.label(name);
                    match st.state {
                        State::Queued => {
                            ui.weak(i.tr("job-queued"));
                        }
                        State::Running => {
                            ui.add(
                                egui::ProgressBar::new(st.progress)
                                    .desired_width(200.0)
                                    .text(format!("{:.0}% · {}", st.progress * 100.0, st.pass)),
                            );
                            if ui.button(i.tr("job-cancel")).clicked() {
                                let _ = std::fs::write(
                                    queue::cancel_path(&self.queue_dir, &job.id),
                                    b"",
                                );
                            }
                        }
                        State::Done => {
                            ui.label(format!(
                                "OK · {} · {}",
                                hc_core::sys::format_bytes(st.bytes),
                                st.message
                            ));
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
                    if !matches!(st.state, State::Running) && ui.small_button("×").clicked() {
                        remove = Some(job.id.clone());
                    }
                });
            }
        });
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
