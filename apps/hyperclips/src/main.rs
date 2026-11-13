//! Hyper Clips background service: tray, global hotkeys, IPC server.
//! It never links a UI toolkit; the UI is a separate process started on demand.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod pipeline;
#[cfg(windows)]
mod service;

#[cfg(windows)]
fn main() {
    if let Err(err) = service::run() {
        // The app has no console; a dialog is the only way to show an early failure.
        hc_os_win::dialog::show_error("Hyper Clips", &format!("{err:#}"));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "Hyper Clips runs on Windows only; this build exists to keep the workspace portable."
    );
    std::process::exit(1);
}
