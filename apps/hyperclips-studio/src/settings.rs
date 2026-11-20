//! Settings page: edits a draft copy of the config, saves it atomically and
//! asks the core to reload (which re-registers hotkeys and reports conflicts).

use eframe::egui;
use hc_core::config::{
    Config, Hotkeys, REPLAY_MAX_SECS, REPLAY_MIN_SECS, Theme, UpdateChannel, UpdateCheck,
};
use hc_core::i18n::{Localizer, available_languages};
use hc_core::ipc::{HotkeyConflict, Request, Response};
use hc_core::paths::{AppPaths, InstallMode};
use hc_hotkeys::{Hotkey, find_duplicates};

use crate::core_link::CoreLink;

const FPS_CHOICES: [u32; 6] = [30, 60, 120, 144, 165, 240];

pub struct SettingsPage {
    saved: Config,
    draft: Config,
    message: Option<(bool, String)>,
    conflicts: Vec<HotkeyConflict>,
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
    pub fn new(config: Config) -> Self {
        Self {
            saved: config.clone(),
            draft: config,
            message: None,
            conflicts: Vec::new(),
        }
    }

    /// Draw the page; returns the new config when it was saved.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        i: &Localizer,
        paths: &AppPaths,
        core: &CoreLink,
    ) -> Option<Config> {
        let problems = hotkey_problems(&self.draft.hotkeys);
        let mut saved = None;
        ui.horizontal(|ui| {
            ui.heading(i.tr("nav-settings"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dirty = self.draft != self.saved;
                if ui
                    .add_enabled(
                        dirty && problems.is_empty(),
                        egui::Button::new(i.tr("settings-save")),
                    )
                    .clicked()
                {
                    saved = self.save(i, paths, core);
                }
                if ui
                    .add_enabled(dirty, egui::Button::new(i.tr("settings-revert")))
                    .clicked()
                {
                    self.draft = self.saved.clone();
                }
            });
        });
        if let Some((ok, msg)) = &self.message {
            let color = if *ok {
                ui.visuals().hyperlink_color
            } else {
                ui.visuals().error_fg_color
            };
            ui.colored_label(color, msg);
        }
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            self.general(ui, i, paths);
            self.capture(ui, i);
            self.hotkeys(ui, i, &problems);
            self.updates(ui, i);
        });
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
        section(ui, &i.tr("settings-general"), |ui| {
            egui::Grid::new("general")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label(i.tr("settings-language"));
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
                    ui.end_row();

                    ui.label(i.tr("settings-theme"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut g.theme, Theme::Light, i.tr("theme-light"));
                        ui.radio_value(&mut g.theme, Theme::Dark, i.tr("theme-dark"));
                        ui.radio_value(&mut g.theme, Theme::Oled, i.tr("theme-oled"));
                    });
                    ui.end_row();

                    ui.label(i.tr("settings-autostart"));
                    if paths.mode == InstallMode::Portable {
                        ui.add_enabled(false, egui::Checkbox::new(&mut g.start_with_windows, ""))
                            .on_disabled_hover_text(i.tr("settings-autostart-portable"));
                    } else {
                        ui.checkbox(&mut g.start_with_windows, "");
                    }
                    ui.end_row();
                });
        });
    }

    fn capture(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let c = &mut self.draft.capture;
        section(ui, &i.tr("settings-capture"), |ui| {
            egui::Grid::new("capture")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label(i.tr("settings-replay-enabled"));
                    ui.checkbox(&mut c.replay_enabled, "");
                    ui.end_row();

                    ui.label(i.tr("settings-replay-seconds"));
                    ui.add(
                        egui::Slider::new(&mut c.replay_seconds, REPLAY_MIN_SECS..=REPLAY_MAX_SECS)
                            .suffix(" s"),
                    );
                    ui.end_row();

                    ui.label(i.tr("settings-fps"));
                    egui::ComboBox::from_id_salt("fps")
                        .selected_text(format!("{} FPS", c.fps))
                        .show_ui(ui, |ui| {
                            for f in FPS_CHOICES {
                                ui.selectable_value(&mut c.fps, f, format!("{f} FPS"));
                            }
                        });
                    ui.end_row();

                    ui.label(i.tr("settings-bitrate"));
                    ui.add(
                        egui::DragValue::new(&mut c.bitrate_kbps)
                            .range(1_000..=200_000)
                            .speed(250),
                    );
                    ui.end_row();
                });
        });
    }

    fn hotkeys(&mut self, ui: &mut egui::Ui, i: &Localizer, problems: &[(&str, String)]) {
        let conflicts = &self.conflicts;
        let hk = &mut self.draft.hotkeys;
        section(ui, &i.tr("settings-hotkeys"), |ui| {
            ui.small(i.tr("settings-hotkeys-hint"));
            ui.add_space(4.0);
            egui::Grid::new("hotkeys")
                .num_columns(3)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    for (id, field) in [
                        ("save-clip", &mut hk.save_clip),
                        ("toggle-recording", &mut hk.toggle_recording),
                        ("screenshot", &mut hk.screenshot),
                        ("add-marker", &mut hk.add_marker),
                        ("open-studio", &mut hk.open_studio),
                    ] {
                        ui.label(i.tr(&format!("action-{id}")));
                        let resp = ui.add(egui::TextEdit::singleline(field).desired_width(180.0));
                        // Normalise to canonical form when the field loses focus.
                        if resp.lost_focus()
                            && let Ok(Some(h)) = Hotkey::parse_optional(field)
                        {
                            *field = h.to_string();
                        }
                        let problem = problems.iter().find(|(p, _)| *p == id).map(|(_, m)| m);
                        let conflict = conflicts.iter().find(|c| c.action == id);
                        let err = ui.visuals().error_fg_color;
                        match (problem, conflict) {
                            (Some(m), _) if m.starts_with("dup:") => {
                                let other = i.tr(&format!("action-{}", &m[4..]));
                                ui.colored_label(
                                    err,
                                    i.tr_args("hotkey-duplicate", &[("other", &other)]),
                                );
                            }
                            (Some(m), _) => {
                                ui.colored_label(err, i.tr_args("hotkey-invalid", &[("error", m)]));
                            }
                            (None, Some(c)) if c.binding == *field => {
                                ui.colored_label(
                                    err,
                                    i.tr_args("hotkey-conflict", &[("reason", &c.reason)]),
                                );
                            }
                            _ => {
                                ui.label("");
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    }

    fn updates(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let u = &mut self.draft.updates;
        section(ui, &i.tr("settings-updates"), |ui| {
            egui::Grid::new("updates")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label(i.tr("settings-update-check"));
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut u.check,
                            UpdateCheck::OnStartup,
                            i.tr("update-on-startup"),
                        );
                        ui.radio_value(
                            &mut u.check,
                            UpdateCheck::EveryHours,
                            i.tr("update-every-hours"),
                        );
                        ui.radio_value(&mut u.check, UpdateCheck::Never, i.tr("update-never"));
                        if u.check == UpdateCheck::EveryHours {
                            ui.add(
                                egui::DragValue::new(&mut u.interval_hours)
                                    .range(1..=720)
                                    .suffix(" h"),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label(i.tr("settings-update-channel"));
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut u.channel,
                            UpdateChannel::Stable,
                            i.tr("channel-stable"),
                        );
                        ui.radio_value(&mut u.channel, UpdateChannel::Beta, i.tr("channel-beta"));
                    });
                    ui.end_row();
                });
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.strong(title);
    ui.add_space(4.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
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
}
