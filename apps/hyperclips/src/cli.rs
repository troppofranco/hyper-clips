//! Command-line interface: talks to the running core over the local pipe.
//!
//! Exit codes: 0 ok, 1 error, 2 feature unavailable, 3 core not running.

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use hc_core::ipc::{Action, IpcError, Request, Response, one_shot};

#[derive(Debug, Parser)]
#[command(
    name = "hyperclips",
    version,
    about = "Hyper Clips — lightweight, private game clipping"
)]
#[command(group = clap::ArgGroup::new("cmd").multiple(false))]
pub struct Args {
    /// Start the core in the tray without opening the window (used by autostart).
    #[arg(long)]
    pub background: bool,
    /// Save the Instant Replay buffer as a clip.
    #[arg(long, group = "cmd")]
    save_clip: bool,
    /// Start or stop a full recording.
    #[arg(long, group = "cmd")]
    toggle_recording: bool,
    /// Take a screenshot.
    #[arg(long, group = "cmd")]
    screenshot: bool,
    /// Add a marker to the current recording.
    #[arg(long, group = "cmd")]
    marker: bool,
    /// Open the Hyper Clips window.
    #[arg(long, group = "cmd")]
    open: bool,
    /// Print the status of the core service.
    #[arg(long, group = "cmd")]
    status: bool,
    /// List clips in the library.
    #[arg(long, group = "cmd")]
    list: bool,
    /// Export a clip with a preset (e.g. --export 42 --preset discord).
    #[arg(long, group = "cmd", value_name = "CLIP_ID")]
    export: Option<String>,
    /// Export preset name.
    #[arg(long, requires = "export")]
    preset: Option<String>,
    /// Quit the core service.
    #[arg(long, group = "cmd")]
    quit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Request(Request),
    /// Recognised but provided by a later phase.
    NotYet(&'static str),
}

impl Args {
    pub fn command(&self) -> Option<Command> {
        let run = |a| Some(Command::Request(Request::Run(a)));
        if self.save_clip {
            run(Action::SaveClip)
        } else if self.toggle_recording {
            run(Action::ToggleRecording)
        } else if self.screenshot {
            run(Action::Screenshot)
        } else if self.marker {
            run(Action::AddMarker)
        } else if self.open {
            run(Action::OpenStudio)
        } else if self.status {
            Some(Command::Request(Request::Status))
        } else if self.quit {
            Some(Command::Request(Request::Shutdown))
        } else if self.list {
            Some(Command::NotYet("--list (library, phase 3)"))
        } else if self.export.is_some() {
            Some(Command::NotYet("--export (editor, phase 4)"))
        } else {
            None
        }
    }
}

pub fn run(cmd: Command) -> ExitCode {
    let req = match cmd {
        Command::NotYet(what) => {
            eprintln!("not available yet: {what}");
            return ExitCode::from(2);
        }
        Command::Request(r) => r,
    };
    match one_shot(&req, Duration::from_secs(3)) {
        Ok(Response::Status(s)) => {
            println!("Hyper Clips {} ({:?}), pid {}", s.version, s.mode, s.pid);
            println!("uptime: {} s", s.uptime_secs);
            println!(
                "memory: working set {}, private {}",
                hc_core::sys::format_bytes(s.working_set_bytes),
                hc_core::sys::format_bytes(s.private_bytes)
            );
            println!(
                "replay buffer: {} (working set excluding buffer: {})",
                hc_core::sys::format_bytes(s.buffer_bytes),
                hc_core::sys::format_bytes(s.working_set_bytes.saturating_sub(s.buffer_bytes))
            );
            println!("replay: {}, recording: {}", s.replay_active, s.recording);
            for c in &s.hotkey_conflicts {
                println!(
                    "hotkey conflict: {} = {} ({})",
                    c.action, c.binding, c.reason
                );
            }
            ExitCode::SUCCESS
        }
        Ok(Response::Ok | Response::Pong { .. } | Response::Reloaded { .. }) => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Ok(Response::Unavailable(msg)) => {
            eprintln!("unavailable: {msg}");
            ExitCode::from(2)
        }
        Ok(Response::Error(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
        Err(IpcError::NotRunning) => {
            eprintln!("Hyper Clips is not running");
            ExitCode::from(3)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: failure (no parent console, e.g. Explorer launch) is harmless.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("hyperclips").chain(args.iter().copied()))
    }

    #[test]
    fn no_args_starts_core() {
        let a = parse(&[]).unwrap();
        assert_eq!(a.command(), None);
        assert!(!a.background);
    }

    #[test]
    fn maps_flags_to_requests() {
        assert_eq!(
            parse(&["--save-clip"]).unwrap().command(),
            Some(Command::Request(Request::Run(Action::SaveClip)))
        );
        assert_eq!(
            parse(&["--status"]).unwrap().command(),
            Some(Command::Request(Request::Status))
        );
        assert!(matches!(
            parse(&["--export", "3", "--preset", "discord"])
                .unwrap()
                .command(),
            Some(Command::NotYet(_))
        ));
    }

    #[test]
    fn commands_are_mutually_exclusive() {
        assert!(parse(&["--save-clip", "--quit"]).is_err());
        assert!(parse(&["--preset", "x"]).is_err());
    }
}
