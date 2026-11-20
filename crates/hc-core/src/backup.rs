//! Settings backup and restore.
//!
//! A backup is one human-readable TOML file holding the user's settings
//! (`config.toml`) and custom game list (`games.toml`). It never contains
//! clips, the library database, logs or keys.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::paths::AppPaths;

const FORMAT: &str = "hyperclips-settings";
const GAMES_FILE: &str = "games.toml";

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Hyper Clips settings backup")]
    NotABackup,
    #[error("backup format {0} is newer than this version supports")]
    TooNew(u32),
    #[error("invalid backup: {0}")]
    Invalid(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct Backup {
    format: String,
    version: u32,
    app_version: String,
    created: String,
    config: String,
    #[serde(default)]
    games: Option<String>,
}

/// Write the current settings to `dest`.
pub fn create(paths: &AppPaths, dest: &Path) -> Result<(), BackupError> {
    let config = match std::fs::read_to_string(paths.config_file()) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default()
            .to_toml()
            .map_err(|e| BackupError::Invalid(e.to_string()))?,
        Err(e) => return Err(e.into()),
    };
    let games = std::fs::read_to_string(paths.config_dir.join(GAMES_FILE)).ok();
    let b = Backup {
        format: FORMAT.into(),
        version: 1,
        app_version: crate::VERSION.into(),
        created: crate::sys::local_timestamp(),
        config,
        games,
    };
    let text = toml::to_string_pretty(&b).map_err(|e| BackupError::Invalid(e.to_string()))?;
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(dest, text)?;
    Ok(())
}

/// Validate `src` and replace the current settings with it. The previous
/// files are kept as `*.bak`. Returns the restored configuration.
pub fn restore(paths: &AppPaths, src: &Path) -> Result<Config, BackupError> {
    let text = std::fs::read_to_string(src)?;
    let b: Backup = toml::from_str(&text).map_err(|_| BackupError::NotABackup)?;
    if b.format != FORMAT {
        return Err(BackupError::NotABackup);
    }
    if b.version > 1 {
        return Err(BackupError::TooNew(b.version));
    }
    let cfg = Config::from_toml(&b.config).map_err(|e| BackupError::Invalid(e.to_string()))?;
    if let Some(g) = &b.games {
        toml::from_str::<toml::Table>(g).map_err(|e| BackupError::Invalid(e.to_string()))?;
    }
    std::fs::create_dir_all(&paths.config_dir)?;
    let cfg_path = paths.config_file();
    if cfg_path.exists() {
        std::fs::copy(&cfg_path, cfg_path.with_extension("toml.bak"))?;
    }
    cfg.save(&cfg_path)
        .map_err(|e| BackupError::Invalid(e.to_string()))?;
    let games_path = paths.config_dir.join(GAMES_FILE);
    if let Some(g) = &b.games {
        if games_path.exists() {
            std::fs::copy(&games_path, games_path.with_extension("toml.bak"))?;
        }
        std::fs::write(&games_path, g)?;
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validation() {
        let root = std::env::temp_dir().join(format!("hc-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = AppPaths::portable(root.clone());
        paths.ensure().unwrap();
        let mut cfg = Config::default();
        cfg.capture.fps = 144;
        cfg.save(&paths.config_file()).unwrap();
        std::fs::write(
            paths.config_dir.join(GAMES_FILE),
            "[games]\n\"x.exe\" = \"X\"\n",
        )
        .unwrap();

        let file = root.join("settings.hcbackup");
        create(&paths, &file).unwrap();
        // Change things, then restore.
        Config::default().save(&paths.config_file()).unwrap();
        std::fs::remove_file(paths.config_dir.join(GAMES_FILE)).unwrap();
        let restored = restore(&paths, &file).unwrap();
        assert_eq!(restored.capture.fps, 144);
        assert_eq!(Config::load(&paths.config_file()).unwrap().capture.fps, 144);
        assert!(paths.config_dir.join(GAMES_FILE).exists());
        assert!(paths.config_file().with_extension("toml.bak").exists());

        let bogus = root.join("bogus.toml");
        std::fs::write(&bogus, "hello = 1\n").unwrap();
        assert!(matches!(
            restore(&paths, &bogus),
            Err(BackupError::NotABackup)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
