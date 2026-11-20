//! Glue between the main thread and the capture service: lifecycle, file
//! naming, game detection and user notifications.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, unbounded};
use hc_capture::desktop::MonitorSelect;
use hc_core::config::{Config, MonitorChoice, ScreenshotFormat};
use hc_core::paths::AppPaths;
use hc_detect::{Game, GameDb, sanitize_file_name};
use hc_recorder::{CaptureService, Event, ServiceConfig, SinkCmd};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::tray::WM_APP_CAPTURE;

/// Seconds buffered when Instant Replay is off (recording pre-roll only).
const MIN_BUFFER_SECONDS: u32 = 3;

pub struct CaptureCtl {
    service: Option<CaptureService>,
    monitor: Option<MonitorSelect>,
    events_tx: Sender<Event>,
    pub events: Receiver<Event>,
    db: GameDb,
    pub game: Option<Game>,
    pub encoder: Option<String>,
}

impl CaptureCtl {
    pub fn new(hwnd: HWND, paths: &AppPaths) -> Self {
        let (svc_tx, svc_rx) = unbounded::<Event>();
        let (main_tx, main_rx) = unbounded::<Event>();
        // Forward service events to the window thread without polling.
        let hwnd_raw = hwnd.0 as isize;
        let _ = std::thread::Builder::new()
            .name("capture-events".into())
            .spawn(move || {
                for ev in svc_rx {
                    if main_tx.send(ev).is_err() {
                        break;
                    }
                    // SAFETY: PostMessageW is thread-safe.
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_raw as *mut _)),
                            WM_APP_CAPTURE,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        Self {
            service: None,
            monitor: None,
            events_tx: svc_tx,
            events: main_rx,
            db: GameDb::load(Some(&paths.config_dir.join("games.toml"))),
            game: None,
            encoder: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.service.is_some()
    }

    pub fn buffer_bytes(&self) -> usize {
        self.service
            .as_ref()
            .map_or(0, CaptureService::buffer_bytes)
    }

    pub fn is_recording(&self) -> bool {
        self.service
            .as_ref()
            .is_some_and(CaptureService::is_recording)
    }

    /// Start (or restart) capture on the right monitor.
    pub fn start(&mut self, config: &Config) -> Result<(), String> {
        self.service = None;
        let monitor = self.wanted_monitor(config);
        let cfg = ServiceConfig {
            capture: config.capture.clone(),
            audio: config.audio.clone(),
            monitor,
            min_buffer_seconds: MIN_BUFFER_SECONDS,
        };
        match CaptureService::start(cfg, self.events_tx.clone()) {
            Ok(s) => {
                self.service = Some(s);
                self.monitor = Some(monitor);
                Ok(())
            }
            Err(e) => {
                tracing::error!(error = %e, "capture service failed to start");
                Err(e.to_string())
            }
        }
    }

    pub fn stop(&mut self) {
        if self.service.take().is_some() {
            tracing::info!("capture stopped");
        }
        self.monitor = None;
    }

    /// Start or stop the service so it matches the configuration.
    pub fn sync(&mut self, config: &Config) {
        if config.capture.replay_enabled {
            if !self.is_running()
                && let Err(e) = self.start(config)
            {
                tracing::warn!(error = %e, "replay unavailable");
            }
        } else if !self.is_recording() {
            self.stop();
        }
    }

    fn wanted_monitor(&self, config: &Config) -> MonitorSelect {
        match (config.capture.monitor, &self.game) {
            (MonitorChoice::Game, Some(_)) => hc_detect::foreground()
                .map_or(MonitorSelect::Primary, |(_, fg)| {
                    MonitorSelect::Containing(fg.center.0, fg.center.1)
                }),
            _ => MonitorSelect::Primary,
        }
    }

    /// Periodic check: which game is in front, and is it on the captured monitor?
    pub fn tick(&mut self, config: &Config) {
        let detected = self.db.detect();
        let changed =
            detected.as_ref().map(|(g, _)| &g.name) != self.game.as_ref().map(|g| &g.name);
        if let Some((g, _)) = &detected {
            if changed {
                tracing::info!(game = %g.name, known = g.known, "game in foreground");
            }
            self.game = Some(g.clone());
        }
        // Follow the game to another monitor (only when idle, never mid-recording).
        if config.capture.monitor == MonitorChoice::Game
            && self.is_running()
            && !self.is_recording()
            && let Some((_, fg)) = detected
        {
            let want = MonitorSelect::Containing(fg.center.0, fg.center.1);
            let same = match (self.monitor, hc_capture::desktop::select_output(want)) {
                (Some(MonitorSelect::Containing(x, y)), Ok(o)) => o.contains(x, y),
                (Some(MonitorSelect::Primary), Ok(o)) => o.is_primary(),
                _ => true,
            };
            if !same {
                tracing::info!("game moved to another monitor, restarting capture");
                let _ = self.start(config);
            }
        }
    }

    fn base_name(&self) -> (String, String) {
        let game = self
            .game
            .as_ref()
            .map_or_else(|| "Desktop".to_owned(), |g| sanitize_file_name(&g.name));
        (
            game.clone(),
            format!("{game} {}", hc_core::sys::local_timestamp()),
        )
    }

    fn clips_root(config: &Config, paths: &AppPaths) -> PathBuf {
        config
            .general
            .clips_dir
            .clone()
            .unwrap_or_else(|| paths.default_clips_dir.clone())
    }

    pub fn save_clip(&mut self, config: &Config, paths: &AppPaths) -> Result<(), String> {
        let service = self.service.as_ref().ok_or("replay is off")?;
        let (game, name) = self.base_name();
        let path = Self::clips_root(config, paths)
            .join(&game)
            .join(format!("{name}.mp4"));
        service.command(SinkCmd::SaveClip {
            path,
            seconds: None,
        });
        Ok(())
    }

    /// Returns true if a recording was started, false if one was stopped.
    pub fn toggle_recording(&mut self, config: &Config, paths: &AppPaths) -> Result<bool, String> {
        if self.is_recording() {
            if let Some(s) = &self.service {
                s.command(SinkCmd::StopRecording);
            }
            return Ok(false);
        }
        if !self.is_running() {
            self.start(config)?;
        }
        let (game, name) = self.base_name();
        let path = Self::clips_root(config, paths)
            .join(&game)
            .join(format!("{name} REC.mp4"));
        if let Some(s) = &self.service {
            s.command(SinkCmd::StartRecording { path });
        }
        Ok(true)
    }

    pub fn add_marker(&self) -> Result<(), String> {
        let s = self.service.as_ref().ok_or("capture is off")?;
        s.command(SinkCmd::AddMarker);
        Ok(())
    }

    pub fn screenshot(&mut self, config: &Config, paths: &AppPaths) -> Result<(), String> {
        if !self.is_running() {
            self.start(config)?;
        }
        let (game, name) = self.base_name();
        let jpeg = config.capture.screenshot_format == ScreenshotFormat::Jpg;
        let ext = if jpeg { "jpg" } else { "png" };
        let path = Self::clips_root(config, paths)
            .join(&game)
            .join("Screenshots")
            .join(format!("{name}.{ext}"));
        if let Some(s) = &self.service {
            s.screenshot(path, jpeg);
        }
        Ok(())
    }

    pub fn set_replay_seconds(&self, seconds: u32) {
        if let Some(s) = &self.service {
            s.command(SinkCmd::SetReplaySeconds(seconds));
        }
    }
}
