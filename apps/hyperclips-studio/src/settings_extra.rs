//! Settings sections for accessibility, performance, game profiles,
//! highlights, LAN sharing, local API, plugins and settings backup.

use eframe::egui;
use hc_core::config::{BatteryPolicy, CaptureMode, Codec, Config, GameProfile};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;

use crate::settings::section;

pub struct ExtraState {
    backup_path: String,
    pub message: Option<(bool, String)>,
    plugins: Option<Vec<Result<hc_plugins::PluginManifest, String>>>,
    show_token: bool,
}

/// What the page must do after the user restored a backup.
pub enum ExtraOutcome {
    Restored(Box<Config>),
}

impl ExtraState {
    pub fn new(paths: &AppPaths) -> Self {
        Self {
            backup_path: paths
                .data_dir
                .join("settings.hcbackup")
                .display()
                .to_string(),
            message: None,
            plugins: None,
            show_token: false,
        }
    }
}

fn opt_u32(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut Option<u32>,
    default: u32,
    range: std::ops::RangeInclusive<u32>,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *v = on.then_some(default);
        }
        if let Some(x) = v {
            ui.add(egui::DragValue::new(x).range(range).suffix(suffix));
        }
    });
}

fn opt_bool(ui: &mut egui::Ui, label: &str, v: &mut Option<bool>, i: &Localizer) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.radio_value(v, None, i.tr("profile-global"));
        ui.radio_value(v, Some(true), i.tr("profile-on"));
        ui.radio_value(v, Some(false), i.tr("profile-off"));
    });
}

pub fn ui(
    st: &mut ExtraState,
    d: &mut Config,
    ui: &mut egui::Ui,
    i: &Localizer,
    paths: &AppPaths,
) -> Option<ExtraOutcome> {
    let mut outcome = None;
    section(ui, &i.tr("settings-accessibility"), |ui| {
        let g = &mut d.general;
        ui.horizontal(|ui| {
            ui.label(i.tr("settings-ui-scale"));
            ui.add(egui::Slider::new(&mut g.ui_scale, 0.75..=2.0).step_by(0.05));
        });
        ui.checkbox(&mut g.reduce_motion, i.tr("settings-reduce-motion"));
        ui.small(i.tr("settings-accessibility-hint"));
    });

    section(ui, &i.tr("settings-performance"), |ui| {
        let p = &mut d.performance;
        ui.checkbox(&mut p.low_impact, i.tr("settings-low-impact"));
        ui.small(i.tr("settings-low-impact-hint"));
        ui.horizontal(|ui| {
            ui.label(i.tr("settings-on-battery"));
            ui.radio_value(
                &mut p.on_battery,
                BatteryPolicy::LowImpact,
                i.tr("battery-low-impact"),
            );
            ui.radio_value(
                &mut p.on_battery,
                BatteryPolicy::Pause,
                i.tr("battery-pause"),
            );
            ui.radio_value(
                &mut p.on_battery,
                BatteryPolicy::Ignore,
                i.tr("battery-ignore"),
            );
        });
    });

    section(ui, &i.tr("settings-profiles"), |ui| {
        ui.small(i.tr("settings-profiles-hint"));
        let mut remove = None;
        for (k, p) in d.profiles.iter_mut().enumerate() {
            egui::CollapsingHeader::new(if p.game.is_empty() {
                "…".to_owned()
            } else {
                p.game.clone()
            })
            .id_salt(("profile", k))
            .default_open(p.game.is_empty())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(i.tr("profile-game"));
                    ui.add(egui::TextEdit::singleline(&mut p.game).desired_width(220.0));
                    if ui.button(i.tr("profile-remove")).clicked() {
                        remove = Some(k);
                    }
                });
                ui.checkbox(&mut p.disable_replay, i.tr("profile-disable-replay"));
                opt_u32(ui, &i.tr("settings-fps"), &mut p.fps, 60, 10..=240, " FPS");
                opt_u32(
                    ui,
                    &i.tr("profile-height"),
                    &mut p.height,
                    1080,
                    360..=4320,
                    " p",
                );
                opt_u32(
                    ui,
                    &i.tr("settings-bitrate"),
                    &mut p.bitrate_kbps,
                    20_000,
                    1_000..=200_000,
                    " kbps",
                );
                opt_u32(
                    ui,
                    &i.tr("settings-replay-seconds"),
                    &mut p.replay_seconds,
                    60,
                    15..=600,
                    " s",
                );
                ui.horizontal(|ui| {
                    ui.label(i.tr("settings-codec"));
                    ui.radio_value(&mut p.codec, None, i.tr("profile-global"));
                    ui.radio_value(&mut p.codec, Some(Codec::H264), "H.264");
                    ui.radio_value(&mut p.codec, Some(Codec::Hevc), "HEVC");
                    ui.radio_value(&mut p.codec, Some(Codec::Av1), "AV1");
                });
                ui.horizontal(|ui| {
                    ui.label(i.tr("settings-capture-mode"));
                    ui.radio_value(&mut p.capture_mode, None, i.tr("profile-global"));
                    ui.radio_value(
                        &mut p.capture_mode,
                        Some(CaptureMode::Monitor),
                        i.tr("mode-monitor"),
                    );
                    ui.radio_value(
                        &mut p.capture_mode,
                        Some(CaptureMode::Window),
                        i.tr("mode-window"),
                    );
                });
                opt_bool(ui, &i.tr("settings-audio-mic"), &mut p.microphone, i);
                opt_bool(
                    ui,
                    &i.tr("settings-separate-tracks"),
                    &mut p.separate_tracks,
                    i,
                );
            });
        }
        if let Some(k) = remove {
            d.profiles.remove(k);
        }
        if ui.button(i.tr("profile-add")).clicked() {
            d.profiles.push(GameProfile::default());
        }
    });

    section(ui, &i.tr("settings-highlights"), |ui| {
        let h = &mut d.highlights;
        ui.checkbox(&mut h.auto_detect, i.tr("settings-highlights-auto"));
        ui.add_enabled(
            h.auto_detect,
            egui::Slider::new(&mut h.margin_db, 3..=30)
                .suffix(" dB")
                .text(i.tr("settings-highlights-margin")),
        );
        ui.small(i.tr("settings-highlights-hint"));
    });

    section(ui, &i.tr("settings-share"), |ui| {
        let s = &mut d.share;
        ui.horizontal(|ui| {
            ui.label(i.tr("settings-share-expire"));
            ui.add(
                egui::DragValue::new(&mut s.expire_minutes)
                    .range(1..=1440)
                    .suffix(" min"),
            );
            ui.label(i.tr("settings-share-max"));
            ui.add(egui::DragValue::new(&mut s.max_downloads).range(0..=100));
        });
        ui.small(i.tr("settings-share-hint"));
    });

    section(ui, &i.tr("settings-api"), |ui| {
        let a = &mut d.api;
        ui.horizontal(|ui| {
            ui.checkbox(&mut a.enabled, i.tr("settings-api-enabled"));
            ui.label(i.tr("settings-api-port"));
            ui.add(egui::DragValue::new(&mut a.port).range(1024..=65535));
        });
        ui.small(i.tr("settings-api-hint"));
        let token_file = paths.data_dir.join("api-token.txt");
        if let Ok(t) = std::fs::read_to_string(&token_file) {
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.show_token, i.tr("settings-api-show-token"));
                if st.show_token {
                    ui.monospace(t.trim());
                }
                if ui.button(i.tr("settings-api-copy-token")).clicked() {
                    ui.ctx().copy_text(t.trim().to_owned());
                }
                if ui.button(i.tr("settings-api-new-token")).clicked() {
                    let _ = std::fs::remove_file(&token_file);
                    st.message = Some((true, i.tr("settings-api-token-reset")));
                }
            });
        }
    });

    section(ui, &i.tr("settings-plugins"), |ui| {
        let p = &mut d.plugins;
        ui.checkbox(&mut p.enabled, i.tr("settings-plugins-enabled"));
        ui.small(i.tr("settings-plugins-hint"));
        let dir = paths.data_dir.join("plugins");
        let list = st.plugins.get_or_insert_with(|| {
            hc_plugins::discover(&dir)
                .into_iter()
                .map(|r| r.map_err(|(path, e)| format!("{}: {e}", path.display())))
                .collect()
        });
        for m in list.iter() {
            match m {
                Ok(m) => {
                    let mut on = !p.disabled.iter().any(|x| x == &m.id);
                    let perms: Vec<String> = m
                        .permissions
                        .iter()
                        .map(|x| format!("{x:?}").to_lowercase())
                        .collect();
                    let label = format!(
                        "{} {} — {} [{}]",
                        m.name,
                        m.version,
                        m.description,
                        perms.join(", ")
                    );
                    if ui.checkbox(&mut on, label).changed() {
                        p.disabled.retain(|x| x != &m.id);
                        if !on {
                            p.disabled.push(m.id.clone());
                        }
                    }
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            }
        }
        ui.horizontal(|ui| {
            if ui.button(i.tr("settings-plugins-folder")).clicked() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
            }
            if ui.button(i.tr("settings-plugins-rescan")).clicked() {
                st.plugins = None;
            }
        });
    });

    section(ui, &i.tr("settings-backup"), |ui| {
        ui.horizontal(|ui| {
            ui.label(i.tr("settings-backup-file"));
            ui.add(egui::TextEdit::singleline(&mut st.backup_path).desired_width(f32::INFINITY));
        });
        ui.horizontal(|ui| {
            let path = std::path::PathBuf::from(st.backup_path.trim());
            if ui.button(i.tr("settings-backup-save")).clicked() {
                st.message = Some(match hc_core::backup::create(paths, &path) {
                    Ok(()) => (true, i.tr("settings-backup-saved")),
                    Err(e) => (false, e.to_string()),
                });
            }
            if ui.button(i.tr("settings-backup-restore")).clicked() {
                match hc_core::backup::restore(paths, &path) {
                    Ok(cfg) => {
                        st.message = Some((true, i.tr("settings-backup-restored")));
                        outcome = Some(ExtraOutcome::Restored(Box::new(cfg)));
                    }
                    Err(e) => st.message = Some((false, e.to_string())),
                }
            }
        });
        ui.small(i.tr("settings-backup-hint"));
        if let Some((ok, msg)) = &st.message {
            let color = if *ok {
                ui.visuals().hyperlink_color
            } else {
                ui.visuals().error_fg_color
            };
            ui.colored_label(color, msg);
        }
    });
    outcome
}
