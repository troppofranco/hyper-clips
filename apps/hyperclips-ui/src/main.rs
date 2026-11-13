//! Hyper Clips UI (Phase 1 spike: also used to measure the RAM cost of egui + glow).
//! Talks to the background service over the named pipe; owns no capture logic.

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui;
use hc_core::config::{Config, Theme};
use hc_core::consts::{current_user, pipe_path};
use hc_core::i18n::Translator;
use hc_core::ipc::{send_request, Request, Response, ServiceStatus};
use hc_core::paths::AppPaths;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Result of one status poll.
enum Poll {
    Status(ServiceStatus),
    Offline,
}

struct App {
    pipe: String,
    tr: Translator,
    updates: Receiver<Poll>,
    status: Option<ServiceStatus>,
    last_action: Option<String>,
}

impl App {
    fn new(ctx: &egui::Context, config: &Config, lang_dir: &std::path::Path) -> Self {
        let pipe = pipe_path(&current_user());
        // The service passes the Windows locale; without it "auto" falls back to English.
        let locale = std::env::var("HC_LOCALE").unwrap_or_default();
        let tr = Translator::new(&config.general.language, &locale, lang_dir);
        apply_theme(ctx, config.general.theme);

        // Poll the service from a worker thread so a slow pipe can never freeze the window.
        let (tx, updates) = mpsc::channel();
        let poll_pipe = pipe.clone();
        let poll_ctx = ctx.clone();
        std::thread::spawn(move || loop {
            let poll = match send_request(&poll_pipe, &Request::GetStatus) {
                Ok(Response::Status(s)) => Poll::Status(s),
                _ => Poll::Offline,
            };
            // The receiver is gone when the window closes: stop the thread.
            if tx.send(poll).is_err() {
                break;
            }
            poll_ctx.request_repaint();
            std::thread::sleep(Duration::from_secs(1));
        });

        Self {
            pipe,
            tr,
            updates,
            status: None,
            last_action: None,
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let visuals = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
        Theme::Oled => {
            let mut v = egui::Visuals::dark();
            v.panel_fill = egui::Color32::BLACK;
            v.window_fill = egui::Color32::BLACK;
            v.extreme_bg_color = egui::Color32::BLACK;
            v
        }
    };
    ctx.set_visuals(visuals);
}

fn megabytes(kb: Option<u64>) -> String {
    kb.map_or_else(
        || "?".to_owned(),
        |k| format!("{:.1} MB", k as f64 / 1024.0),
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(poll) = self.updates.try_recv() {
            self.status = match poll {
                Poll::Status(s) => Some(s),
                Poll::Offline => None,
            };
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(self.tr.t("ui_title"));
            ui.add_space(8.0);
            ui.label(self.tr.t("ui_status"));
            match &self.status {
                Some(s) => {
                    ui.label(format!("v{} - {} s", s.version, s.uptime_secs));
                    ui.label(format!(
                        "{}: {} / {}",
                        self.tr.t("ui_memory"),
                        megabytes(s.private_kb),
                        megabytes(s.working_set_kb)
                    ));
                }
                None => {
                    ui.colored_label(egui::Color32::LIGHT_RED, self.tr.t("ui_service_offline"));
                }
            }
            ui.add_space(8.0);
            ui.separator();
            if ui.button(self.tr.t("ui_save_clip")).clicked() {
                self.last_action = Some(match send_request(&self.pipe, &Request::SaveClip) {
                    Ok(Response::Ok) => "OK".to_owned(),
                    Ok(Response::Saved { path }) => path,
                    Ok(Response::Error { message }) => message,
                    Ok(_) => "?".to_owned(),
                    Err(e) => e.to_string(),
                });
            }
            if let Some(msg) = &self.last_action {
                ui.label(msg);
            }
            ui.add_space(8.0);
            ui.separator();
            ui.weak(self.tr.t("ui_privacy_note"));
        });
    }
}

fn main() -> eframe::Result {
    // Path or config problems must never prevent the window from opening.
    let paths = AppPaths::detect().ok();
    let config = paths
        .as_ref()
        .and_then(|p| Config::load_or_default(&p.config_file()).ok())
        .unwrap_or_default();
    let lang_dir = paths.map_or_else(|| PathBuf::from("lang"), |p| p.lang_dir());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 320.0])
            .with_title("Hyper Clips"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Hyper Clips",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, &config, &lang_dir)))),
    )
}
