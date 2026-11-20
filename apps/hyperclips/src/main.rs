//! Hyper Clips core process: tray icon, global hotkeys, IPC server and the
//! capture/replay service. Uses only OS media APIs (DXGI, D3D11, Media
//! Foundation, WASAPI): no GUI toolkit, third-party codec or network stack.

#![windows_subsystem = "windows"]

mod capture;
mod cli;
mod core_app;
mod indexer;
mod ipc_server;
mod overlay;
mod studio;
mod tray;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // A GUI-subsystem exe has no console: borrow the parent's so CLI output is visible.
    if std::env::args_os().len() > 1 {
        cli::attach_parent_console();
    }
    let args = cli::Args::parse();
    match args.command() {
        Some(cmd) => cli::run(cmd),
        None => core_app::run(args.background),
    }
}
