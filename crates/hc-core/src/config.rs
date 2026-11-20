//! User configuration, stored as human-readable TOML.
//!
//! Every field has a default so that old or partial files always load;
//! values are clamped by [`Config::sanitize`] instead of being rejected.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const REPLAY_MIN_SECS: u32 = 15;
pub const REPLAY_MAX_SECS: u32 = 600;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config: {0}")]
    Read(std::io::Error),
    #[error("invalid config TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("cannot write config: {0}")]
    Write(std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub capture: Capture,
    pub hotkeys: Hotkeys,
    pub notifications: Notifications,
    pub privacy: Privacy,
    pub updates: Updates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    #[default]
    Dark,
    Oled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// BCP-47 language tag, or "auto" to follow Windows.
    pub language: String,
    pub theme: Theme,
    /// Installed mode only: registers a HKCU Run entry.
    pub start_with_windows: bool,
    /// Custom clips folder; empty means the default for the install mode.
    pub clips_dir: Option<PathBuf>,
    pub log_level: LogLevel,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::Dark,
            start_with_windows: false,
            clips_dir: None,
            log_level: LogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    H264,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RateControl {
    Cbr,
    #[default]
    Vbr,
    Cqp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BufferStorage {
    #[default]
    Auto,
    Memory,
    Disk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub replay_enabled: bool,
    pub replay_seconds: u32,
    pub buffer_storage: BufferStorage,
    pub fps: u32,
    /// Output height in pixels; 0 keeps the source resolution.
    pub height: u32,
    pub codec: Codec,
    pub rate_control: RateControl,
    pub bitrate_kbps: u32,
    pub cqp: u8,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            replay_enabled: true,
            replay_seconds: 60,
            buffer_storage: BufferStorage::Auto,
            fps: 60,
            height: 1080,
            codec: Codec::H264,
            rate_control: RateControl::Vbr,
            bitrate_kbps: 20_000,
            cqp: 23,
        }
    }
}

/// Hotkeys as user-editable strings, e.g. `"Ctrl+Shift+F8"` or `"Mouse5"`.
/// An empty string disables the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub save_clip: String,
    pub toggle_recording: String,
    pub screenshot: String,
    pub add_marker: String,
    pub open_studio: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            save_clip: "F8".into(),
            toggle_recording: "Ctrl+F9".into(),
            screenshot: "Alt+F8".into(),
            add_marker: "Shift+F8".into(),
            open_studio: "Ctrl+Alt+H".into(),
        }
    }
}

impl Hotkeys {
    /// Stable (action id, binding) pairs; the id is also the i18n key suffix.
    pub fn entries(&self) -> [(&'static str, &str); 5] {
        [
            ("save-clip", &self.save_clip),
            ("toggle-recording", &self.toggle_recording),
            ("screenshot", &self.screenshot),
            ("add-marker", &self.add_marker),
            ("open-studio", &self.open_studio),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayPosition {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub sound: bool,
    pub position: OverlayPosition,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: true,
            position: OverlayPosition::TopRight,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// Strip title/game/software metadata from exported files.
    pub strip_metadata: bool,
    /// Opt the executables out of Windows Error Reporting (installed mode).
    pub disable_windows_error_reporting: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            strip_metadata: true,
            disable_windows_error_reporting: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCheck {
    #[default]
    OnStartup,
    EveryHours,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    pub check: UpdateCheck,
    pub interval_hours: u32,
    pub channel: UpdateChannel,
    /// Versions the user chose to skip.
    pub ignored_versions: Vec<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            check: UpdateCheck::OnStartup,
            interval_hours: 24,
            channel: UpdateChannel::Stable,
            ignored_versions: Vec::new(),
        }
    }
}

impl Config {
    /// Load from `path`; a missing file yields defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Read(e)),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let mut cfg: Config = toml::from_str(text)?;
        cfg.sanitize();
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Atomic save: write a temp file next to the target, then rename over it.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(ConfigError::Write)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(ConfigError::Write)?;
        std::fs::rename(&tmp, path).map_err(ConfigError::Write)
    }

    /// Clamp out-of-range values to the nearest supported setting.
    pub fn sanitize(&mut self) {
        let c = &mut self.capture;
        c.replay_seconds = c.replay_seconds.clamp(REPLAY_MIN_SECS, REPLAY_MAX_SECS);
        c.fps = c.fps.clamp(10, 240);
        c.bitrate_kbps = c.bitrate_kbps.clamp(1_000, 200_000);
        c.cqp = c.cqp.clamp(1, 51);
        if c.height != 0 {
            c.height = c.height.clamp(360, 4320);
        }
        self.updates.interval_hours = self.updates.interval_hours.clamp(1, 24 * 30);
        if self.general.language.trim().is_empty() {
            self.general.language = "auto".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip() {
        let cfg = Config::default();
        let text = cfg.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), cfg);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let cfg = Config::from_toml("[capture]\nfps = 144\n").unwrap();
        assert_eq!(cfg.capture.fps, 144);
        assert_eq!(cfg.capture.replay_seconds, 60);
        assert_eq!(cfg.hotkeys, Hotkeys::default());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let cfg =
            Config::from_toml("[capture]\nreplay_seconds = 5\nfps = 9999\nbitrate_kbps = 1\n")
                .unwrap();
        assert_eq!(cfg.capture.replay_seconds, REPLAY_MIN_SECS);
        assert_eq!(cfg.capture.fps, 240);
        assert_eq!(cfg.capture.bitrate_kbps, 1_000);
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(Config::from_toml("[capture\n").is_err());
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("hc-cfg-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.general.theme = Theme::Oled;
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("hc-definitely-missing/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }
}
