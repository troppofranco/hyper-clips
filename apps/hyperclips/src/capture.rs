//! Glue between the main thread and the capture service: lifecycle, file
//! naming, game detection and user notifications.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, unbounded};
use hc_capture::desktop::MonitorSelect;
use hc_capture::screenshot::ImageFormat;
use hc_core::config::{CaptureMode, Config, MonitorChoice, ScreenshotFormat};
use hc_core::paths::AppPaths;
use hc_detect::{Foreground, Game, GameDb, sanitize_file_name};
use hc_recorder::{CaptureService, CaptureTarget, Event, ServiceConfig, SinkCmd};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::tray::WM_APP_CAPTURE;

/// Seconds buffered when Instant Replay is off (recording pre-roll only).
const MIN_BUFFER_SECONDS: u32 = 3;

pub struct CaptureCtl {
    service: Option<CaptureService>,
    target: Option<CaptureTarget>,
    fg: Option<Foreground>,
    sent_pid: Option<u32>,
    tmp_dir: PathBuf,
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
            target: None,
            fg: None,
            sent_pid: None,
            tmp_dir: paths.tmp_dir.clone(),
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

    /// Centre of the game window if the game is in the foreground right now.
    pub fn game_in_front(&self) -> Option<(i32, i32)> {
        let (_, now) = hc_detect::foreground()?;
        (self.fg?.pid == now.pid).then_some(now.center)
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

    /// Start (or restart) capture on the right target.
    pub fn start(&mut self, config: &Config) -> Result<(), String> {
        self.service = None;
        let target = self.wanted_target(config);
        let cfg = ServiceConfig {
            capture: config.capture.clone(),
            audio: config.audio.clone(),
            webcam: config.webcam.clone(),
            target,
            game_pid: self.fg.map(|f| f.pid),
            tmp_dir: self.tmp_dir.clone(),
            min_buffer_seconds: MIN_BUFFER_SECONDS,
        };
        match CaptureService::start(cfg, self.events_tx.clone()) {
            Ok(s) => {
                self.service = Some(s);
                self.target = Some(target);
                self.sent_pid = self.fg.map(|f| f.pid);
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
        self.target = None;
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

    fn wanted_target(&self, config: &Config) -> CaptureTarget {
        let game_monitor = match (config.capture.monitor, self.fg) {
            (MonitorChoice::Game, Some(fg)) => MonitorSelect::Containing(fg.center.0, fg.center.1),
            _ => MonitorSelect::Primary,
        };
        match config.capture.mode {
            CaptureMode::Window => match self.fg {
                Some(fg) if hc_capture::window::is_supported() => CaptureTarget::Window {
                    hwnd: fg.hwnd,
                    center: fg.center,
                },
                // No game window yet: capture the monitor until one shows up.
                _ => CaptureTarget::Monitor {
                    select: game_monitor,
                    region: None,
                },
            },
            CaptureMode::Region => match config.capture.region {
                Some(r) => CaptureTarget::Monitor {
                    select: MonitorSelect::Containing(
                        r.x + r.width as i32 / 2,
                        r.y + r.height as i32 / 2,
                    ),
                    region: Some((r.x, r.y, r.width, r.height)),
                },
                None => CaptureTarget::Monitor {
                    select: game_monitor,
                    region: None,
                },
            },
            CaptureMode::Monitor => CaptureTarget::Monitor {
                select: game_monitor,
                region: None,
            },
        }
    }

    /// Periodic check: which game is in front, and does capture follow it?
    pub fn tick(&mut self, config: &Config) {
        let detected = self.db.detect();
        if let Some((g, fg)) = &detected {
            if self.game.as_ref().map(|x| &x.name) != Some(&g.name) {
                tracing::info!(game = %g.name, known = g.known, "game in foreground");
            }
            self.game = Some(g.clone());
            self.fg = Some(*fg);
        }
        let Some(fg) = detected.map(|(_, fg)| fg) else {
            return;
        };
        // Per-process audio follows the game without restarting video.
        if self.sent_pid != Some(fg.pid)
            && let Some(s) = &self.service
        {
            s.set_game_pid(Some(fg.pid));
            self.sent_pid = Some(fg.pid);
        }
        if !self.is_running() || self.is_recording() {
            return;
        }
        // Restart video only when the target really changed (never mid-recording).
        let want = self.wanted_target(config);
        let restart = match (self.target, want) {
            (
                Some(CaptureTarget::Window { hwnd: a, .. }),
                CaptureTarget::Window { hwnd: b, .. },
            ) => a != b,
            (
                Some(CaptureTarget::Monitor {
                    select: a,
                    region: ra,
                }),
                CaptureTarget::Monitor {
                    select: b,
                    region: rb,
                },
            ) => ra != rb || !same_monitor(a, b),
            (Some(_), _) => true,
            (None, _) => false,
        };
        if restart {
            tracing::info!("capture target changed (game window/monitor), restarting capture");
            let _ = self.start(config);
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
        let format = match config.capture.screenshot_format {
            ScreenshotFormat::Png => ImageFormat::Png,
            ScreenshotFormat::Jpg => ImageFormat::Jpeg,
            ScreenshotFormat::Webp => ImageFormat::Webp,
        };
        let path = Self::clips_root(config, paths)
            .join(&game)
            .join("Screenshots")
            .join(format!("{name}.{}", format.extension()));
        if let Some(s) = &self.service {
            s.screenshot(path, format);
        }
        Ok(())
    }

    pub fn set_replay_seconds(&self, seconds: u32) {
        if let Some(s) = &self.service {
            s.command(SinkCmd::SetReplaySeconds(seconds));
        }
    }
}

fn same_monitor(a: MonitorSelect, b: MonitorSelect) -> bool {
    match (
        hc_capture::desktop::select_output(a),
        hc_capture::desktop::select_output(b),
    ) {
        (Ok(x), Ok(y)) => x.device_name == y.device_name,
        _ => true,
    }
}
