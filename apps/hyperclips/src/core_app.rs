//! Core process lifecycle: startup, window procedure, action dispatch.

use std::cell::RefCell;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use hc_core::config::Config;
use hc_core::i18n::Localizer;
use hc_core::ipc::{Action, CoreStatus, HotkeyConflict, Request, Response, one_shot};
use hc_core::paths::{AppPaths, InstallMode};
use hc_core::sys::{self, SingleInstance};
use hc_core::{CORE_EXE, STUDIO_EXE, UPDATER_HELPER_EXE};
use hc_hotkeys::Registrar;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

// shellapi.h: NIN_SELECT = WM_USER, NIN_KEYSELECT = NIN_SELECT | NINF_KEY.
const NIN_SELECT: u32 = 0x0400;
const NIN_KEYSELECT: u32 = 0x0401;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG, PostQuitMessage,
    RegisterWindowMessageW, SetTimer, TranslateMessage, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY,
    WM_INPUT, WM_RBUTTONUP, WM_TIMER,
};
use windows::core::w;

use crate::capture::CaptureCtl;
use crate::ipc_server::{Envelope, IpcServer};
use crate::studio;
use crate::tray::{MenuCmd, Tray, WM_APP_CAPTURE, WM_APP_IPC, WM_APP_TRAY};
use hc_recorder::Event;

/// Game detection interval (ms).
const DETECT_TIMER_ID: usize = 1;
const DETECT_INTERVAL_MS: u32 = 3000;

const INSTANCE_MUTEX: &str = "HyperClips.Core";

/// Main-thread state. Accessed only from the window procedure.
struct App {
    paths: AppPaths,
    config: Config,
    i18n: Localizer,
    tray: Tray,
    hotkeys: Registrar,
    conflicts: Vec<HotkeyConflict>,
    ipc_rx: Receiver<Envelope>,
    started: Instant,
    taskbar_created: u32,
    capture: CaptureCtl,
    overlay: Option<crate::overlay::Overlay>,
    indexer: crate::indexer::Indexer,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn run(background: bool, health_file: Option<std::path::PathBuf>) -> ExitCode {
    let t0 = Instant::now();
    // SAFETY: called before any window exists.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let paths = match AppPaths::detect().and_then(|p| p.ensure().map(|()| p)) {
        Ok(p) => p,
        Err(e) => return fatal(&format!("cannot prepare data folders: {e}")),
    };
    let (config, config_error) = match Config::load(&paths.config_file()) {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e.to_string())),
    };
    let _log = hc_core::logging::init(&paths.log_dir, "core", config.general.log_level).ok();
    tracing::info!(
        version = hc_core::VERSION,
        mode = ?paths.mode,
        config = %hc_core::paths::redact(&paths.config_file()),
        "core starting"
    );
    if let Some(err) = config_error {
        tracing::error!(%err, "config unreadable, using defaults (file left untouched)");
    }

    let Some(_instance) = SingleInstance::acquire(INSTANCE_MUTEX) else {
        tracing::info!("another core is running; asking it to open the studio");
        let _ = one_shot(&Request::Run(Action::OpenStudio), Duration::from_secs(3));
        return ExitCode::SUCCESS;
    };

    apply_system_settings(&paths, &config);

    let hwnd = match crate::tray::create_window(Some(wndproc)) {
        Ok(h) => h,
        Err(e) => return fatal(&format!("cannot create window: {e}")),
    };
    let (ipc_tx, ipc_rx) = unbounded();
    let mut ipc = match IpcServer::start(hwnd, ipc_tx) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::error!(error = %e, "IPC server unavailable (CLI and studio control disabled)");
            None
        }
    };

    let i18n = Localizer::new(&config.general.language, Some(&paths.exe_dir));
    let mut app = App {
        tray: Tray::new(hwnd),
        hotkeys: Registrar::new(hwnd),
        conflicts: Vec::new(),
        ipc_rx,
        started: t0,
        capture: CaptureCtl::new(hwnd, &paths),
        indexer: crate::indexer::Indexer::start(&paths),
        overlay: crate::overlay::Overlay::new()
            .inspect_err(|e| tracing::warn!(error = %e, "in-game overlay unavailable"))
            .ok(),
        // SAFETY: constant string.
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        paths,
        config,
        i18n,
    };
    app.apply_hotkeys();
    let tip = app.tooltip();
    app.tray.add(&tip);
    if !app.conflicts.is_empty() {
        app.toast(&app.i18n.tr("notify-hotkey-conflicts"), true);
    }
    tracing::info!(
        startup_ms = sys::ms_since_process_start(),
        init_ms = t0.elapsed().as_millis() as u64,
        "tray ready"
    );
    app.capture.tick(&app.config);
    app.capture.sync(&app.config);
    // SAFETY: timer owned by our window, killed on destroy.
    unsafe {
        SetTimer(Some(hwnd), DETECT_TIMER_ID, DETECT_INTERVAL_MS, None);
    }
    if let Some(h) = &health_file {
        let _ = std::fs::write(h, b"ok");
    }
    crate::updates::start(&app.paths, &app.config, hwnd);
    if !background && let Err(e) = studio::open(&app.paths) {
        tracing::warn!(error = %e, "could not open studio");
    }
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    message_loop();

    if let Some(s) = ipc.as_mut() {
        s.stop();
    }
    // Dropping the app stops capture and finalises any running recording.
    APP.with(|cell| cell.borrow_mut().take());
    tracing::info!("core stopped");
    ExitCode::SUCCESS
}

fn message_loop() {
    let mut msg = MSG::default();
    // SAFETY: standard Win32 message pump on the thread owning the window.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn apply_system_settings(paths: &AppPaths, config: &Config) {
    // Portable mode never writes outside its own folder (no registry).
    if paths.mode != InstallMode::Installed {
        return;
    }
    if config.privacy.disable_windows_error_reporting {
        sys::exclude_from_wer(&[CORE_EXE, STUDIO_EXE, UPDATER_HELPER_EXE]);
    }
    if sys::autostart_enabled() != config.general.start_with_windows {
        let exe = paths.sibling_exe(CORE_EXE);
        if let Err(e) = sys::set_autostart(config.general.start_with_windows, &exe) {
            tracing::warn!(error = %e, "cannot update autostart entry");
        }
    }
}

fn fatal(msg: &str) -> ExitCode {
    tracing::error!(msg);
    eprintln!("Hyper Clips: {msg}");
    ExitCode::FAILURE
}

impl App {
    fn apply_hotkeys(&mut self) {
        let entries = self.config.hotkeys.entries();
        self.conflicts = self
            .hotkeys
            .apply(&entries)
            .into_iter()
            .map(|c| HotkeyConflict {
                action: c.action,
                binding: c.binding,
                reason: c.reason,
            })
            .collect();
        for c in &self.conflicts {
            tracing::warn!(action = %c.action, binding = %c.binding, reason = %c.reason, "hotkey conflict");
        }
    }

    fn tooltip(&self) -> String {
        let state = if self.config.capture.replay_enabled {
            "state-on"
        } else {
            "state-off"
        };
        self.i18n
            .tr_args("tray-tooltip", &[("replay", &self.i18n.tr(state))])
    }

    fn toast(&self, text: &str, warning: bool) {
        let n = &self.config.notifications;
        if !n.enabled {
            return;
        }
        // Over the game when it is in front; Windows toast otherwise.
        if n.in_game
            && let (Some(ov), Some(anchor)) = (&self.overlay, self.capture.game_in_front())
        {
            ov.show(text, n.position, anchor, self.capture.is_recording());
            if n.sound {
                // SAFETY: plain system sound request.
                unsafe {
                    let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(
                        windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
                    );
                }
            }
            return;
        }
        self.tray.notify(hc_core::APP_NAME, text, warning, n.sound);
    }

    fn menu_items(&self) -> Vec<(MenuCmd, String)> {
        vec![
            (MenuCmd::Open, self.i18n.tr("tray-open")),
            (MenuCmd::SaveClip, self.i18n.tr("tray-save-clip")),
            (
                MenuCmd::ToggleRecording,
                self.i18n.tr("tray-toggle-recording"),
            ),
            (MenuCmd::Screenshot, self.i18n.tr("tray-screenshot")),
            (MenuCmd::Quit, self.i18n.tr("tray-quit")),
        ]
    }

    fn run_action(&mut self, action: Action) -> Response {
        tracing::info!(action = action.id(), "action");
        match action {
            Action::OpenStudio => match studio::open(&self.paths) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e),
            },
            Action::SaveClip => {
                let r = self.capture.save_clip(&self.config, &self.paths);
                self.action_result(action, r)
            }
            Action::ToggleRecording => {
                let r = self
                    .capture
                    .toggle_recording(&self.config, &self.paths)
                    .map(|_| ());
                self.action_result(action, r)
            }
            Action::Screenshot => {
                let r = self.capture.screenshot(&self.config, &self.paths);
                self.action_result(action, r)
            }
            Action::AddMarker => {
                let r = self.capture.add_marker();
                self.action_result(action, r)
            }
        }
    }

    fn action_result(&self, action: Action, r: Result<(), String>) -> Response {
        match r {
            Ok(()) => Response::Ok,
            Err(reason) => {
                let label = self.i18n.tr(&format!("action-{}", action.id()));
                let msg = self.i18n.tr_args(
                    "notify-unavailable",
                    &[("action", &label), ("reason", &reason)],
                );
                self.toast(&msg, true);
                Response::Unavailable(reason)
            }
        }
    }

    fn on_capture_events(&mut self) {
        while let Ok(ev) = self.capture.events.try_recv() {
            // Every saved file goes into the library (in the background).
            if let Event::ClipSaved { path, .. }
            | Event::RecordingStopped { path, .. }
            | Event::ScreenshotSaved { path } = &ev
            {
                self.indexer.submit(crate::indexer::Job {
                    path: path.clone(),
                    game: self.capture.game.as_ref().map(|g| g.name.clone()),
                    limit_bytes: self.config.library.limit_bytes(),
                });
            }
            match ev {
                Event::Started {
                    encoder,
                    hardware,
                    width,
                    height,
                    monitor,
                } => {
                    tracing::info!(%encoder, hardware, width, height, %monitor, "capture running");
                    self.capture.encoder = Some(encoder);
                }
                Event::ClipSaved { path, seconds } => {
                    tracing::info!(path = %hc_core::paths::redact(&path), seconds, "clip saved");
                    let game = self
                        .capture
                        .game
                        .as_ref()
                        .map_or("Desktop", |g| g.name.as_str())
                        .to_owned();
                    let msg = self.i18n.tr_args(
                        "notify-clip-saved",
                        &[("seconds", &format!("{seconds:.0}")), ("game", &game)],
                    );
                    self.toast(&msg, false);
                }
                Event::RecordingStarted { .. } => {
                    self.toast(&self.i18n.tr("notify-recording-started"), false);
                    self.tray.set_tooltip(&self.tooltip());
                }
                Event::RecordingStopped {
                    path,
                    seconds,
                    markers,
                } => {
                    tracing::info!(path = %hc_core::paths::redact(&path), seconds, markers, "recording saved");
                    let total = seconds as u64;
                    let duration = format!("{}:{:02}", total / 60, total % 60);
                    let msg = self.i18n.tr_args(
                        "notify-recording-stopped",
                        &[("duration", &duration), ("markers", &markers.to_string())],
                    );
                    self.toast(&msg, false);
                    self.tray.set_tooltip(&self.tooltip());
                    // Free capture resources if replay is off.
                    self.capture.sync(&self.config);
                }
                Event::MarkerAdded { .. } => self.toast(&self.i18n.tr("notify-marker"), false),
                Event::ScreenshotSaved { path } => {
                    tracing::info!(path = %hc_core::paths::redact(&path), "screenshot saved");
                    self.toast(&self.i18n.tr("notify-screenshot"), false);
                }
                Event::Error(e) => {
                    tracing::warn!(error = %e, "capture error");
                    self.toast(&self.i18n.tr_args("notify-error", &[("error", &e)]), true);
                }
            }
        }
    }

    fn reload_config(&mut self) -> Response {
        match Config::load(&self.paths.config_file()) {
            Ok(cfg) => {
                let lang_changed = cfg.general.language != self.config.general.language;
                if lang_changed {
                    self.i18n =
                        Localizer::new(&self.config.general.language, Some(&self.paths.exe_dir));
                }
                let capture_changed =
                    cfg.capture != self.config.capture || cfg.audio != self.config.audio;
                let only_duration = capture_changed && {
                    let mut a = cfg.capture.clone();
                    a.replay_seconds = self.config.capture.replay_seconds;
                    a == self.config.capture && cfg.audio == self.config.audio
                };
                self.config = cfg;
                apply_system_settings(&self.paths, &self.config);
                self.apply_hotkeys();
                if only_duration {
                    self.capture
                        .set_replay_seconds(self.config.capture.replay_seconds);
                } else if capture_changed && !self.capture.is_recording() {
                    self.capture.stop();
                }
                self.capture.sync(&self.config);
                self.tray.set_tooltip(&self.tooltip());
                tracing::info!("config reloaded");
                Response::Reloaded {
                    conflicts: self.conflicts.clone(),
                }
            }
            Err(e) => Response::Error(e.to_string()),
        }
    }

    fn status(&self) -> CoreStatus {
        let mem = sys::current_memory();
        CoreStatus {
            version: hc_core::VERSION.into(),
            mode: self.paths.mode,
            pid: std::process::id(),
            uptime_secs: self.started.elapsed().as_secs(),
            recording: self.capture.is_recording(),
            replay_active: self.capture.is_running() && self.config.capture.replay_enabled,
            working_set_bytes: mem.working_set,
            private_bytes: mem.private_bytes,
            buffer_bytes: self.capture.buffer_bytes() as u64,
            hotkey_conflicts: self.conflicts.clone(),
        }
    }

    /// Execute queued IPC requests. Returns true if the core should exit.
    fn drain_ipc(&mut self) -> bool {
        let mut quit = false;
        while let Ok(env) = self.ipc_rx.try_recv() {
            let response = match env.request {
                Request::Ping => Response::Pong {
                    version: hc_core::VERSION.into(),
                },
                Request::Status => Response::Status(self.status()),
                Request::Run(a) => self.run_action(a),
                Request::ReloadConfig => self.reload_config(),
                Request::Shutdown => {
                    quit = true;
                    Response::Ok
                }
            };
            let _ = env.reply.send(response);
        }
        quit
    }
}

static PENDING_UPDATE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
const WM_APP_UPDATE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;

/// Called from the update thread: wake the UI thread to show a toast.
pub fn post_update_available(hwnd: isize, version: &str) {
    if let Ok(mut p) = PENDING_UPDATE.lock() {
        *p = Some(version.to_owned());
    }
    // SAFETY: PostMessageW is thread-safe.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(HWND(hwnd as *mut _)),
            WM_APP_UPDATE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Run `f` with the app state if it is not already borrowed (re-entrancy guard).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            with_app(|app| {
                let action = app.hotkeys.on_hotkey(wparam).and_then(Action::from_id);
                if let Some(a) = action {
                    app.run_action(a);
                }
            });
            LRESULT(0)
        }
        WM_INPUT => {
            with_app(|app| {
                let action = app.hotkeys.on_raw_input(lparam).and_then(Action::from_id);
                if let Some(a) = action {
                    app.run_action(a);
                }
            });
            // SAFETY: WM_INPUT must reach DefWindowProc for cleanup.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: event in LOWORD(lParam).
            let event = (lparam.0 as u32) & 0xFFFF;
            match event {
                e if e == WM_CONTEXTMENU || e == WM_RBUTTONUP => {
                    // Build items, release the borrow, then run the nested menu loop.
                    if let Some(items) = with_app(|app| app.menu_items())
                        && let Some(cmd) = Tray::show_menu(hwnd, &items)
                    {
                        on_menu(hwnd, cmd);
                    }
                }
                e if e == NIN_SELECT || e == NIN_KEYSELECT => {
                    with_app(|app| app.run_action(Action::OpenStudio));
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_IPC => {
            if with_app(App::drain_ipc).unwrap_or(false) {
                // SAFETY: our own window.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            let v = PENDING_UPDATE.lock().ok().and_then(|mut p| p.take());
            if let Some(v) = v {
                with_app(|app| {
                    let msg = app.i18n.tr_args("notify-update", &[("version", &v)]);
                    app.tray.notify(hc_core::APP_NAME, &msg, false, false);
                });
            }
            LRESULT(0)
        }
        WM_APP_CAPTURE => {
            with_app(App::on_capture_events);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == DETECT_TIMER_ID => {
            with_app(|app| app.capture.tick(&app.config));
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: our own timer.
            unsafe {
                let _ = KillTimer(Some(hwnd), DETECT_TIMER_ID);
            }
            with_app(|app| {
                app.capture.stop();
                app.tray.remove();
            });
            // SAFETY: ends the message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m => {
            let recreated = with_app(|app| {
                if app.taskbar_created != 0 && m == app.taskbar_created {
                    let tip = app.tooltip();
                    app.tray.add(&tip);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if recreated {
                return LRESULT(0);
            }
            // SAFETY: default handling for everything else.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn on_menu(hwnd: HWND, cmd: MenuCmd) {
    let action = match cmd {
        MenuCmd::Open => Action::OpenStudio,
        MenuCmd::SaveClip => Action::SaveClip,
        MenuCmd::ToggleRecording => Action::ToggleRecording,
        MenuCmd::Screenshot => Action::Screenshot,
        MenuCmd::Quit => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return;
        }
    };
    with_app(|app| app.run_action(action));
}
