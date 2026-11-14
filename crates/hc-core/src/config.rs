//! User configuration (`config.toml`): typed, validated, written atomically.

use crate::error::{Error, Result};
use crate::hotkey::{find_conflicts, Hotkey, HotkeyAction};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MIN_REPLAY_SECS: u32 = 15;
pub const MAX_REPLAY_SECS: u32 = 600;
pub const MIN_FPS: u32 = 15;
pub const MAX_FPS: u32 = 144;
pub const MIN_BITRATE_KBPS: u32 = 500;
pub const MAX_BITRATE_KBPS: u32 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Oled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheck {
    #[default]
    OnStart,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `"auto"` picks the system language, otherwise a file name in `lang/` (e.g. `"it"`).
    pub language: String,
    pub theme: Theme,
    pub start_with_windows: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replay {
    pub enabled: bool,
    pub duration_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub save_clip: String,
    pub toggle_recording: String,
    pub screenshot: String,
    pub open_ui: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    pub update_check: UpdateCheck,
    pub strip_metadata: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub fps: u32,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Record everything the PC plays (WASAPI loopback) as its own track.
    pub system: bool,
    /// Record the default microphone as its own track (off by default: privacy).
    pub microphone: bool,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Games {
    /// Tag clips with the fullscreen program that was in front.
    pub detect: bool,
    /// Extra program names (e.g. `tool.exe`) that are never treated as games.
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Updates {
    pub channel: Channel,
    /// Version the user chose to ignore (empty = none).
    pub skip_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Storage {
    /// Folder for saved clips. Empty = default (Videos\HyperClips, or `clips` when portable).
    pub clips_dir: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub replay: Replay,
    pub video: Video,
    pub audio: Audio,
    pub games: Games,
    pub storage: Storage,
    pub updates: Updates,
    pub hotkeys: Hotkeys,
    pub privacy: Privacy,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::Dark,
            start_with_windows: false,
        }
    }
}
impl Default for Replay {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_secs: 60,
        }
    }
}
impl Default for Video {
    fn default() -> Self {
        // The Phase 2 encoder is software H.264, so the default stays modest.
        Self {
            fps: 30,
            bitrate_kbps: 8000,
        }
    }
}
impl Default for Games {
    fn default() -> Self {
        Self {
            detect: true,
            ignore: Vec::new(),
        }
    }
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            system: true,
            microphone: false,
            bitrate_kbps: 128,
        }
    }
}
impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            save_clip: "Alt+F10".into(),
            toggle_recording: "Alt+F9".into(),
            screenshot: "Alt+F8".into(),
            open_ui: "Ctrl+Alt+H".into(),
        }
    }
}
impl Default for Privacy {
    fn default() -> Self {
        Self {
            update_check: UpdateCheck::OnStart,
            strip_metadata: true,
        }
    }
}

impl Hotkeys {
    pub fn spec(&self, action: HotkeyAction) -> &str {
        match action {
            HotkeyAction::SaveClip => &self.save_clip,
            HotkeyAction::ToggleRecording => &self.toggle_recording,
            HotkeyAction::Screenshot => &self.screenshot,
            HotkeyAction::OpenUi => &self.open_ui,
        }
    }

    /// Parses every binding; fails on invalid specs or on two actions sharing a key.
    pub fn parsed(&self) -> Result<Vec<(HotkeyAction, Hotkey)>> {
        let mut list = Vec::with_capacity(HotkeyAction::ALL.len());
        for action in HotkeyAction::ALL {
            let hk = Hotkey::parse(self.spec(action))
                .map_err(|e| Error::Hotkey(format!("{}: {e}", action.config_key())))?;
            list.push((action, hk));
        }
        if let Some((a, b)) = find_conflicts(&list).first() {
            return Err(Error::Hotkey(format!(
                "{} and {} use the same key",
                a.config_key(),
                b.config_key()
            )));
        }
        Ok(list)
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if !(MIN_REPLAY_SECS..=MAX_REPLAY_SECS).contains(&self.replay.duration_secs) {
            return Err(Error::Invalid(format!(
                "replay.duration_secs must be {MIN_REPLAY_SECS}..={MAX_REPLAY_SECS}"
            )));
        }
        if !(MIN_FPS..=MAX_FPS).contains(&self.video.fps) {
            return Err(Error::Invalid(format!(
                "video.fps must be {MIN_FPS}..={MAX_FPS}"
            )));
        }
        if ![96, 128, 160, 192].contains(&self.audio.bitrate_kbps) {
            return Err(Error::Invalid(
                "audio.bitrate_kbps must be 96, 128, 160 or 192".into(),
            ));
        }
        if !(MIN_BITRATE_KBPS..=MAX_BITRATE_KBPS).contains(&self.video.bitrate_kbps) {
            return Err(Error::Invalid(format!(
                "video.bitrate_kbps must be {MIN_BITRATE_KBPS}..={MAX_BITRATE_KBPS}"
            )));
        }
        // Language is later used to build a file path: forbid anything but a plain code.
        let lang = &self.general.language;
        if lang.is_empty()
            || lang.len() > 16
            || !lang
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::Invalid(
                "general.language is not a valid language code".into(),
            ));
        }
        self.hotkeys.parsed().map(|_| ())
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Loads the config. A missing file yields defaults; an unreadable or invalid
    /// file is moved to `config.toml.bad` and defaults are used, so a typo never
    /// prevents the app from starting.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        match Self::from_toml(&text) {
            Ok(cfg) => Ok(cfg),
            Err(err) => {
                tracing::warn!("invalid config ({err}); keeping a copy and using defaults");
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                // Best effort: failing to keep the copy must not block startup.
                let _ = std::fs::rename(path, PathBuf::from(bad));
                Ok(Self::default())
            }
        }
    }

    /// Validates, then writes through a temporary file and a rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = self.to_toml()?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip() {
        let cfg = Config::default();
        cfg.validate().unwrap();
        assert_eq!(Config::from_toml(&cfg.to_toml().unwrap()).unwrap(), cfg);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let cfg = Config::from_toml("[replay]\nduration_secs = 120\n").unwrap();
        assert_eq!(cfg.replay.duration_secs, 120);
        assert!(cfg.replay.enabled);
        assert_eq!(cfg.hotkeys, Hotkeys::default());
    }

    #[test]
    fn rejects_out_of_range_duration() {
        assert!(Config::from_toml("[replay]\nduration_secs = 5\n").is_err());
        assert!(Config::from_toml("[replay]\nduration_secs = 601\n").is_err());
    }

    #[test]
    fn update_channel_parses() {
        let c =
            Config::from_toml("[updates]\nchannel = \"beta\"\nskip_version = \"0.1.5\"\n").unwrap();
        assert_eq!(c.updates.channel, Channel::Beta);
        assert_eq!(c.updates.skip_version, "0.1.5");
        assert!(Config::from_toml("[updates]\nchannel = \"nightly\"\n").is_err());
    }

    #[test]
    fn rejects_bad_video_settings() {
        assert!(Config::from_toml("[video]\nfps = 5\n").is_err());
        assert!(Config::from_toml("[video]\nbitrate_kbps = 10\n").is_err());
        let ok = Config::from_toml("[video]\nfps = 60\nbitrate_kbps = 20000\n").unwrap();
        assert_eq!((ok.video.fps, ok.video.bitrate_kbps), (60, 20000));
    }

    #[test]
    fn rejects_hotkey_conflicts_and_bad_language() {
        let dup = "[hotkeys]\nsave_clip = \"Alt+F9\"\ntoggle_recording = \"Alt+F9\"\n";
        assert!(Config::from_toml(dup).is_err());
        assert!(Config::from_toml("[general]\nlanguage = \"../../x\"\n").is_err());
    }

    #[test]
    fn invalid_file_is_set_aside_and_defaults_used() {
        let dir = std::env::temp_dir().join(format!("hc-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(Config::load_or_default(&path).unwrap(), Config::default());
        assert!(dir.join("config.toml.bad").is_file());
        assert!(!path.exists());

        let mut cfg = Config::default();
        cfg.replay.duration_secs = 90;
        cfg.save(&path).unwrap();
        assert_eq!(Config::load_or_default(&path).unwrap(), cfg);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
