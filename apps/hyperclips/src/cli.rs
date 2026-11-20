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
    /// Written once the core is up (used by the updater's health check).
    #[arg(long, hide = true)]
    pub health_file: Option<std::path::PathBuf>,
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
    /// Export preset: discord, youtube, shorts, twitter, max, gif, webp.
    #[arg(long, requires = "export")]
    preset: Option<String>,
    /// Detect highlights (loud moments) in a clip and add them as markers.
    #[arg(long, group = "cmd", value_name = "CLIP_ID")]
    highlights: Option<i64>,
    /// Save settings (config and custom game list) to a backup file.
    #[arg(long, group = "cmd", value_name = "FILE")]
    backup_settings: Option<std::path::PathBuf>,
    /// Restore settings from a backup file (current ones are kept as .bak).
    #[arg(long, group = "cmd", value_name = "FILE")]
    restore_settings: Option<std::path::PathBuf>,
    /// Quit the core service.
    #[arg(long, group = "cmd")]
    quit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Request(Request),
    /// Read the library directly (works without the core running).
    List,
    /// Queue an export of a library item with a preset.
    Export {
        id: i64,
        preset: String,
    },
    Highlights(i64),
    Backup(std::path::PathBuf),
    Restore(std::path::PathBuf),
    /// Invalid argument value.
    Invalid(String),
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
            Some(Command::List)
        } else if let Some(id) = &self.export {
            Some(match id.trim().parse() {
                Ok(id) => Command::Export {
                    id,
                    preset: self.preset.clone().unwrap_or_else(|| "discord".into()),
                },
                Err(_) => Command::Invalid(format!("not a clip id: {id}")),
            })
        } else if let Some(id) = self.highlights {
            Some(Command::Highlights(id))
        } else if let Some(p) = &self.backup_settings {
            Some(Command::Backup(p.clone()))
        } else {
            self.restore_settings.clone().map(Command::Restore)
        }
    }
}

pub fn run(cmd: Command) -> ExitCode {
    let req = match cmd {
        Command::List => return list(),
        Command::Export { id, preset } => return export(id, &preset),
        Command::Highlights(id) => return highlights(id),
        Command::Backup(p) => return backup(&p),
        Command::Restore(p) => return restore(&p),
        Command::Invalid(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
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

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::FAILURE
}

fn open_library() -> Result<(hc_core::paths::AppPaths, hc_library::Library), String> {
    let paths = hc_core::paths::AppPaths::detect().map_err(|e| e.to_string())?;
    let lib = hc_library::Library::open(&paths.library_db()).map_err(|e| e.to_string())?;
    Ok((paths, lib))
}

fn export(id: i64, preset: &str) -> ExitCode {
    use hc_editor::{Preset, Project, Segment, TrackMix};
    let Some(pr) = Preset::ALL
        .into_iter()
        .find(|p| p.id().eq_ignore_ascii_case(preset))
    else {
        let names: Vec<&str> = Preset::ALL.iter().map(|p| p.id()).collect();
        return fail(format!(
            "unknown preset {preset} (use one of: {})",
            names.join(", ")
        ));
    };
    let (paths, lib) = match open_library() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let item = match lib.get(id) {
        Ok(Some(i)) if i.kind != hc_library::Kind::Screenshot => i,
        Ok(_) => return fail(format!("no video with id {id}")),
        Err(e) => return fail(e),
    };
    let mut project = Project::default();
    project
        .segments
        .push(Segment::new(item.path.clone(), item.duration_ms));
    project.tracks = (0..hc_editor::decode::audio_stream_count(&item.path))
        .map(|k| TrackMix {
            volume: 1.0,
            muted: k != 0,
        })
        .collect();
    pr.apply(&mut project.output, &mut project.framing);
    let config = hc_core::config::Config::load(&paths.config_file()).unwrap_or_default();
    let dir = config
        .general
        .clips_dir
        .clone()
        .unwrap_or_else(|| paths.default_clips_dir.clone())
        .join("Exports");
    let ext = project.output.format.extension();
    let base = format!("{} {}", hc_library::sanitize(&item.title), pr.id());
    let mut out = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    let queue_dir = paths.cache_dir.join("exports");
    let job = hc_editor::queue::Job {
        id: format!("cli-{}-{}", std::process::id(), id),
        project,
        output: out.clone(),
        library_db: Some(paths.library_db()),
        thumbs_dir: Some(paths.thumbs_dir()),
    };
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| hc_editor::queue::enqueue(&queue_dir, &job))
    {
        return fail(e);
    }
    // The studio executable hosts the export worker (one drains the queue).
    let running = hc_editor::queue::list(&queue_dir)
        .iter()
        .any(|(_, s)| s.state == hc_editor::queue::State::Running);
    if !running
        && let Err(e) = std::process::Command::new(paths.sibling_exe(hc_core::STUDIO_EXE))
            .arg("--export-worker")
            .arg(&queue_dir)
            .spawn()
    {
        return fail(format!("cannot start the export worker: {e}"));
    }
    println!("queued: {}", out.display());
    ExitCode::SUCCESS
}

fn highlights(id: i64) -> ExitCode {
    let (paths, mut lib) = match open_library() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let item = match lib.get(id) {
        Ok(Some(i)) if i.duration_ms > 0 => i,
        Ok(_) => return fail(format!("no video with id {id}")),
        Err(e) => return fail(e),
    };
    let config = hc_core::config::Config::load(&paths.config_file()).unwrap_or_default();
    let params = hc_editor::highlight::Params {
        margin_db: config.highlights.margin_db as f32,
        ..Default::default()
    };
    hc_encode::mf::init_thread();
    let found = match hc_editor::highlight::detect_file(&item.path, item.duration_ms, &params) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    for h in &found {
        let s = h.at_secs as u64;
        println!(
            "{:>3}:{:02}.{}  +{:.1} dB",
            s / 60,
            s % 60,
            ((h.at_secs.fract()) * 10.0) as u32,
            h.score_db
        );
    }
    let secs: Vec<f64> = found.iter().map(|h| h.at_secs).collect();
    match lib.add_highlights(id, &secs) {
        Ok(added) => {
            println!("{} highlights, {added} new markers", found.len());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn backup(dest: &std::path::Path) -> ExitCode {
    let r = hc_core::paths::AppPaths::detect()
        .map_err(|e| e.to_string())
        .and_then(|p| hc_core::backup::create(&p, dest).map_err(|e| e.to_string()));
    match r {
        Ok(()) => {
            println!("settings saved to {}", dest.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn restore(src: &std::path::Path) -> ExitCode {
    let r = hc_core::paths::AppPaths::detect()
        .map_err(|e| e.to_string())
        .and_then(|p| hc_core::backup::restore(&p, src).map_err(|e| e.to_string()));
    match r {
        Ok(_) => {
            // Apply immediately if the core is running.
            let _ = one_shot(&Request::ReloadConfig, Duration::from_secs(3));
            println!("settings restored");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn list() -> ExitCode {
    let (_, lib) = match open_library() {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let items = match lib.list(
        &hc_library::Filter::default(),
        hc_library::Sort::Newest,
        100,
        0,
    ) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{:>5}  {:<10}  {:>8}  {:<20}  path",
        "id", "kind", "length", "game"
    );
    for it in items {
        let secs = it.duration_ms / 1000;
        println!(
            "{:>5}  {:<10}  {:>5}:{:02}  {:<20}  {}{}",
            it.id,
            it.kind.as_str(),
            secs / 60,
            secs % 60,
            it.game.unwrap_or_default(),
            it.path.display(),
            if it.favorite { "  ★" } else { "" }
        );
    }
    ExitCode::SUCCESS
}
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // Output redirected to a file/pipe already has a handle: keep it
    // (attaching would replace it with the console and lose the output).
    // SAFETY: plain handle query; failure to attach is harmless.
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null()) {
            return;
        }
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
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
        assert_eq!(parse(&["--list"]).unwrap().command(), Some(Command::List));
        assert_eq!(
            parse(&["--export", "3", "--preset", "gif"])
                .unwrap()
                .command(),
            Some(Command::Export {
                id: 3,
                preset: "gif".into()
            })
        );
        assert!(matches!(
            parse(&["--export", "x"]).unwrap().command(),
            Some(Command::Invalid(_))
        ));
        assert_eq!(
            parse(&["--highlights", "7"]).unwrap().command(),
            Some(Command::Highlights(7))
        );
    }

    #[test]
    fn commands_are_mutually_exclusive() {
        assert!(parse(&["--save-clip", "--quit"]).is_err());
        assert!(parse(&["--preset", "x"]).is_err());
    }
}
