//! Launching / focusing the on-demand studio process.

use hc_core::STUDIO_EXE;
use hc_core::paths::AppPaths;
use hc_core::sys::{find_processes, focus_process_window};

pub fn open(paths: &AppPaths) -> Result<(), String> {
    let running = find_processes(STUDIO_EXE);
    if !running.is_empty() {
        // Already open (or starting): just bring it to the front.
        for pid in running {
            if focus_process_window(pid) {
                break;
            }
        }
        return Ok(());
    }
    let exe = paths.sibling_exe(STUDIO_EXE);
    std::process::Command::new(&exe)
        .current_dir(&paths.exe_dir)
        .spawn()
        .map(|child| tracing::info!(pid = child.id(), "studio started"))
        .map_err(|e| format!("{}: {e}", hc_core::paths::redact(&exe)))
}
