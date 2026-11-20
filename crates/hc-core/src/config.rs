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
    pub audio: Audio,
    pub webcam: Webcam,
    pub library: LibrarySettings,
    pub hotkeys: Hotkeys,
    pub notifications: Notifications,
    pub privacy: Privacy,
    pub updates: Updates,
    pub performance: Performance,
    pub highlights: Highlights,
    pub share: Share,
    pub api: Api,
    pub plugins: Plugins,
    /// Per-game overrides, matched on the detected game name or executable.
    pub profiles: Vec<GameProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    Light,
    #[default]
    Dark,
    Oled,
    /// Maximum contrast (accessibility).
    HighContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BatteryPolicy {
    /// Same settings on battery and on AC power.
    Ignore,
    /// Apply the low-impact limits while on battery.
    #[default]
    LowImpact,
    /// Stop the replay buffer while on battery.
    Pause,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Performance {
    /// Cap capture at 30 fps / 720p with a lower bitrate, and skip noise
    /// suppression, to keep the impact on the game as low as possible.
    pub low_impact: bool,
    pub on_battery: BatteryPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlights {
    /// Analyse saved clips locally and add markers on loud moments.
    pub auto_detect: bool,
    /// Required loudness jump over the background, in dB (lower = more).
    pub margin_db: u32,
}

impl Default for Highlights {
    fn default() -> Self {
        Self {
            auto_detect: true,
            margin_db: 9,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    /// Lifetime of a LAN share link, in minutes.
    pub expire_minutes: u32,
    /// Stop after this many complete downloads (0 = unlimited).
    pub max_downloads: u32,
}

impl Default for Share {
    fn default() -> Self {
        Self {
            expire_minutes: 15,
            max_downloads: 0,
        }
    }
}

/// Local HTTP API on 127.0.0.1 (off by default, token protected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Api {
    pub enabled: bool,
    pub port: u16,
}

impl Default for Api {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 47_821,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Plugins {
    /// Off by default: plugins only run when explicitly enabled.
    pub enabled: bool,
    /// Plugin ids the user switched off.
    pub disabled: Vec<String>,
}

/// Settings that override the global ones while a given game is detected.
/// `None` keeps the global value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GameProfile {
    /// Game name as detected, or executable name (case-insensitive).
    pub game: String,
    /// Do not keep a replay buffer for this game.
    pub disable_replay: bool,
    pub replay_seconds: Option<u32>,
    pub fps: Option<u32>,
    pub height: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub codec: Option<Codec>,
    pub capture_mode: Option<CaptureMode>,
    pub microphone: Option<bool>,
    pub separate_tracks: Option<bool>,
}

impl GameProfile {
    pub fn matches(&self, game: &str, exe: Option<&str>) -> bool {
        let want = self.game.trim();
        !want.is_empty()
            && (want.eq_ignore_ascii_case(game.trim())
                || exe.is_some_and(|e| want.eq_ignore_ascii_case(e)))
    }
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
    /// UI zoom factor (accessibility), 0.75..=2.0.
    pub ui_scale: f32,
    /// Disable UI animations.
    pub reduce_motion: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::Dark,
            start_with_windows: false,
            clips_dir: None,
            log_level: LogLevel::Info,
            ui_scale: 1.0,
            reduce_motion: false,
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
    pub monitor: MonitorChoice,
    pub screenshot_format: ScreenshotFormat,
    pub mode: CaptureMode,
    /// Virtual-desktop rectangle for `CaptureMode::Region`.
    pub region: Option<Region>,
    /// Draw the mouse pointer into the video.
    pub show_cursor: bool,
    /// SDR paper white used when tone-mapping an HDR desktop (nits).
    pub sdr_white_nits: u32,
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
            monitor: MonitorChoice::Game,
            screenshot_format: ScreenshotFormat::Png,
            mode: CaptureMode::Monitor,
            region: None,
            show_cursor: true,
            sdr_white_nits: 200,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MonitorChoice {
    /// Monitor showing the foreground window (the game) when capture starts.
    #[default]
    Game,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotFormat {
    #[default]
    Png,
    Jpg,
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    /// Whole monitor via Desktop Duplication (best for fullscreen games).
    #[default]
    Monitor,
    /// Only the game window via Windows Graphics Capture.
    Window,
    /// A fixed rectangle of the desktop.
    Region,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LibrarySettings {
    /// Storage limit in GB for the library; 0 = unlimited.
    pub max_storage_gb: f64,
    /// When over the limit, move the oldest non-favourite items to the
    /// Recycle Bin automatically.
    pub auto_delete: bool,
    /// Additional folders scanned into the library.
    pub extra_folders: Vec<PathBuf>,
}

impl LibrarySettings {
    pub fn limit_bytes(&self) -> Option<u64> {
        (self.auto_delete && self.max_storage_gb > 0.0)
            .then_some((self.max_storage_gb * 1024.0 * 1024.0 * 1024.0) as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Webcam {
    /// Off by default: the camera is only opened when explicitly enabled.
    pub enabled: bool,
    /// Camera friendly name; None = first camera.
    pub device: Option<String>,
    pub position: OverlayPosition,
    /// Overlay height as a percentage of the video height.
    pub size_percent: u32,
}

impl Default for Webcam {
    fn default() -> Self {
        Self {
            enabled: false,
            device: None,
            position: OverlayPosition::BottomRight,
            size_percent: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Everything the PC plays (game, music, chat).
    pub system: bool,
    /// Off by default: recording the microphone must be an explicit choice.
    pub microphone: bool,
    /// Percent, 0..=200.
    pub system_volume: u32,
    pub microphone_volume: u32,
    /// AAC bitrate in kbps (96/128/160/192).
    pub bitrate_kbps: u32,
    /// Separate tracks for game, chat apps and other sounds (per-process
    /// loopback, Windows 10 2004+). Falls back to one system track.
    pub separate_tracks: bool,
    /// Executables treated as voice chat.
    pub chat_apps: Vec<String>,
    /// Hotkey that must be held for the mic to be recorded ("" = always on).
    pub push_to_talk: String,
    pub noise_gate: bool,
    /// Gate threshold in dBFS.
    pub noise_gate_db: i32,
    /// RNNoise-based suppression, fully local.
    pub noise_suppression: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            system: true,
            microphone: false,
            system_volume: 100,
            microphone_volume: 100,
            bitrate_kbps: 160,
            separate_tracks: true,
            chat_apps: [
                "Discord.exe",
                "DiscordPTB.exe",
                "DiscordCanary.exe",
                "TeamSpeak.exe",
                "ts3client_win64.exe",
                "Mumble.exe",
                "ms-teams.exe",
                "Teams.exe",
                "slack.exe",
            ]
            .map(String::from)
            .to_vec(),
            push_to_talk: String::new(),
            noise_gate: false,
            noise_gate_db: -45,
            noise_suppression: false,
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
    /// Show notifications over the game instead of as Windows toasts.
    pub in_game: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: true,
            position: OverlayPosition::TopRight,
            in_game: true,
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
        self.audio.system_volume = self.audio.system_volume.min(200);
        self.audio.microphone_volume = self.audio.microphone_volume.min(200);
        self.audio.bitrate_kbps = self.audio.bitrate_kbps.clamp(96, 192);
        self.audio.noise_gate_db = self.audio.noise_gate_db.clamp(-90, 0);
        if !self.library.max_storage_gb.is_finite() || self.library.max_storage_gb < 0.0 {
            self.library.max_storage_gb = 0.0;
        }
        self.webcam.size_percent = self.webcam.size_percent.clamp(5, 60);
        self.capture.sdr_white_nits = self.capture.sdr_white_nits.clamp(80, 1000);
        self.updates.interval_hours = self.updates.interval_hours.clamp(1, 24 * 30);
        if self.general.language.trim().is_empty() {
            self.general.language = "auto".into();
        }
        if !self.general.ui_scale.is_finite() {
            self.general.ui_scale = 1.0;
        }
        self.general.ui_scale = self.general.ui_scale.clamp(0.75, 2.0);
        self.highlights.margin_db = self.highlights.margin_db.clamp(3, 30);
        self.share.expire_minutes = self.share.expire_minutes.clamp(1, 24 * 60);
        if self.api.port < 1024 {
            self.api.port = Api::default().port;
        }
        self.profiles.retain(|p| !p.game.trim().is_empty());
    }

    /// The settings actually used for capture right now: the global ones,
    /// overridden by the matching game profile, then by the power policy.
    pub fn effective(&self, game: Option<&str>, exe: Option<&str>, on_battery: bool) -> Config {
        let mut c = self.clone();
        if let Some(p) = game.and_then(|g| self.profiles.iter().find(|p| p.matches(g, exe))) {
            let cap = &mut c.capture;
            if p.disable_replay {
                cap.replay_enabled = false;
            }
            if let Some(v) = p.replay_seconds {
                cap.replay_seconds = v;
            }
            if let Some(v) = p.fps {
                cap.fps = v;
            }
            if let Some(v) = p.height {
                cap.height = v;
            }
            if let Some(v) = p.bitrate_kbps {
                cap.bitrate_kbps = v;
            }
            if let Some(v) = p.codec {
                cap.codec = v;
            }
            if let Some(v) = p.capture_mode {
                cap.mode = v;
            }
            if let Some(v) = p.microphone {
                c.audio.microphone = v;
            }
            if let Some(v) = p.separate_tracks {
                c.audio.separate_tracks = v;
            }
        }
        let battery = on_battery && self.performance.on_battery != BatteryPolicy::Ignore;
        if battery && self.performance.on_battery == BatteryPolicy::Pause {
            c.capture.replay_enabled = false;
        }
        if self.performance.low_impact
            || (battery && self.performance.on_battery == BatteryPolicy::LowImpact)
        {
            let cap = &mut c.capture;
            cap.fps = cap.fps.min(30);
            cap.height = if cap.height == 0 {
                720
            } else {
                cap.height.min(720)
            };
            cap.bitrate_kbps = (cap.bitrate_kbps / 2).clamp(2_000, 8_000);
            c.audio.noise_suppression = false;
        }
        c.sanitize();
        c
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
    fn profiles_and_power_policy() {
        let mut cfg = Config::from_toml(
            "[[profiles]]\ngame = \"Valorant\"\nfps = 144\nbitrate_kbps = 40000\n\
             [[profiles]]\ngame = \"solitaire.exe\"\ndisable_replay = true\n",
        )
        .unwrap();
        let e = cfg.effective(Some("valorant"), None, false);
        assert_eq!((e.capture.fps, e.capture.bitrate_kbps), (144, 40_000));
        assert!(
            !cfg.effective(Some("Solitaire"), Some("Solitaire.EXE"), false)
                .capture
                .replay_enabled
        );
        assert_eq!(
            cfg.effective(Some("Other"), None, false).capture,
            cfg.capture
        );
        // Battery (default policy) limits the profile too.
        let b = cfg.effective(Some("Valorant"), None, true);
        assert_eq!(
            (b.capture.fps, b.capture.height, b.capture.bitrate_kbps),
            (30, 720, 8_000)
        );
        cfg.performance.on_battery = BatteryPolicy::Pause;
        assert!(!cfg.effective(None, None, true).capture.replay_enabled);
        cfg.performance.on_battery = BatteryPolicy::Ignore;
        assert_eq!(cfg.effective(None, None, true).capture, cfg.capture);
        cfg.performance.low_impact = true;
        assert_eq!(cfg.effective(None, None, false).capture.fps, 30);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("hc-definitely-missing/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }
}
