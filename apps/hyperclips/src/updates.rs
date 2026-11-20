//! Scheduled update checks in the core (the only network traffic of the
//! app, and only if enabled). Finding an update shows a notification; the
//! studio handles download, verification and installation.

use std::time::Duration;

use hc_core::config::{Config, UpdateChannel, UpdateCheck};
use hc_core::paths::AppPaths;
use windows::Win32::Foundation::HWND;

/// Spawn the checker thread according to the user's settings.
pub fn start(paths: &AppPaths, config: &Config, hwnd: HWND) {
    hc_updater::set_network_log(paths.log_dir.join("network.log"));
    let u = config.updates.clone();
    let interval = match u.check {
        UpdateCheck::Never => return,
        UpdateCheck::OnStartup => None,
        UpdateCheck::EveryHours => Some(Duration::from_secs(
            u64::from(u.interval_hours.max(1)) * 3600,
        )),
    };
    let channel = match u.channel {
        UpdateChannel::Stable => hc_updater::Channel::Stable,
        UpdateChannel::Beta => hc_updater::Channel::Beta,
    };
    let hwnd_raw = hwnd.0 as isize;
    let state_file = paths.data_dir.join("update-available.txt");
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            // Do not compete with startup.
            std::thread::sleep(Duration::from_secs(20));
            loop {
                match hc_updater::check(channel, &u.ignored_versions) {
                    Ok(Some(r)) => {
                        tracing::info!(version = %r.version, "update available");
                        // The studio reads this to show the banner; the core shows a toast.
                        let _ = std::fs::write(&state_file, r.version.to_string());
                        crate::core_app::post_update_available(hwnd_raw, &r.version.to_string());
                    }
                    Ok(None) => {
                        let _ = std::fs::remove_file(&state_file);
                    }
                    Err(e) => tracing::warn!(error = %e, "update check failed"),
                }
                match interval {
                    Some(d) => std::thread::sleep(d),
                    None => break,
                }
            }
        });
}
