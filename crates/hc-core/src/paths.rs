//! Where Hyper Clips keeps its files, including portable mode.
//!
//! Portable mode is on when a file named `portable.flag` sits next to the
//! executable: everything is then written under `<exe dir>\data` and nothing
//! outside the app folder is touched.

use crate::error::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const PORTABLE_FLAG: &str = "portable.flag";

#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
    exe_dir: PathBuf,
    portable: bool,
}

impl AppPaths {
    /// Resolves paths for the running executable.
    pub fn detect() -> Result<Self> {
        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or_else(|| Error::Invalid("executable has no parent folder".into()))?;
        Self::resolve_with(dir, &|k| std::env::var_os(k), cfg!(windows))
    }

    /// Testable core: environment and OS flavour are injected.
    pub fn resolve_with(
        exe_dir: &Path,
        env: &dyn Fn(&str) -> Option<OsString>,
        windows: bool,
    ) -> Result<Self> {
        let exe_dir = exe_dir.to_path_buf();
        if exe_dir.join(PORTABLE_FLAG).is_file() {
            return Ok(Self {
                root: exe_dir.join("data"),
                exe_dir,
                portable: true,
            });
        }

        // Non-Windows branch exists only so the workspace can be developed on Linux.
        let (base, dir_name) = if windows {
            (env("APPDATA").map(PathBuf::from), "HyperClips")
        } else {
            (
                env("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))),
                "hyperclips",
            )
        };
        let base = base
            .filter(|p| p.is_absolute())
            .ok_or_else(|| Error::Invalid("cannot determine the data folder".into()))?;

        Ok(Self {
            root: base.join(dir_name),
            exe_dir,
            portable: false,
        })
    }

    pub fn is_portable(&self) -> bool {
        self.portable
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn exe_dir(&self) -> &Path {
        &self.exe_dir
    }
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }
    /// External language files; they override the embedded translations.
    pub fn lang_dir(&self) -> PathBuf {
        self.exe_dir.join("lang")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [&self.root, &self.logs_dir(), &self.data_dir()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn windows_root_is_appdata() {
        // A path that is absolute on the host OS (a bare "/x" is not absolute on Windows).
        let appdata = std::env::temp_dir().join("AppData");
        let env = move |k: &str| (k == "APPDATA").then(|| appdata.clone().into_os_string());
        let p = AppPaths::resolve_with(Path::new("app"), &env, true).unwrap();
        let expected = std::env::temp_dir().join("AppData").join("HyperClips");
        assert!(!p.is_portable());
        assert_eq!(p.root(), expected.as_path());
        assert_eq!(p.config_file(), expected.join("config.toml"));
        assert_eq!(p.lang_dir(), Path::new("app").join("lang"));
    }

    #[test]
    fn missing_or_relative_appdata_is_an_error() {
        assert!(AppPaths::resolve_with(Path::new("/app"), &env_of(&[]), true).is_err());
        let env = env_of(&[("APPDATA", "relative")]);
        assert!(AppPaths::resolve_with(Path::new("/app"), &env, true).is_err());
    }

    #[test]
    fn portable_flag_keeps_everything_in_app_folder() {
        let dir = std::env::temp_dir().join(format!("hc-paths-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PORTABLE_FLAG), b"").unwrap();
        let p = AppPaths::resolve_with(&dir, &env_of(&[]), true).unwrap();
        assert!(p.is_portable());
        assert!(p.root().starts_with(&dir));
        assert!(p.logs_dir().starts_with(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
