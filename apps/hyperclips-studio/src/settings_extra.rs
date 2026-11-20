//! Settings categories for performance, game profiles, highlights, LAN
//! sharing, local API, plugins and settings backup.

use eframe::egui::{self, RichText};
use hc_core::config::{BatteryPolicy, CaptureMode, Codec, Config, GameProfile};
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;

use crate::widgets::{card, divider, row, toggle};

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

pub fn performance(d: &mut Config, ui: &mut egui::Ui, i: &Localizer) {
    card(ui, None, |ui| {
        let p = &mut d.performance;
        row(
            ui,
            &i.tr("settings-low-impact"),
            &i.tr("settings-low-impact-hint"),
            |ui| {
                toggle(ui, &mut p.low_impact);
            },
        );
        divider(ui);
        row(
            ui,
            &i.tr("settings-on-battery"),
            &i.tr("settings-on-battery-hint"),
            |ui| {
                egui::ComboBox::from_id_salt("battery")
                    .selected_text(i.tr(match p.on_battery {
                        BatteryPolicy::LowImpact => "battery-low-impact",
                        BatteryPolicy::Pause => "battery-pause",
                        BatteryPolicy::Ignore => "battery-ignore",
                    }))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut p.on_battery,
                            BatteryPolicy::LowImpact,
                            i.tr("battery-low-impact"),
                        );
                        ui.selectable_value(
                            &mut p.on_battery,
                            BatteryPolicy::Pause,
                            i.tr("battery-pause"),
                        );
                        ui.selectable_value(
                            &mut p.on_battery,
                            BatteryPolicy::Ignore,
                            i.tr("battery-ignore"),
                        );
                    });
            },
        );
    });
}

pub fn profiles(d: &mut Config, ui: &mut egui::Ui, i: &Localizer) {
    let mut remove = None;
    for (k, p) in d.profiles.iter_mut().enumerate() {
        let title = if p.game.is_empty() {
            i.tr("profile-new")
        } else {
            p.game.clone()
        };
        card(ui, None, |ui| {
            egui::CollapsingHeader::new(RichText::new(title).strong())
                .id_salt(("profile", k))
                .default_open(p.game.is_empty())
                .show(ui, |ui| {
                    row(
                        ui,
                        &i.tr("profile-game"),
                        &i.tr("profile-game-hint"),
                        |ui| {
                            if ui.button(i.tr("profile-remove")).clicked() {
                                remove = Some(k);
                            }
                            ui.add(egui::TextEdit::singleline(&mut p.game).desired_width(200.0));
                        },
                    );
                    row(ui, &i.tr("profile-disable-replay"), "", |ui| {
                        toggle(ui, &mut p.disable_replay);
                    });
                    divider(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new(i.tr("profile-overrides")).strong());
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
        });
    }
    if let Some(k) = remove {
        d.profiles.remove(k);
    }
    if crate::widgets::primary_button(ui, &i.tr("profile-add")).clicked() {
        d.profiles.push(GameProfile::default());
    }
}

pub fn highlights(d: &mut Config, ui: &mut egui::Ui, i: &Localizer) {
    card(ui, Some(&i.tr("settings-highlights")), |ui| {
        let h = &mut d.highlights;
        row(
            ui,
            &i.tr("settings-highlights-auto"),
            &i.tr("settings-highlights-hint"),
            |ui| {
                toggle(ui, &mut h.auto_detect);
            },
        );
        divider(ui);
        row(
            ui,
            &i.tr("settings-highlights-margin"),
            &i.tr("settings-highlights-margin-hint"),
            |ui| {
                ui.add_enabled(
                    h.auto_detect,
                    egui::Slider::new(&mut h.margin_db, 3..=30).suffix(" dB"),
                );
            },
        );
    });
}

pub fn share_and_api(
    st: &mut ExtraState,
    d: &mut Config,
    ui: &mut egui::Ui,
    i: &Localizer,
    paths: &AppPaths,
) {
    card(ui, Some(&i.tr("settings-share")), |ui| {
        let s = &mut d.share;
        ui.label(
            RichText::new(i.tr("settings-share-hint"))
                .color(crate::theme::palette(ui.ctx()).weak)
                .size(12.5),
        );
        row(ui, &i.tr("settings-share-expire"), "", |ui| {
            ui.add(
                egui::DragValue::new(&mut s.expire_minutes)
                    .range(1..=1440)
                    .suffix(" min"),
            );
        });
        divider(ui);
        row(ui, &i.tr("settings-share-max"), "", |ui| {
            ui.add(egui::DragValue::new(&mut s.max_downloads).range(0..=100));
        });
    });
    card(ui, Some(&i.tr("settings-api")), |ui| {
        let a = &mut d.api;
        row(
            ui,
            &i.tr("settings-api-enabled"),
            &i.tr("settings-api-hint"),
            |ui| {
                toggle(ui, &mut a.enabled);
            },
        );
        divider(ui);
        row(ui, &i.tr("settings-api-port"), "", |ui| {
            ui.add(egui::DragValue::new(&mut a.port).range(1024..=65535));
        });
        let token_file = paths.data_dir.join("api-token.txt");
        if let Ok(t) = std::fs::read_to_string(&token_file) {
            divider(ui);
            row(
                ui,
                &i.tr("settings-api-token"),
                &i.tr("settings-api-token-hint"),
                |ui| {
                    if ui.button(i.tr("settings-api-new-token")).clicked() {
                        let _ = std::fs::remove_file(&token_file);
                        st.message = Some((true, i.tr("settings-api-token-reset")));
                    }
                    if ui.button(i.tr("settings-api-copy-token")).clicked() {
                        ui.ctx().copy_text(t.trim().to_owned());
                        st.message = Some((true, i.tr("settings-copied")));
                    }
                    ui.checkbox(&mut st.show_token, i.tr("settings-api-show-token"));
                },
            );
            if st.show_token {
                ui.monospace(t.trim());
            }
        }
        ui.hyperlink_to(
            i.tr("settings-api-docs"),
            "https://github.com/troppofranco/hyper-clips/blob/main/docs/API.md",
        );
    });
    message(st, ui);
}

pub fn plugins(
    st: &mut ExtraState,
    d: &mut Config,
    ui: &mut egui::Ui,
    i: &Localizer,
    paths: &AppPaths,
) {
    let dir = paths.data_dir.join("plugins");
    card(ui, None, |ui| {
        let p = &mut d.plugins;
        row(
            ui,
            &i.tr("settings-plugins-enabled"),
            &i.tr("settings-plugins-hint"),
            |ui| {
                toggle(ui, &mut p.enabled);
            },
        );
        divider(ui);
        let list = st.plugins.get_or_insert_with(|| {
            hc_plugins::discover(&dir)
                .into_iter()
                .map(|r| r.map_err(|(path, e)| format!("{}: {e}", path.display())))
                .collect()
        });
        if list.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(i.tr("settings-plugins-none"))
                    .color(crate::theme::palette(ui.ctx()).weak),
            );
        }
        for m in list.iter() {
            match m {
                Ok(m) => {
                    let mut on = !p.disabled.iter().any(|x| x == &m.id);
                    let perms: Vec<String> = m
                        .permissions
                        .iter()
                        .map(|x| format!("{x:?}").to_lowercase())
                        .collect();
                    let help = format!("{} · {} [{}]", m.version, m.description, perms.join(", "));
                    row(ui, &m.name, &help, |ui| {
                        if toggle(ui, &mut on).changed() {
                            p.disabled.retain(|x| x != &m.id);
                            if !on {
                                p.disabled.push(m.id.clone());
                            }
                        }
                    });
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            }
        }
        ui.add_space(6.0);
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
}

pub fn backup(
    st: &mut ExtraState,
    ui: &mut egui::Ui,
    i: &Localizer,
    paths: &AppPaths,
) -> Option<ExtraOutcome> {
    let mut outcome = None;
    card(ui, None, |ui| {
        ui.label(
            RichText::new(i.tr("settings-backup-hint")).color(crate::theme::palette(ui.ctx()).weak),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(i.tr("settings-backup-file"));
            ui.add(egui::TextEdit::singleline(&mut st.backup_path).desired_width(f32::INFINITY));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let path = std::path::PathBuf::from(st.backup_path.trim());
            if crate::widgets::primary_button(ui, &i.tr("settings-backup-save")).clicked() {
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
    });
    message(st, ui);
    outcome
}

fn message(st: &ExtraState, ui: &mut egui::Ui) {
    if let Some((ok, msg)) = &st.message {
        let color = if *ok {
            crate::theme::palette(ui.ctx()).ok
        } else {
            ui.visuals().error_fg_color
        };
        ui.colored_label(color, msg);
    }
}
