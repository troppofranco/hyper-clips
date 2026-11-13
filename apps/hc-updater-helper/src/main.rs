//! Update helper. The service copies it to a temp folder, starts it with the verified
//! package and exits. The helper waits for the service to end, backs up the program files,
//! applies the update and restores the backup if anything fails.
//!
//! Only program files are touched (`*.exe`, `lang\`); user data (`data\`, `clips\`) never is.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("hc-updater-helper runs on Windows only.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(opts) = windows_impl::Options::parse(&args) else {
        std::process::exit(2);
    };
    let log = std::env::temp_dir().join("hyperclips-update.log");
    let outcome = windows_impl::run(&opts, &log);
    std::process::exit(i32::from(outcome.is_err()));
}

#[cfg(windows)]
mod windows_impl {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    pub struct Options {
        pub pid: u32,
        pub app_dir: PathBuf,
        pub package: PathBuf,
        pub portable: bool,
        pub exe: String,
    }

    impl Options {
        pub fn parse(args: &[String]) -> Option<Self> {
            let value = |flag: &str| {
                args.iter()
                    .position(|a| a == flag)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            Some(Self {
                pid: value("--pid")?.parse().ok()?,
                app_dir: PathBuf::from(value("--app-dir")?),
                package: PathBuf::from(value("--package")?),
                portable: args.iter().any(|a| a == "--portable"),
                exe: value("--exe").unwrap_or_else(|| "hyperclips.exe".to_owned()),
            })
        }
    }

    fn log_line(log: &Path, text: &str) {
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{text}");
        }
    }

    fn wait_for_exit(pid: u32) {
        // SAFETY: plain handle API; a failure to open means the process is already gone.
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                let _ = WaitForSingleObject(handle, 30_000);
                let _ = CloseHandle(handle);
            }
        }
    }

    /// Program files only: top-level executables and the lang folder.
    fn program_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                let is_exe = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"));
                if (p.is_file() && is_exe) || (p.is_dir() && e.file_name() == "lang") {
                    out.push(p);
                }
            }
        }
        out
    }

    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        if from.is_dir() {
            fs::create_dir_all(to)?;
            for e in fs::read_dir(from)? {
                let e = e?;
                copy_tree(&e.path(), &to.join(e.file_name()))?;
            }
            Ok(())
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(from, to).map(|_| ())
        }
    }

    fn backup(app_dir: &Path, backup_dir: &Path) -> std::io::Result<()> {
        let _ = fs::remove_dir_all(backup_dir);
        fs::create_dir_all(backup_dir)?;
        for item in program_files(app_dir) {
            if let Some(name) = item.file_name() {
                copy_tree(&item, &backup_dir.join(name))?;
            }
        }
        Ok(())
    }

    fn restore(backup_dir: &Path, app_dir: &Path) -> std::io::Result<()> {
        for item in program_files(backup_dir) {
            if let Some(name) = item.file_name() {
                copy_tree(&item, &app_dir.join(name))?;
            }
        }
        Ok(())
    }

    fn run_installer(opts: &Options, log: &Path) -> Result<(), String> {
        let status = Command::new(&opts.package)
            .args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CLOSEAPPLICATIONS",
            ])
            .arg(format!("/DIR={}", opts.app_dir.display()))
            .status()
            .map_err(|e| format!("cannot start the installer: {e}"))?;
        log_line(log, &format!("installer exit code: {status}"));
        if status.success() {
            Ok(())
        } else {
            Err(format!("installer failed: {status}"))
        }
    }

    fn apply_portable(opts: &Options, log: &Path) -> Result<(), String> {
        let temp = std::env::temp_dir()
            .join("HyperClipsUpdate")
            .join("extract");
        let _ = fs::remove_dir_all(&temp);
        let quote = |p: &Path| p.display().to_string().replace('\'', "''");
        let script = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            quote(&opts.package),
            quote(&temp)
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(|e| format!("cannot run PowerShell: {e}"))?;
        if !status.success() {
            return Err(format!("unzip failed: {status}"));
        }
        // The zip holds one top-level folder.
        let root = fs::read_dir(&temp)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .ok_or("the zip is empty")?;
        let new_files = program_files(&root);
        if !new_files
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == opts.exe.as_str()))
        {
            return Err("the zip does not contain the program".into());
        }
        for item in new_files {
            if let Some(name) = item.file_name() {
                copy_tree(&item, &opts.app_dir.join(name)).map_err(|e| e.to_string())?;
            }
        }
        log_line(log, "portable files replaced");
        Ok(())
    }

    fn launch(app_dir: &Path, exe: &str) {
        let _ = Command::new(app_dir.join(exe)).current_dir(app_dir).spawn();
    }

    pub fn run(opts: &Options, log: &Path) -> Result<(), String> {
        log_line(
            log,
            &format!("--- update to {} ---", opts.package.display()),
        );
        wait_for_exit(opts.pid);
        // The UI may still be open and would lock its executable.
        let _ = Command::new("taskkill")
            .args(["/f", "/im", "hyperclips-ui.exe"])
            .status();
        std::thread::sleep(Duration::from_millis(500));

        let backup_dir = opts.app_dir.with_extension("bak");
        backup(&opts.app_dir, &backup_dir).map_err(|e| format!("backup failed: {e}"))?;

        let result = if opts.portable {
            apply_portable(opts, log)
        } else {
            run_installer(opts, log)
        };

        match result {
            Ok(()) => {
                let _ = fs::remove_dir_all(&backup_dir);
                // The Setup program relaunches the app itself; portable needs us to do it.
                if opts.portable {
                    launch(&opts.app_dir, &opts.exe);
                }
                log_line(log, "update applied");
                Ok(())
            }
            Err(e) => {
                log_line(
                    log,
                    &format!("update failed ({e}); restoring the previous version"),
                );
                let restored = restore(&backup_dir, &opts.app_dir);
                log_line(log, &format!("restore: {restored:?}"));
                launch(&opts.app_dir, &opts.exe);
                Err(e)
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::*;

        fn strings(v: &[&str]) -> Vec<String> {
            v.iter().map(|s| (*s).to_owned()).collect()
        }

        #[test]
        fn parses_arguments() {
            let o = Options::parse(&strings(&[
                "--pid",
                "42",
                "--app-dir",
                "C:\\App",
                "--package",
                "C:\\p.exe",
                "--portable",
            ]))
            .unwrap();
            assert_eq!(o.pid, 42);
            assert!(o.portable);
            assert_eq!(o.exe, "hyperclips.exe");
            assert!(Options::parse(&strings(&["--pid", "x"])).is_none());
        }

        #[test]
        fn backup_and_restore_touch_only_program_files() {
            let base = std::env::temp_dir().join(format!("hc-helper-test-{}", std::process::id()));
            let app = base.join("app");
            fs::create_dir_all(app.join("lang")).unwrap();
            fs::create_dir_all(app.join("clips")).unwrap();
            fs::write(app.join("hyperclips.exe"), b"old").unwrap();
            fs::write(app.join("lang").join("en.toml"), b"x").unwrap();
            fs::write(app.join("clips").join("big.mp4"), b"video").unwrap();
            let bak = base.join("app.bak");

            backup(&app, &bak).unwrap();
            assert!(bak.join("hyperclips.exe").is_file());
            assert!(bak.join("lang").join("en.toml").is_file());
            assert!(
                !bak.join("clips").exists(),
                "user clips must never be backed up"
            );

            fs::write(app.join("hyperclips.exe"), b"broken").unwrap();
            restore(&bak, &app).unwrap();
            assert_eq!(fs::read(app.join("hyperclips.exe")).unwrap(), b"old");
            assert!(app.join("clips").join("big.mp4").is_file());
            fs::remove_dir_all(&base).unwrap();
        }
    }
}
