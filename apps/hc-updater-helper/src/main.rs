//! hc-updater-helper — replaces the application files after it has exited.
//!
//! Runs from a temp copy (the install folder is being replaced):
//!   hc-updater-helper --package <file> --mode installed|portable
//!                     --target <install dir> --data <data dir> [--wait <pid>]...
//!
//! 1. wait for the app processes to exit;
//! 2. back up the program files (never user data);
//! 3. installed: run the verified Setup.exe silently;
//!    portable: extract the verified zip over the folder;
//! 4. start the new version with a health check; if it does not report
//!    healthy within 30 s, restore the backup and restart the old version.
//!
//! The package was already verified (SHA-256) by the app.

#![windows_subsystem = "windows"]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
/// Program files we own in the install folder (user data lives elsewhere,
/// or under `data\` in portable mode, which is never touched).
const PROGRAM_EXTENSIONS: &[&str] = &["exe", "dll", "ftl", "toml"];

struct Args {
    package: PathBuf,
    portable: bool,
    target: PathBuf,
    data: PathBuf,
    wait: Vec<u32>,
}

fn parse_args() -> Option<Args> {
    let mut a = std::env::args().skip(1);
    let (mut package, mut mode, mut target, mut data, mut wait) =
        (None, None, None, None, Vec::new());
    while let Some(k) = a.next() {
        match k.as_str() {
            "--package" => package = a.next().map(PathBuf::from),
            "--mode" => mode = a.next(),
            "--target" => target = a.next().map(PathBuf::from),
            "--data" => data = a.next().map(PathBuf::from),
            "--wait" => {
                if let Some(p) = a.next().and_then(|s| s.parse().ok()) {
                    wait.push(p);
                }
            }
            _ => {}
        }
    }
    Some(Args {
        package: package?,
        portable: mode? == "portable",
        target: target?,
        data: data?,
        wait,
    })
}

struct Log(Option<std::fs::File>);

impl Log {
    fn line(&mut self, msg: &str) {
        if let Some(f) = &mut self.0 {
            let _ = writeln!(f, "{msg}");
        }
    }
}

fn wait_for_exit(pid: u32, timeout: Duration) {
    // SAFETY: handle opened with SYNCHRONIZE only and closed below.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, timeout.as_millis() as u32) == WAIT_OBJECT_0;
            let _ = CloseHandle(h);
        }
    }
}

fn is_program_file(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| PROGRAM_EXTENSIONS.contains(&e.as_str()))
}

/// Copy program files (top level + `locales\`) of `from` into `to`.
fn copy_program(from: &Path, to: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_file() && is_program_file(&p) {
            let dest = to.join(e.file_name());
            // The helper normally runs from a temp copy, but never try to
            // overwrite our own running image if it was started in place.
            if is_self(&dest) {
                continue;
            }
            std::fs::copy(&p, dest)?;
            n += 1;
        } else if p.is_dir() && e.file_name() == "locales" {
            n += copy_program_tree(&p, &to.join("locales"))?;
        }
    }
    Ok(n)
}

fn is_self(p: &Path) -> bool {
    let canon = |x: &Path| std::fs::canonicalize(x).ok();
    std::env::current_exe()
        .ok()
        .and_then(|me| canon(&me))
        .is_some_and(|me| canon(p) == Some(me))
}

fn copy_program_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += copy_program_tree(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
            n += 1;
        }
    }
    Ok(n)
}

fn extract_zip(zip_path: &Path, target: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut n = 0;
    for k in 0..zip.len() {
        let mut entry = zip.by_index(k).map_err(|e| e.to_string())?;
        // Reject absolute paths / ".." (zip-slip).
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        // Strip a single top-level folder if the zip has one.
        let rel: PathBuf = rel
            .components()
            .skip_while(|c| c.as_os_str().to_string_lossy().starts_with("HyperClips-"))
            .collect();
        if rel.as_os_str().is_empty() || rel.starts_with("data") {
            continue; // never overwrite user data
        }
        let out = target.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

/// Minimal PE check ("MZ" + "PE\0\0" at e_lfanew) so a corrupt binary is
/// rejected without asking Windows to run it (which can pop a modal dialog).
fn looks_like_pe(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    if data.len() < 0x40 || &data[..2] != b"MZ" {
        return false;
    }
    let off = u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
    data.get(off..off + 4) == Some(b"PE\0\0")
}

/// Start the new core with a health-check file; true if it reports healthy.
fn healthy(target: &Path, marker: &Path) -> bool {
    let _ = std::fs::remove_file(marker);
    let exe = target.join("hyperclips.exe");
    if !looks_like_pe(&exe) {
        return false;
    }
    let Ok(mut child) = std::process::Command::new(&exe)
        .arg("--background")
        .arg("--health-file")
        .arg(marker)
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    while start.elapsed() < HEALTH_TIMEOUT {
        if marker.exists() {
            return true;
        }
        if let Ok(Some(_)) = child.try_wait() {
            return false; // crashed on start
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    false
}

fn run(a: &Args, log: &mut Log) -> Result<(), String> {
    for pid in &a.wait {
        wait_for_exit(*pid, Duration::from_secs(60));
    }
    std::thread::sleep(Duration::from_millis(500));
    let backup = a.data.join("rollback");
    let _ = std::fs::remove_dir_all(&backup);
    let n = copy_program(&a.target, &backup).map_err(|e| format!("backup failed: {e}"))?;
    log.line(&format!("backed up {n} program files"));

    let applied = if a.portable {
        extract_zip(&a.package, &a.target).map(|n| log.line(&format!("extracted {n} files")))
    } else {
        // Inno Setup silent install into the same per-user folder.
        let status = std::process::Command::new(&a.package)
            .args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/NOCANCEL",
                "/CLOSEAPPLICATIONS",
            ])
            .arg(format!("/DIR={}", a.target.display()))
            .status()
            .map_err(|e| e.to_string())?;
        log.line(&format!("setup exit code {:?}", status.code()));
        if status.success() {
            Ok(())
        } else {
            Err(format!("setup failed with code {:?}", status.code()))
        }
    };

    let marker = a.data.join("update-health.ok");
    if applied.is_ok() && healthy(&a.target, &marker) {
        log.line("new version is healthy");
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_file(&a.package);
        return Ok(());
    }
    // Roll back: restore the previous program files and restart them.
    log.line(&format!("update failed ({applied:?}); rolling back"));
    copy_program(&backup, &a.target).map_err(|e| format!("rollback failed: {e}"))?;
    let _ = std::process::Command::new(a.target.join("hyperclips.exe"))
        .arg("--background")
        .spawn();
    Err("rolled back to the previous version".into())
}

fn main() -> ExitCode {
    // Never block on system error dialogs while nobody may be watching.
    // SAFETY: process-wide error mode flags.
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    let Some(args) = parse_args() else {
        return ExitCode::from(2);
    };
    let _ = std::fs::create_dir_all(&args.data);
    let mut log = Log(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(args.data.join("update.log"))
        .ok());
    log.line(&format!(
        "update {} -> {}",
        args.package.display(),
        args.target.display()
    ));
    match run(&args, &mut log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log.line(&format!("error: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pe_check() {
        let me = std::env::current_exe().unwrap();
        assert!(looks_like_pe(&me));
        let bad = std::env::temp_dir().join(format!("hc-notpe-{}.exe", std::process::id()));
        std::fs::write(&bad, b"not a program").unwrap();
        assert!(!looks_like_pe(&bad));
        std::fs::remove_file(bad).unwrap();
    }

    #[test]
    fn program_file_filter() {
        assert!(is_program_file(Path::new("hyperclips.exe")));
        assert!(is_program_file(Path::new("x/main.FTL")));
        assert!(!is_program_file(Path::new("library.db")));
        assert!(!is_program_file(Path::new("clip.mp4")));
    }

    #[test]
    fn zip_extraction_skips_data_and_strips_root() {
        let dir = std::env::temp_dir().join(format!("hc-helper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zpath = dir.join("p.zip");
        {
            let f = std::fs::File::create(&zpath).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opt: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            z.start_file("HyperClips-1.0.0/hyperclips.exe", opt)
                .unwrap();
            z.write_all(b"new").unwrap();
            z.start_file("HyperClips-1.0.0/data/library.db", opt)
                .unwrap();
            z.write_all(b"must not overwrite").unwrap();
            z.finish().unwrap();
        }
        let target = dir.join("app");
        std::fs::create_dir_all(target.join("data")).unwrap();
        std::fs::write(target.join("data").join("library.db"), b"user").unwrap();
        assert_eq!(extract_zip(&zpath, &target).unwrap(), 1);
        assert_eq!(
            std::fs::read(target.join("hyperclips.exe")).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(target.join("data").join("library.db")).unwrap(),
            b"user"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
