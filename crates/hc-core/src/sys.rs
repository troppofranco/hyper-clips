//! Small Windows helpers shared by the executables.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetProcessTimes};
use windows::core::{HSTRING, w};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    pub working_set: u64,
    pub private_bytes: u64,
}

/// Memory counters of an arbitrary process handle (needs QUERY_LIMITED_INFORMATION + VM_READ).
pub fn process_memory(process: HANDLE) -> Option<MemoryUsage> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: struct is correctly sized; EX is a superset of the basic counters.
    unsafe {
        GetProcessMemoryInfo(
            process,
            (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            c.cb,
        )
        .ok()?;
    }
    Some(MemoryUsage {
        working_set: c.WorkingSetSize as u64,
        private_bytes: c.PrivateUsage as u64,
    })
}

pub fn current_memory() -> MemoryUsage {
    // SAFETY: pseudo-handle, never closed.
    process_memory(unsafe { GetCurrentProcess() }).unwrap_or_default()
}

/// (creation time, kernel+user CPU time) in 100ns units.
pub fn process_times(process: HANDLE) -> Option<(u64, u64)> {
    let (mut create, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: plain out-parameters.
    unsafe { GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user).ok()? };
    Some((ft(create), ft(kernel) + ft(user)))
}

/// Milliseconds since the current process was created.
pub fn ms_since_process_start() -> u64 {
    // SAFETY: pseudo-handle.
    let Some((created, _)) = process_times(unsafe { GetCurrentProcess() }) else {
        return 0;
    };
    let now = windows_now();
    now.saturating_sub(created) / 10_000
}

fn windows_now() -> u64 {
    // FILETIME epoch is 1601-01-01; offset to UNIX epoch in 100ns units.
    const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() / 100)
        .unwrap_or(0);
    EPOCH_DIFF + u64::try_from(unix).unwrap_or(0)
}

fn ft(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

/// Named mutex held for the process lifetime; `None` if another instance owns it.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    pub fn acquire(name: &str) -> Option<Self> {
        let wide = HSTRING::from(format!("Local\\{name}"));
        // SAFETY: handle is closed in Drop.
        unsafe {
            let h = CreateMutexW(None, true, &wide).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(h);
                return None;
            }
            Some(Self(h))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: windows::core::PCWSTR = w!("HyperClips");

/// Add or remove the per-user autostart entry (HKCU\...\Run).
pub fn set_autostart(enabled: bool, exe: &Path) -> windows::core::Result<()> {
    // SAFETY: registry calls with valid, NUL-terminated wide strings.
    unsafe {
        if enabled {
            let cmd: Vec<u16> = format!("\"{}\" --background", exe.display())
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr().cast()),
                (cmd.len() * 2) as u32,
            )
            .ok()
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            // Deleting a missing value is fine.
            if r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                r.ok()
            }
        }
    }
}

pub fn autostart_enabled() -> bool {
    // SAFETY: size query only.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
        .is_ok()
    }
}

/// Opt our executables out of Windows Error Reporting for the current user,
/// so crash dumps are never uploaded to Microsoft.
pub fn exclude_from_wer(exe_names: &[&str]) {
    use windows::Win32::System::ErrorReporting::WerAddExcludedApplication;
    for name in exe_names {
        let wide = HSTRING::from(*name);
        // SAFETY: valid wide string; per-user (bAllUsers = false), no admin needed.
        if let Err(e) = unsafe { WerAddExcludedApplication(&wide, false) } {
            tracing::warn!(exe = name, error = %e, "WER exclusion failed");
        }
    }
}

/// Local time as `YYYY-MM-DD HH-MM-SS` (safe in file names).
pub fn local_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: plain query.
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// PIDs of running processes whose image name matches `exe_name` (case-insensitive).
pub fn find_processes(exe_name: &str) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: snapshot handle closed below; entry struct sized correctly.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case(exe_name) {
                out.push(entry.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Root PIDs of the process trees whose image name is in `names`
/// (a match whose parent is also a match is not a root).
pub fn process_tree_roots(names: &[String]) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut all: Vec<(u32, u32, bool)> = Vec::new();
    // SAFETY: snapshot handle closed below; entry struct sized correctly.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            let hit = names.iter().any(|n| n.eq_ignore_ascii_case(&name));
            all.push((e.th32ProcessID, e.th32ParentProcessID, hit));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    all.iter()
        .filter(|(_, parent, hit)| *hit && !all.iter().any(|(p, _, h)| p == parent && *h))
        .map(|(pid, _, _)| *pid)
        .collect()
}

/// Restore and bring to front the main window of `pid`. Returns false if none found.
pub fn focus_process_window(pid: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    struct Search {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        // SAFETY: lparam is the &mut Search passed below, alive during EnumWindows.
        let s = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut wpid = 0u32;
        // SAFETY: plain Win32 queries on a window handle provided by EnumWindows.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut wpid));
            if wpid == s.pid
                && IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).is_err()
            {
                s.found = Some(hwnd);
                return false.into();
            }
        }
        true.into()
    }
    let mut search = Search { pid, found: None };
    // SAFETY: callback only touches `search` during the call.
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut search as *mut Search as isize));
        match search.found {
            Some(hwnd) => {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd).as_bool()
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_is_a_tree_root() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(process_tree_roots(&[name]).contains(&std::process::id()));
    }

    #[test]
    fn finds_own_process() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(find_processes(&name).contains(&std::process::id()));
    }

    #[test]
    fn memory_is_reported() {
        let m = current_memory();
        assert!(m.working_set > 0);
        assert!(m.private_bytes > 0);
    }

    #[test]
    fn uptime_is_sane() {
        assert!(ms_since_process_start() < 10 * 60 * 1000);
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(10 * 1024 * 1024), "10.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn single_instance_is_exclusive() {
        let name = format!("HyperClips.test.{}", std::process::id());
        let a = SingleInstance::acquire(&name);
        assert!(a.is_some());
        // Same process re-opening an owned mutex still reports ALREADY_EXISTS.
        assert!(SingleInstance::acquire(&name).is_none());
        drop(a);
        assert!(SingleInstance::acquire(&name).is_some());
    }
}
