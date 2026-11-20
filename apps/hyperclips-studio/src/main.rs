//! Hyper Clips studio: the window (library, editor, settings).
//! Started on demand by the core; exiting it releases all UI memory.

#![windows_subsystem = "windows"]

mod app;
mod core_link;
mod settings;
mod theme;

use std::time::Duration;

use hc_core::config::Config;
use hc_core::ipc::{IpcError, Request, one_shot};
use hc_core::paths::AppPaths;
use hc_core::sys::{find_processes, focus_process_window};
use hc_core::{CORE_EXE, STUDIO_EXE};

fn main() -> eframe::Result {
    // Single instance: focus the existing window instead of opening another.
    let me = std::process::id();
    if let Some(pid) = find_processes(STUDIO_EXE).into_iter().find(|p| *p != me) {
        focus_process_window(pid);
        return Ok(());
    }

    let paths = match AppPaths::detect() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Hyper Clips: {e}");
            return Ok(());
        }
    };
    let _ = paths.ensure();
    let config = Config::load(&paths.config_file()).unwrap_or_default();
    let _log = hc_core::logging::init(&paths.log_dir, "studio", config.general.log_level).ok();
    tracing::info!(version = hc_core::VERSION, "studio starting");

    ensure_core_running(&paths);

    let icon = eframe::egui::IconData {
        rgba: hc_core::brand::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(hc_core::APP_NAME)
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(icon),
        ..Default::default()
    };
    let result = eframe::run_native(
        hc_core::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, paths, config)))),
    );
    tracing::info!("studio closed");
    result
}

/// The studio is useless without the core: start it in the background if needed.
fn ensure_core_running(paths: &AppPaths) {
    match one_shot(&Request::Ping, Duration::from_millis(500)) {
        Ok(_) => {}
        Err(IpcError::NotRunning) => {
            let exe = paths.sibling_exe(CORE_EXE);
            match std::process::Command::new(&exe).arg("--background").spawn() {
                Ok(c) => tracing::info!(pid = c.id(), "started core"),
                Err(e) => tracing::error!(error = %e, "cannot start core"),
            }
        }
        Err(e) => tracing::warn!(error = %e, "core ping failed"),
    }
}
