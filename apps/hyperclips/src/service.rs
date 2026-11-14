//! Service wiring (Windows only).

use anyhow::{Context, Result};
use hc_core::config::{Config, UpdateCheck};
use hc_core::consts::{current_user, pipe_path, UI_EXE};
use hc_core::error::Error as CoreError;
use hc_core::hotkey::HotkeyAction;
use hc_core::i18n::Translator;
use hc_core::ipc::{send_request, Request, Response, ServiceStatus};
use hc_core::paths::AppPaths;
use hc_core::platform::MemoryProbe;
use hc_library::{Library, NewClip};
use hc_os_win::tray::{self, TrayEvent, TrayLabels};
use hc_os_win::{clock, foreground, instance, locale, memory::WinMemoryProbe, pipe, sound};

use crate::pipeline::{Pipeline, PipelineSettings};
use crate::update::UpdateCtx;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Window handle of the tray window, used by the IPC `Shutdown` request.
static TRAY_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Starts the UI process on demand and avoids starting a second copy.
struct UiLauncher {
    exe: PathBuf,
    /// System locale handed to the UI so "auto" resolves the same way as in the tray.
    locale: String,
    child: Mutex<Option<Child>>,
}

impl UiLauncher {
    fn open(&self) -> Result<()> {
        let mut slot = self
            .child
            .lock()
            .map_err(|_| anyhow::anyhow!("UI launcher lock poisoned"))?;
        if let Some(child) = slot.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                // Already running. Bringing its window to the front is a later improvement.
                return Ok(());
            }
        }
        let child = Command::new(&self.exe)
            .env("HC_LOCALE", &self.locale)
            .spawn()
            .with_context(|| format!("cannot start {}", self.exe.display()))?;
        *slot = Some(child);
        Ok(())
    }
}

/// Remembers the last fullscreen program seen in the foreground.
struct GameDetector {
    last: Mutex<Option<(String, Instant)>>,
}

impl GameDetector {
    /// How long a game stays "current" after it leaves the foreground (alt-tab, menus).
    const GRACE: Duration = Duration::from_secs(30);

    fn start(ignore: Vec<String>) -> Arc<Self> {
        let detector = Arc::new(Self {
            last: Mutex::new(None),
        });
        let worker = Arc::clone(&detector);
        let spawned = std::thread::Builder::new()
            .name("hc-games".into())
            .spawn(move || loop {
                if let Some(app) = foreground::foreground_app() {
                    if app.fullscreen && !hc_core::games::is_ignored(&app.exe_name, &ignore) {
                        if let Ok(mut last) = worker.last.lock() {
                            *last =
                                Some((hc_core::games::display_name(&app.exe_name), Instant::now()));
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            });
        if let Err(e) = spawned {
            tracing::warn!("game detection unavailable: {e}");
        }
        detector
    }

    fn current(&self) -> String {
        self.last
            .lock()
            .ok()
            .and_then(|l| l.clone())
            .filter(|(_, seen)| seen.elapsed() < Self::GRACE)
            .map(|(name, _)| name)
            .unwrap_or_default()
    }
}

/// Saves clips from the running pipeline into the clips folder.
struct ClipSaver {
    pipeline: Option<Arc<Pipeline>>,
    dir: PathBuf,
    secs: u32,
    library: Option<Mutex<Library>>,
    games: Option<Arc<GameDetector>>,
}

impl ClipSaver {
    fn save(&self) -> Result<PathBuf> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("capture is not running (see the log)"))?;
        let path = self
            .dir
            .join(format!("HyperClips_{}.mp4", clock::local_timestamp()));
        let duration_ms = pipeline.save_clip(&path, self.secs)?;
        self.record_in_library(&path, duration_ms);
        Ok(path)
    }
}

impl ClipSaver {
    /// The file is already on disk: a library failure is logged, never fatal.
    fn record_in_library(&self, path: &std::path::Path, duration_ms: i64) {
        let Some(lib) = &self.library else { return };
        let size = std::fs::metadata(path).map_or(0, |m| m.len() as i64);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let entry = NewClip {
            path: path.to_string_lossy().into_owned(),
            created_at: now,
            duration_ms,
            size_bytes: size,
            game: self.games.as_ref().map(|g| g.current()).unwrap_or_default(),
        };
        match lib.lock() {
            Ok(l) => {
                if let Err(e) = l.add(&entry) {
                    tracing::warn!("cannot record the clip in the library: {e}");
                }
            }
            Err(_) => tracing::warn!("library lock poisoned"),
        }
    }

    /// Starts or stops a full-session recording. `Some(path)` = it just stopped.
    fn toggle_recording(&self) -> Result<Option<PathBuf>> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("capture is not running (see the log)"))?;
        if pipeline.is_recording() {
            let stopped = pipeline.stop_recording()?;
            return Ok(stopped.map(|(path, ms)| {
                self.record_in_library(&path, ms);
                path
            }));
        }
        let path = self
            .dir
            .join(format!("HyperClips_{}_rec.mp4", clock::local_timestamp()));
        pipeline.start_recording(path)?;
        Ok(None)
    }

    fn screenshot(&self) -> Result<PathBuf> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("capture is not running (see the log)"))?;
        let path = self
            .dir
            .join("Screenshots")
            .join(format!("HyperClips_{}.png", clock::local_timestamp()));
        pipeline.screenshot(&path)?;
        Ok(path)
    }
}

fn clips_dir(config: &Config, paths: &AppPaths) -> PathBuf {
    if !config.storage.clips_dir.is_empty() {
        PathBuf::from(&config.storage.clips_dir)
    } else if paths.is_portable() {
        paths.root().join("clips")
    } else if let Some(profile) = std::env::var_os("USERPROFILE") {
        PathBuf::from(profile).join("Videos").join("HyperClips")
    } else {
        paths.root().join("clips")
    }
}

pub fn run() -> Result<()> {
    let paths = AppPaths::detect()?;
    let pipe_name = pipe_path(&current_user());

    // A second launch just asks the running instance to show its UI.
    let _instance = match instance::acquire() {
        Ok(guard) => guard,
        Err(CoreError::AlreadyRunning) => {
            let _ = send_request(&pipe_name, &Request::OpenUi);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    paths.ensure_dirs()?;
    let _log_guard = hc_core::logging::init(&paths.logs_dir(), "service", cfg!(debug_assertions))?;
    tracing::info!(
        "Hyper Clips {} starting (portable: {})",
        env!("CARGO_PKG_VERSION"),
        paths.is_portable()
    );

    let config_path = paths.config_file();
    let config = Config::load_or_default(&config_path)?;
    if !config_path.exists() {
        // Give the user a file to edit; failure to write it is not fatal.
        if let Err(e) = config.save(&config_path) {
            tracing::warn!("cannot write default config: {e}");
        }
    }

    let system_locale = locale::system_locale();
    let translator = Translator::new(&config.general.language, &system_locale, &paths.lang_dir());
    let hotkeys = config.hotkeys.parsed()?;

    // Capture + replay buffer. A failure (no GPU, blocked capture) must not kill the tray.
    let pipeline = if config.replay.enabled {
        match Pipeline::start(PipelineSettings {
            fps: config.video.fps,
            bitrate_bps: config.video.bitrate_kbps.saturating_mul(1000),
            replay_secs: config.replay.duration_secs,
            audio_system: config.audio.system,
            audio_mic: config.audio.microphone,
            audio_bitrate_bps: config.audio.bitrate_kbps.saturating_mul(1000),
        }) {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                tracing::error!("cannot start capture: {e}");
                None
            }
        }
    } else {
        None
    };
    let clips_folder = clips_dir(&config, &paths);
    let library = match Library::open(&paths.data_dir().join("library.db")) {
        Ok(lib) => {
            // Pick up clips that are already in the folder and forget deleted ones.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            match lib.scan_folder(&clips_folder, now) {
                Ok(n) if n > 0 => tracing::info!("library: imported {n} existing clips"),
                Ok(_) => {}
                Err(e) => tracing::warn!("library scan failed: {e}"),
            }
            if let Err(e) = lib.prune_missing() {
                tracing::warn!("library prune failed: {e}");
            }
            Some(Mutex::new(lib))
        }
        Err(e) => {
            tracing::warn!("library unavailable: {e}");
            None
        }
    };
    let saver = Arc::new(ClipSaver {
        pipeline: pipeline.clone(),
        dir: clips_folder,
        secs: config.replay.duration_secs,
        library,
        games: config
            .games
            .detect
            .then(|| GameDetector::start(config.games.ignore.clone())),
    });

    let launcher = Arc::new(UiLauncher {
        exe: paths.exe_dir().join(UI_EXE),
        locale: system_locale,
        child: Mutex::new(None),
    });

    // IPC server on its own thread.
    let started = Instant::now();
    let portable = paths.is_portable();
    let handler_launcher = Arc::clone(&launcher);
    let handler_saver = Arc::clone(&saver);
    let replay_active = pipeline.is_some();
    let handler: pipe::Handler = Arc::new(move |request| match request {
        Request::Ping => Response::Pong,
        Request::GetStatus => {
            let mem = WinMemoryProbe.current_process_memory();
            Response::Status(ServiceStatus {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                uptime_secs: started.elapsed().as_secs(),
                replay_active,
                portable,
                working_set_kb: mem.map(|m| m.working_set_kb),
                private_kb: mem.map(|m| m.private_kb),
            })
        }
        Request::OpenUi => match handler_launcher.open() {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error {
                message: format!("{e:#}"),
            },
        },
        Request::SaveClip => match handler_saver.save() {
            Ok(path) => Response::Saved {
                path: path.display().to_string(),
            },
            Err(e) => Response::Error {
                message: format!("{e:#}"),
            },
        },
        Request::ToggleRecording => match handler_saver.toggle_recording() {
            Ok(Some(path)) => Response::Saved {
                path: path.display().to_string(),
            },
            Ok(None) => Response::Ok,
            Err(e) => Response::Error {
                message: format!("{e:#}"),
            },
        },
        Request::Shutdown => {
            tray::request_quit(TRAY_WINDOW.load(Ordering::Relaxed));
            Response::Ok
        }
    });
    let server_pipe = pipe_name.clone();
    std::thread::Builder::new()
        .name("hc-pipe-server".into())
        .spawn(move || {
            if let Err(e) = pipe::serve(&server_pipe, handler) {
                tracing::error!("IPC server stopped: {e}");
            }
        })
        .context("cannot start the IPC server thread")?;

    let labels = TrayLabels {
        tooltip: translator.t("tray_tooltip").to_owned(),
        open: translator.t("tray_open").to_owned(),
        save_clip: translator.t("tray_save_clip").to_owned(),
        toggle_recording: translator.t("tray_toggle_recording").to_owned(),
        check_updates: translator.t("tray_check_updates").to_owned(),
        quit: translator.t("tray_quit").to_owned(),
    };

    let updater = Arc::new(UpdateCtx::new(
        paths.clone(),
        config_path.clone(),
        config.clone(),
        Translator::new(
            &config.general.language,
            &locale::system_locale(),
            &paths.lang_dir(),
        ),
        Box::new(|| tray::request_quit(TRAY_WINDOW.load(Ordering::Relaxed))),
    ));
    if config.privacy.update_check == UpdateCheck::OnStart {
        updater.spawn_check(false);
    }
    let event_updater = Arc::clone(&updater);
    let event_launcher = Arc::clone(&launcher);
    let event_saver = Arc::clone(&saver);
    let on_event = move |event: TrayEvent| match event {
        TrayEvent::Open | TrayEvent::Hotkey(HotkeyAction::OpenUi) => {
            if let Err(e) = event_launcher.open() {
                tracing::error!("cannot open the UI: {e:#}");
            }
        }
        TrayEvent::SaveClip | TrayEvent::Hotkey(HotkeyAction::SaveClip) => {
            // Muxing takes a moment: keep the tray's message loop free.
            let saver = Arc::clone(&event_saver);
            let spawned = std::thread::Builder::new()
                .name("hc-save".into())
                .spawn(move || match saver.save() {
                    Ok(path) => {
                        tracing::info!("clip saved: {}", path.display());
                        sound::beep_ok();
                    }
                    Err(e) => {
                        tracing::error!("cannot save clip: {e:#}");
                        sound::beep_error();
                    }
                });
            if let Err(e) = spawned {
                tracing::error!("cannot start the save thread: {e}");
            }
        }
        TrayEvent::ToggleRecording | TrayEvent::Hotkey(HotkeyAction::ToggleRecording) => {
            let saver = Arc::clone(&event_saver);
            let spawned = std::thread::Builder::new()
                .name("hc-record".into())
                .spawn(move || match saver.toggle_recording() {
                    Ok(Some(path)) => {
                        tracing::info!("recording saved: {}", path.display());
                        sound::beep_ok();
                    }
                    Ok(None) => {
                        tracing::info!("recording started");
                        sound::beep_ok();
                    }
                    Err(e) => {
                        tracing::error!("recording toggle failed: {e:#}");
                        sound::beep_error();
                    }
                });
            if let Err(e) = spawned {
                tracing::error!("cannot start the recording thread: {e}");
            }
        }
        TrayEvent::CheckUpdates => event_updater.spawn_check(true),
        TrayEvent::Hotkey(HotkeyAction::Screenshot) => {
            let saver = Arc::clone(&event_saver);
            let spawned = std::thread::Builder::new()
                .name("hc-shot".into())
                .spawn(move || match saver.screenshot() {
                    Ok(path) => {
                        tracing::info!("screenshot saved: {}", path.display());
                        sound::beep_ok();
                    }
                    Err(e) => {
                        tracing::error!("screenshot failed: {e:#}");
                        sound::beep_error();
                    }
                });
            if let Err(e) = spawned {
                tracing::error!("cannot start the screenshot thread: {e}");
            }
        }
    };

    tray::run(labels, &hotkeys, on_event, |window| {
        TRAY_WINDOW.store(window, Ordering::Relaxed);
    })?;

    if let Some(p) = &pipeline {
        p.shutdown();
    }
    tracing::info!("Hyper Clips stopped");
    Ok(())
}
