//! Resolution of every directory the app is allowed to write to.
//!
//! Installed mode splits small roaming config from heavy local data:
//! - config: `%APPDATA%\HyperClips`
//! - data/cache/logs: `%LOCALAPPDATA%\HyperClips`
//! - clips: `<Videos>\Hyper Clips`
//!
//! Portable mode (a `portable.flag` file next to the executable) keeps
//! everything under `<exe dir>\data`.

use std::path::{Path, PathBuf};

/// Marker file that switches the app to portable mode.
pub const PORTABLE_FLAG: &str = "portable.flag";
const DIR_NAME: &str = "HyperClips";
const CLIPS_DIR_NAME: &str = "Hyper Clips";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InstallMode {
    Installed,
    Portable,
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("cannot locate the running executable: {0}")]
    CurrentExe(std::io::Error),
    #[error("cannot resolve known folder {0}: {1}")]
    KnownFolder(&'static str, String),
    #[error("cannot create directory {path}: {source}")]
    Create {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Well-known OS folders, injected so resolution is testable.
#[derive(Debug, Clone)]
pub struct KnownDirs {
    pub roaming_app_data: PathBuf,
    pub local_app_data: PathBuf,
    pub videos: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub mode: InstallMode,
    pub exe_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub default_clips_dir: PathBuf,
}

impl AppPaths {
    /// Resolve paths for the running executable.
    pub fn detect() -> Result<Self, PathError> {
        let exe = std::env::current_exe().map_err(PathError::CurrentExe)?;
        let exe_dir = exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let portable = exe_dir.join(PORTABLE_FLAG).is_file();
        if portable {
            Ok(Self::portable(exe_dir))
        } else {
            Ok(Self::installed(exe_dir, &known_dirs()?))
        }
    }

    pub fn portable(exe_dir: PathBuf) -> Self {
        let root = exe_dir.join("data");
        Self {
            mode: InstallMode::Portable,
            config_dir: root.join("config"),
            data_dir: root.join("library"),
            cache_dir: root.join("cache"),
            log_dir: root.join("logs"),
            tmp_dir: root.join("tmp"),
            default_clips_dir: root.join("clips"),
            exe_dir,
        }
    }

    pub fn installed(exe_dir: PathBuf, known: &KnownDirs) -> Self {
        let local = known.local_app_data.join(DIR_NAME);
        Self {
            mode: InstallMode::Installed,
            config_dir: known.roaming_app_data.join(DIR_NAME),
            data_dir: local.join("library"),
            cache_dir: local.join("cache"),
            log_dir: local.join("logs"),
            tmp_dir: local.join("tmp"),
            default_clips_dir: known.videos.join(CLIPS_DIR_NAME),
            exe_dir,
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn sibling_exe(&self, name: &str) -> PathBuf {
        self.exe_dir.join(name)
    }

    /// Every directory owned by the app (used by the privacy audit and "delete all").
    pub fn owned_dirs(&self) -> Vec<&Path> {
        vec![
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.tmp_dir,
        ]
    }

    /// Create the directories needed at startup (clips dir is created lazily).
    pub fn ensure(&self) -> Result<(), PathError> {
        for dir in self.owned_dirs() {
            std::fs::create_dir_all(dir).map_err(|source| PathError::Create {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }
}

/// Replace the user profile prefix with `~` so logs never contain the account name.
pub fn redact(path: &Path) -> String {
    let shown = path.display().to_string();
    match std::env::var_os("USERPROFILE") {
        Some(profile) => redact_with(&shown, &profile.to_string_lossy()),
        None => shown,
    }
}

fn redact_with(path: &str, profile: &str) -> String {
    if profile.is_empty() {
        return path.to_owned();
    }
    if path.len() >= profile.len()
        && path.is_char_boundary(profile.len())
        && path[..profile.len()].eq_ignore_ascii_case(profile)
    {
        format!("~{}", &path[profile.len()..])
    } else {
        path.to_owned()
    }
}

fn known_dirs() -> Result<KnownDirs, PathError> {
    use windows::Win32::UI::Shell::{
        FOLDERID_LocalAppData, FOLDERID_RoamingAppData, FOLDERID_Videos,
    };
    Ok(KnownDirs {
        roaming_app_data: known_folder(&FOLDERID_RoamingAppData, "RoamingAppData")?,
        local_app_data: known_folder(&FOLDERID_LocalAppData, "LocalAppData")?,
        videos: known_folder(&FOLDERID_Videos, "Videos")?,
    })
}

fn known_folder(id: &windows::core::GUID, name: &'static str) -> Result<PathBuf, PathError> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: SHGetKnownFolderPath returns a CoTaskMem-allocated string that we
    // copy and free exactly once.
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
            .map_err(|e| PathError::KnownFolder(name, e.to_string()))?;
        let result = raw
            .to_string()
            .map(PathBuf::from)
            .map_err(|e| PathError::KnownFolder(name, e.to_string()));
        CoTaskMemFree(Some(raw.0 as *const _));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> KnownDirs {
        KnownDirs {
            roaming_app_data: PathBuf::from(r"C:\Users\u\AppData\Roaming"),
            local_app_data: PathBuf::from(r"C:\Users\u\AppData\Local"),
            videos: PathBuf::from(r"C:\Users\u\Videos"),
        }
    }

    #[test]
    fn installed_layout() {
        let p = AppPaths::installed(PathBuf::from(r"C:\Apps\HC"), &known());
        assert_eq!(p.mode, InstallMode::Installed);
        assert_eq!(
            p.config_file(),
            PathBuf::from(r"C:\Users\u\AppData\Roaming\HyperClips\config.toml")
        );
        assert_eq!(
            p.log_dir,
            PathBuf::from(r"C:\Users\u\AppData\Local\HyperClips\logs")
        );
        assert_eq!(
            p.default_clips_dir,
            PathBuf::from(r"C:\Users\u\Videos\Hyper Clips")
        );
    }

    #[test]
    fn portable_layout_stays_inside_exe_dir() {
        let exe_dir = PathBuf::from(r"D:\Portable\HC");
        let p = AppPaths::portable(exe_dir.clone());
        assert_eq!(p.mode, InstallMode::Portable);
        for dir in p.owned_dirs() {
            assert!(dir.starts_with(&exe_dir), "{} escapes", dir.display());
        }
        assert!(p.default_clips_dir.starts_with(&exe_dir));
    }

    #[test]
    fn redaction() {
        assert_eq!(
            redact_with(r"C:\Users\Mario\Videos\a.mp4", r"C:\Users\Mario"),
            r"~\Videos\a.mp4"
        );
        assert_eq!(redact_with(r"c:\users\mario\x", r"C:\Users\Mario"), r"~\x");
        assert_eq!(redact_with(r"D:\Games\x", r"C:\Users\Mario"), r"D:\Games\x");
        assert_eq!(redact_with(r"D:\x", ""), r"D:\x");
    }
}
