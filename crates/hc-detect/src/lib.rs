//! Game detection.
//!
//! Looks only at the *foreground window*: its process image name (opened
//! with `PROCESS_QUERY_LIMITED_INFORMATION`, never with memory access, so it
//! is safe with anti-cheat software) and whether it covers its monitor.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

const EMBEDDED: &str = include_str!("../../../data/games.toml");

/// Processes that are never games even when fullscreen.
const NOT_GAMES: &[&str] = &[
    "explorer.exe",
    "hyperclips.exe",
    "hyperclips-studio.exe",
    "applicationframehost.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "lockapp.exe",
    "msedge.exe",
    "chrome.exe",
    "firefox.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "powerpnt.exe",
    "taskmgr.exe",
];

#[derive(Debug, Deserialize)]
struct DbFile {
    #[serde(default)]
    game: Vec<DbEntry>,
}

#[derive(Debug, Deserialize)]
struct DbEntry {
    name: String,
    exe: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub name: String,
    pub exe: String,
    /// True if matched in the database (false = fullscreen heuristic).
    pub known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foreground {
    /// Centre of the foreground window (virtual desktop coordinates).
    pub center: (i32, i32),
    pub covers_monitor: bool,
    pub pid: u32,
    /// Window handle as an integer (HWND is not Send).
    pub hwnd: isize,
}

#[derive(Debug, Default)]
pub struct GameDb {
    by_exe: HashMap<String, String>,
}

impl GameDb {
    /// Embedded list plus an optional user file that overrides it.
    pub fn load(user_file: Option<&Path>) -> Self {
        let mut db = Self::default();
        db.merge(EMBEDDED);
        if let Some(p) = user_file
            && let Ok(text) = std::fs::read_to_string(p)
        {
            db.merge(&text);
        }
        db
    }

    fn merge(&mut self, text: &str) {
        match toml::from_str::<DbFile>(text) {
            Ok(f) => {
                for g in f.game {
                    for exe in g.exe {
                        self.by_exe.insert(exe.to_ascii_lowercase(), g.name.clone());
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "invalid games database"),
        }
    }

    pub fn len(&self) -> usize {
        self.by_exe.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_exe.is_empty()
    }

    /// Classify a process; `covers_monitor` enables the fullscreen heuristic.
    pub fn classify(&self, exe: &str, covers_monitor: bool) -> Option<Game> {
        let lower = exe.to_ascii_lowercase();
        if let Some(name) = self.by_exe.get(&lower) {
            return Some(Game {
                name: name.clone(),
                exe: exe.to_owned(),
                known: true,
            });
        }
        if covers_monitor && !NOT_GAMES.contains(&lower.as_str()) {
            let stem = exe
                .strip_suffix(".exe")
                .or_else(|| exe.strip_suffix(".EXE"))
                .unwrap_or(exe);
            return Some(Game {
                name: stem.to_owned(),
                exe: exe.to_owned(),
                known: false,
            });
        }
        None
    }

    /// Game currently in the foreground, if any.
    pub fn detect(&self) -> Option<(Game, Foreground)> {
        let (exe, fg) = foreground()?;
        self.classify(&exe, fg.covers_monitor).map(|g| (g, fg))
    }
}

/// Image name and geometry of the foreground window.
pub fn foreground() -> Option<(String, Foreground)> {
    // SAFETY: plain Win32 queries; the process handle is closed before returning.
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_invalid() || hwnd == GetShellWindow() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let exe = full.rsplit('\\').next().unwrap_or(&full).to_owned();

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let covers = GetMonitorInfoW(monitor, &mut mi).as_bool()
            && rect.left <= mi.rcMonitor.left
            && rect.top <= mi.rcMonitor.top
            && rect.right >= mi.rcMonitor.right
            && rect.bottom >= mi.rcMonitor.bottom;
        Some((
            exe,
            Foreground {
                center: ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
                covers_monitor: covers,
                pid,
                hwnd: hwnd.0 as isize,
            },
        ))
    }
}

/// Make a name safe for use as a folder / file name.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Desktop".to_owned()
    } else {
        trimmed.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_db_parses() {
        let db = GameDb::load(None);
        assert!(db.len() > 20);
        let g = db.classify("CS2.EXE", false).unwrap();
        assert_eq!(g.name, "Counter-Strike 2");
        assert!(g.known);
    }

    #[test]
    fn fullscreen_heuristic() {
        let db = GameDb::load(None);
        assert_eq!(db.classify("MyIndie.exe", false), None);
        let g = db.classify("MyIndie.exe", true).unwrap();
        assert_eq!(g.name, "MyIndie");
        assert!(!g.known);
        assert_eq!(db.classify("explorer.exe", true), None);
        assert_eq!(db.classify("chrome.exe", true), None);
    }

    #[test]
    fn user_file_overrides() {
        let dir = std::env::temp_dir().join(format!("hc-games-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("games.toml");
        std::fs::write(
            &p,
            "[[game]]\nname = \"My Game\"\nexe = [\"cs2.exe\", \"mine.exe\"]\n",
        )
        .unwrap();
        let db = GameDb::load(Some(&p));
        assert_eq!(db.classify("cs2.exe", false).unwrap().name, "My Game");
        assert_eq!(db.classify("mine.exe", false).unwrap().name, "My Game");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_file_name("Halo: Infinite?"), "Halo_ Infinite_");
        assert_eq!(sanitize_file_name("  ..  "), "Desktop");
        assert_eq!(sanitize_file_name("osu!"), "osu!");
    }
}
