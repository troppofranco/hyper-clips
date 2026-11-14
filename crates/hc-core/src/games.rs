//! Game detection rules (platform independent). The OS layer reports the foreground
//! program; these functions decide whether it counts as a game.

/// Programs that are never treated as games even when fullscreen.
const BUILTIN_IGNORED: &[&str] = &[
    "explorer.exe",
    "hyperclips.exe",
    "hyperclips-ui.exe",
    "applicationframehost.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "lockapp.exe",
    "taskmgr.exe",
    "vlc.exe",
    "mpv.exe",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "powerpnt.exe",
];

/// True when `exe` (a file name such as `Game.exe`) must not be reported as a game.
pub fn is_ignored(exe: &str, extra: &[String]) -> bool {
    let lower = exe.to_lowercase();
    BUILTIN_IGNORED.contains(&lower.as_str()) || extra.iter().any(|e| e.to_lowercase() == lower)
}

/// Human-readable name for a program: the file name without `.exe`.
pub fn display_name(exe: &str) -> String {
    let lower = exe.to_lowercase();
    if lower.ends_with(".exe") {
        exe[..exe.len() - 4].to_owned()
    } else {
        exe.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_and_user_ignores_are_case_insensitive() {
        assert!(is_ignored("Explorer.EXE", &[]));
        assert!(is_ignored("mygame.exe", &["MyGame.exe".to_owned()]));
        assert!(!is_ignored("valorant.exe", &[]));
    }

    #[test]
    fn display_name_strips_extension() {
        assert_eq!(display_name("cs2.exe"), "cs2");
        assert_eq!(display_name("Game.EXE"), "Game");
        assert_eq!(display_name("tool"), "tool");
    }
}
