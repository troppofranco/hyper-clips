//! Hyper Clips core process: tray icon, global hotkeys, IPC server and
//! (from Phase 2) the capture/replay service. Deliberately free of any GUI
//! toolkit, media framework or network stack to keep its footprint small.

#![windows_subsystem = "windows"]

mod cli;
mod core_app;
mod ipc_server;
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
