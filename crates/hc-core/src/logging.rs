//! Local-only logging: rolling files in the app's log directory, nothing else.

use std::io::Write;
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;

use crate::config::LogLevel;

const MAX_LOG_FILES: usize = 7;

impl From<LogLevel> for LevelFilter {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Error => LevelFilter::ERROR,
            LogLevel::Warn => LevelFilter::WARN,
            LogLevel::Info => LevelFilter::INFO,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Trace => LevelFilter::TRACE,
        }
    }
}

/// Keep this alive for the whole process: dropping it flushes pending logs.
pub struct LogGuard {
    _guard: WorkerGuard,
}

/// Initialise logging into `<log_dir>/<prefix>.YYYY-MM-DD.log`.
/// Also installs a panic hook that writes `crash.log` synchronously, because
/// release builds abort on panic and would otherwise lose buffered lines.
pub fn init(log_dir: &Path, prefix: &str, level: LogLevel) -> std::io::Result<LogGuard> {
    std::fs::create_dir_all(log_dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(log_dir)
        .map_err(std::io::Error::other)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_filter(LevelFilter::from(level));
    // Ignore "already set" (tests may initialise twice).
    let _ = tracing_subscriber::registry().with(layer).try_init();

    install_panic_hook(log_dir.join("crash.log"));
    Ok(LogGuard { _guard: guard })
}

fn install_panic_hook(crash_file: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".into());
        tracing::error!(%location, %payload, "panic");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_file)
        {
            let _ = writeln!(
                f,
                "[{} v{}] panic at {location}: {payload}",
                unix_time(),
                crate::VERSION
            );
        }
        default_hook(info);
    }));
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
