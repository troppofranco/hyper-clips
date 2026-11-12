//! Local-only logging: daily rolling files in the app's `logs` folder.
//! Nothing is ever sent anywhere.

use crate::error::{Error, Result};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;

const KEEP_LOG_FILES: usize = 7;

/// Keep the returned guard alive for the whole process; dropping it flushes the log.
pub fn init(logs_dir: &Path, prefix: &str, verbose: bool) -> Result<WorkerGuard> {
    std::fs::create_dir_all(logs_dir)?;
    prune(logs_dir, prefix);

    let appender = tracing_appender::rolling::daily(logs_dir, format!("{prefix}.log"));
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let level = if verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(level)
        .try_init()
        .map_err(|e| Error::System(format!("logging already initialised: {e}")))?;
    Ok(guard)
}

/// Deletes the oldest log files of this process kind beyond `KEEP_LOG_FILES`.
fn prune(dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .collect();
    // Daily file names end with the date, so name order is age order.
    files.sort_by_key(std::fs::DirEntry::file_name);
    if files.len() > KEEP_LOG_FILES {
        for old in &files[..files.len() - KEEP_LOG_FILES] {
            let _ = std::fs::remove_file(old.path());
        }
    }
}
