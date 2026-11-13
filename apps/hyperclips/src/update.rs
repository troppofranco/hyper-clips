//! In-app update flow (Windows only): check, ask, download, verify, hand over to the helper.

use anyhow::{anyhow, Context, Result};
use hc_core::config::{Channel, Config};
use hc_core::consts::{REPO_NAME, REPO_OWNER};
use hc_core::i18n::Translator;
use hc_core::paths::AppPaths;
use hc_os_win::dialog::{self, Choice};
use hc_updater::net::Client;
use hc_updater::release::is_newer;
use hc_updater::verify;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct UpdateCtx {
    pub paths: AppPaths,
    pub config_path: PathBuf,
    pub config: Mutex<Config>,
    pub tr: Translator,
    /// Called once the helper is running, to make the service exit.
    pub quit: Box<dyn Fn() + Send + Sync>,
    busy: AtomicBool,
}

impl UpdateCtx {
    pub fn new(
        paths: AppPaths,
        config_path: PathBuf,
        config: Config,
        tr: Translator,
        quit: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            paths,
            config_path,
            config: Mutex::new(config),
            tr,
            quit,
            busy: AtomicBool::new(false),
        }
    }

    /// Starts a check on its own thread. Automatic checks wait a little so they never
    /// slow down start-up.
    pub fn spawn_check(self: &Arc<Self>, manual: bool) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return; // a check is already running
        }
        let ctx = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("hc-update".into())
            .spawn(move || {
                if !manual {
                    std::thread::sleep(Duration::from_secs(10));
                }
                if let Err(e) = ctx.check(manual) {
                    tracing::warn!("update failed: {e:#}");
                    if manual {
                        dialog::info(
                            ctx.tr.t("update_title"),
                            &ctx.text("update_failed", &[("error", &format!("{e:#}"))]),
                        );
                    }
                }
                ctx.busy.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.busy.store(false, Ordering::SeqCst);
        }
    }

    fn text(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let mut s = self.tr.t(key).to_owned();
        for (name, value) in vars {
            s = s.replace(&format!("{{{name}}}"), value);
        }
        s
    }

    fn check(&self, manual: bool) -> Result<()> {
        let (beta, skipped) = {
            let c = self
                .config
                .lock()
                .map_err(|_| anyhow!("config lock poisoned"))?;
            (
                c.updates.channel == Channel::Beta,
                c.updates.skip_version.clone(),
            )
        };
        let client = Client::new();
        let Some(release) = client.fetch_newest(REPO_OWNER, REPO_NAME, beta)? else {
            return Ok(());
        };
        if !is_newer(env!("CARGO_PKG_VERSION"), &release.version) {
            if manual {
                dialog::info(self.tr.t("update_title"), self.tr.t("update_uptodate"));
            }
            return Ok(());
        }
        let version = release.version.to_string();
        if !manual && skipped == version {
            return Ok(());
        }
        // Without a trusted key nothing can be verified, so nothing is ever installed.
        if verify::trusted_key().is_err() {
            tracing::warn!("version {version} is available but this build has no signing key");
            if manual {
                dialog::info(self.tr.t("update_title"), self.tr.t("update_no_key"));
            }
            return Ok(());
        }

        let portable = self.paths.is_portable();
        let package = release.package(portable).ok_or_else(|| {
            anyhow!(
                "the release has no {} package",
                if portable { "portable" } else { "setup" }
            )
        })?;
        let signature = release
            .asset(&format!("{}.minisig", package.name))
            .ok_or_else(|| anyhow!("the release is not signed"))?;
        let checksums = release
            .asset("checksums.txt")
            .ok_or_else(|| anyhow!("the release has no checksums.txt"))?;

        let notes: String = release.notes.chars().take(800).collect();
        let question = self.text(
            "update_available",
            &[("version", &version), ("notes", &notes)],
        );
        match dialog::ask_yes_no_cancel(self.tr.t("update_title"), &question) {
            Choice::Yes => {}
            Choice::No => return Ok(()),
            Choice::Cancel => {
                let mut c = self
                    .config
                    .lock()
                    .map_err(|_| anyhow!("config lock poisoned"))?;
                c.updates.skip_version = version;
                if let Err(e) = c.save(&self.config_path) {
                    tracing::warn!("cannot remember the skipped version: {e}");
                }
                return Ok(());
            }
        }

        let data = client.download(&package.url).context("download failed")?;
        let checksums_text = client
            .download_text(&checksums.url)
            .context("checksums download failed")?;
        verify::verify_checksum(&data, &package.name, &checksums_text)?;
        let signature_text = client
            .download_text(&signature.url)
            .context("signature download failed")?;
        verify::verify_signature(&signature_text, &data)?;

        // Stage the verified package and a private copy of the helper outside the app folder,
        // because the installer replaces the app folder's files.
        let stage = std::env::temp_dir().join("HyperClipsUpdate");
        std::fs::create_dir_all(&stage)?;
        let package_path = stage.join(&package.name);
        std::fs::write(&package_path, &data)?;
        let helper_src = self.paths.exe_dir().join("hc-updater-helper.exe");
        let helper = stage.join("hc-updater-helper.exe");
        std::fs::copy(&helper_src, &helper)
            .with_context(|| format!("cannot copy {}", helper_src.display()))?;

        let mut cmd = Command::new(&helper);
        cmd.arg("--pid")
            .arg(std::process::id().to_string())
            .arg("--app-dir")
            .arg(self.paths.exe_dir())
            .arg("--package")
            .arg(&package_path);
        if portable {
            cmd.arg("--portable");
        }
        cmd.spawn().context("cannot start the update helper")?;
        tracing::info!("update to {version} handed to the helper, exiting");
        (self.quit)();
        Ok(())
    }
}
