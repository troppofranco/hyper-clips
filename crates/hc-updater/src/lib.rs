//! Auto-update from the project's public GitHub Releases.
//!
//! Privacy: the only network traffic of the whole application lives here.
//! Requests carry a generic User-Agent and no identifiers; every request URL
//! is appended to a local `network.log` shown by the Privacy Audit.
//!
//! Integrity: the downloaded package must match its SHA-256 line in the
//! release's `checksums.txt` (both fetched over HTTPS from GitHub). This
//! catches corrupted or truncated downloads; authenticity relies on the
//! GitHub account that publishes the releases. Any mismatch aborts.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The one place to change the release source.
pub const OWNER: &str = "troppofranco";
pub const REPO: &str = "hyper-clips";
/// Generic on purpose: no version, no machine or user identifiers.
pub const USER_AGENT: &str = "HyperClips-Updater";
pub const CHECKSUMS: &str = "checksums.txt";

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("network: {0}")]
    Net(String),
    #[error("invalid release data: {0}")]
    Data(String),

    #[error("checksum mismatch for {0}")]
    Checksum(String),
    #[error("file system: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed with the Setup.exe: update by running the new installer.
    Installed,
    /// Portable folder: update by replacing files from the zip.
    Portable,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Public GitHub page of this release.
    pub fn page_url(&self) -> String {
        format!(
            "https://github.com/{OWNER}/{REPO}/releases/tag/{}",
            self.tag
        )
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    pub fn package_name(&self, kind: InstallKind) -> String {
        match kind {
            InstallKind::Installed => format!("HyperClips-Setup-{}.exe", self.version),
            InstallKind::Portable => format!("HyperClips-{}-portable.zip", self.version),
        }
    }
}

static NETWORK_LOG: OnceLock<PathBuf> = OnceLock::new();

/// Where request URLs are logged for the Privacy Audit.
pub fn set_network_log(path: PathBuf) {
    let _ = NETWORK_LOG.set(path);
}

fn log_request(url: &str) {
    tracing::info!(%url, "network request (update check)");
    if let Some(p) = NETWORK_LOG.get()
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{t} GET {url}");
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .build()
        .into()
}

fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>> {
    log_request(url);
    let mut resp = match agent()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        // No release published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(b"[]".to_vec()),
        Err(e) => return Err(UpdateError::Net(e.to_string())),
    };
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| UpdateError::Net(e.to_string()))
}

/// Releases from the GitHub API (stable: latest only; beta: recent ones).
pub fn fetch_releases(channel: Channel) -> Result<Vec<Release>> {
    let base = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases");
    let (url, many) = match channel {
        Channel::Stable => (format!("{base}/latest"), false),
        Channel::Beta => (format!("{base}?per_page=15"), true),
    };
    let body = get_bytes(&url, 4 * 1024 * 1024)?;
    let api: Vec<ApiRelease> = if many || body.as_slice() == b"[]" {
        serde_json::from_slice(&body).map_err(|e| UpdateError::Data(e.to_string()))?
    } else {
        vec![serde_json::from_slice(&body).map_err(|e| UpdateError::Data(e.to_string()))?]
    };
    Ok(api
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let v = Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            Some(Release {
                version: v,
                tag: r.tag_name,
                notes: r.body.unwrap_or_default(),
                prerelease: r.prerelease,
                assets: r.assets,
            })
        })
        .collect())
}

/// The newest release that is strictly newer than `current`, allowed by the
/// channel and not ignored by the user. Never returns a downgrade.
pub fn pick_update(
    releases: &[Release],
    current: &Version,
    channel: Channel,
    ignored: &[String],
) -> Option<Release> {
    releases
        .iter()
        .filter(|r| channel == Channel::Beta || (!r.prerelease && r.version.pre.is_empty()))
        .filter(|r| r.version > *current)
        .filter(|r| {
            !ignored
                .iter()
                .any(|i| i.trim_start_matches('v') == r.version.to_string())
        })
        .max_by(|a, b| a.version.cmp(&b.version))
        .cloned()
}

/// Check GitHub for an update.
pub fn check(channel: Channel, ignored: &[String]) -> Result<Option<Release>> {
    let current = Version::parse(hc_core::VERSION).map_err(|e| UpdateError::Data(e.to_string()))?;
    check_against(&current, channel, ignored)
}

/// Like [`check`], against an explicit current version.
pub fn check_against(
    current: &Version,
    channel: Channel,
    ignored: &[String],
) -> Result<Option<Release>> {
    let current = current.clone();
    Ok(pick_update(
        &fetch_releases(channel)?,
        &current,
        channel,
        ignored,
    ))
}

/// `<sha256>  <file name>` lines (sha256sum format).
pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let hash = parts.next()?.to_ascii_lowercase();
            let name = parts.next()?.trim_start_matches('*').to_owned();
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
                .then_some((name, hash))
        })
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn download(
    url: &str,
    dest: &Path,
    expected_size: u64,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    log_request(url);
    let mut resp = agent()
        .get(url)
        .call()
        .map_err(|e| UpdateError::Net(e.to_string()))?;
    let mut reader = resp.body_mut().with_config().limit(1 << 31).reader();
    let tmp = dest.with_extension("download");
    let mut out = std::fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
        if expected_size > 0 {
            progress((total as f64 / expected_size as f64).min(1.0) as f32);
        }
    }
    out.flush()?;
    drop(out);
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Download and fully verify the package for `kind`. Returns its path in
/// `dir`. On any verification error the downloaded file is deleted.
pub fn download_verified(
    release: &Release,
    kind: InstallKind,
    dir: &Path,
    progress: &mut dyn FnMut(f32),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let sums = release
        .asset(CHECKSUMS)
        .ok_or_else(|| UpdateError::Data("release has no checksums.txt".into()))?;
    let sums_data = get_bytes(&sums.url, 1 << 20)?;
    let name = release.package_name(kind);
    let expected = parse_checksums(&String::from_utf8_lossy(&sums_data))
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, h)| h)
        .ok_or_else(|| UpdateError::Data(format!("{name} not listed in checksums.txt")))?;
    let asset = release
        .asset(&name)
        .ok_or_else(|| UpdateError::Data(format!("release has no {name}")))?;
    let dest = dir.join(&name);
    download(&asset.url, &dest, asset.size, progress)?;
    let actual = sha256_file(&dest)?;
    if actual != expected {
        let _ = std::fs::remove_file(&dest);
        return Err(UpdateError::Checksum(name));
    }
    tracing::info!(file = %name, "update package verified (SHA-256)");
    Ok(dest)
}

/// Copy the helper to a temp folder (the install dir gets replaced) and start
/// it. The caller must then exit so files can be replaced.
pub fn launch_helper(
    helper_exe: &Path,
    package: &Path,
    kind: InstallKind,
    install_dir: &Path,
    data_dir: &Path,
    wait_pids: &[u32],
) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("hc-updater-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let helper = tmp.join("hc-updater-helper.exe");
    std::fs::copy(helper_exe, &helper)?;
    let mut cmd = std::process::Command::new(helper);
    cmd.arg("--package")
        .arg(package)
        .arg("--mode")
        .arg(match kind {
            InstallKind::Installed => "installed",
            InstallKind::Portable => "portable",
        })
        .arg("--target")
        .arg(install_dir)
        .arg("--data")
        .arg(data_dir);
    for pid in wait_pids {
        cmd.arg("--wait").arg(pid.to_string());
    }
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(v: &str, pre: bool) -> Release {
        Release {
            version: Version::parse(v).unwrap(),
            tag: format!("v{v}"),
            notes: String::new(),
            prerelease: pre,
            assets: vec![],
        }
    }

    #[test]
    fn picks_newest_allowed() {
        let cur = Version::parse("0.2.0").unwrap();
        let list = vec![
            rel("0.1.9", false),
            rel("0.2.1", false),
            rel("0.3.0-beta.1", true),
            rel("0.2.0", false),
        ];
        assert_eq!(
            pick_update(&list, &cur, Channel::Stable, &[])
                .unwrap()
                .version
                .to_string(),
            "0.2.1"
        );
        assert_eq!(
            pick_update(&list, &cur, Channel::Beta, &[])
                .unwrap()
                .version
                .to_string(),
            "0.3.0-beta.1"
        );
        assert!(pick_update(&list, &cur, Channel::Stable, &["v0.2.1".into()]).is_none());
        // Never a downgrade or same version.
        let old = vec![rel("0.1.0", false), rel("0.2.0", false)];
        assert!(pick_update(&old, &cur, Channel::Beta, &[]).is_none());
    }

    #[test]
    fn checksums_format() {
        let h = "a".repeat(64);
        let text = format!(
            "{h}  HyperClips-Setup-1.0.0.exe\n{h} *HyperClips-1.0.0-portable.zip\nbad line\n"
        );
        let list = parse_checksums(&text);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].0, "HyperClips-1.0.0-portable.zip");
    }
}
