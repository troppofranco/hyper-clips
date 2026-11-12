//! Service wiring (Windows only).

use anyhow::{Context, Result};
use hc_core::config::Config;
use hc_core::consts::{current_user, pipe_path, UI_EXE};
use hc_core::error::Error as CoreError;
use hc_core::hotkey::HotkeyAction;
use hc_core::i18n::Translator;
use hc_core::ipc::{send_request, Request, Response, ServiceStatus};
use hc_core::paths::AppPaths;
use hc_core::platform::MemoryProbe;
use hc_os_win::tray::{self, TrayEvent, TrayLabels};
use hc_os_win::{instance, locale, memory::WinMemoryProbe, pipe};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

    let launcher = Arc::new(UiLauncher {
        exe: paths.exe_dir().join(UI_EXE),
        locale: system_locale,
        child: Mutex::new(None),
    });

    // IPC server on its own thread.
    let started = Instant::now();
    let portable = paths.is_portable();
    let handler_launcher = Arc::clone(&launcher);
    let handler: pipe::Handler = Arc::new(move |request| match request {
        Request::Ping => Response::Pong,
        Request::GetStatus => {
            let mem = WinMemoryProbe.current_process_memory();
            Response::Status(ServiceStatus {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                uptime_secs: started.elapsed().as_secs(),
                // Capture is not implemented yet (Phase 2), so the buffer is never active.
                replay_active: false,
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
        Request::SaveClip | Request::ToggleRecording => Response::Error {
            message: "capture is not implemented yet (Phase 2)".to_owned(),
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
        quit: translator.t("tray_quit").to_owned(),
    };

    let event_launcher = Arc::clone(&launcher);
    let on_event = move |event: TrayEvent| match event {
        TrayEvent::Open | TrayEvent::Hotkey(HotkeyAction::OpenUi) => {
            if let Err(e) = event_launcher.open() {
                tracing::error!("cannot open the UI: {e:#}");
            }
        }
        TrayEvent::SaveClip | TrayEvent::Hotkey(HotkeyAction::SaveClip) => {
            tracing::info!("save clip requested (capture arrives in Phase 2)");
        }
        TrayEvent::ToggleRecording | TrayEvent::Hotkey(HotkeyAction::ToggleRecording) => {
            tracing::info!("toggle recording requested (capture arrives in Phase 2)");
        }
        TrayEvent::Hotkey(HotkeyAction::Screenshot) => {
            tracing::info!("screenshot requested (arrives in Phase 2)");
        }
    };

    tray::run(labels, &hotkeys, on_event, |window| {
        TRAY_WINDOW.store(window, Ordering::Relaxed);
    })?;

    tracing::info!("Hyper Clips stopped");
    Ok(())
}
