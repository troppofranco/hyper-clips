//! GitHub release metadata and version comparison.

use crate::{Result, UpdateError};
use semver::Version;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct RawRelease {
    tag_name: String,
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    fn from_raw(raw: RawRelease) -> Result<Self> {
        let version = Version::parse(raw.tag_name.trim_start_matches('v'))
            .map_err(|e| UpdateError::Parse(format!("tag '{}': {e}", raw.tag_name)))?;
        Ok(Self {
            version,
            tag: raw.tag_name,
            notes: raw.body.unwrap_or_default(),
            prerelease: raw.prerelease,
            assets: raw
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                    size: a.size,
                })
                .collect(),
        })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The package for this kind of installation: the Setup program or the portable zip.
    pub fn package(&self, portable: bool) -> Option<&Asset> {
        self.assets.iter().find(|a| {
            if portable {
                a.name.starts_with("HyperClips-") && a.name.ends_with("-portable.zip")
            } else {
                a.name.starts_with("HyperClips-Setup-") && a.name.ends_with(".exe")
            }
        })
    }
}

/// Parses the JSON of `releases/latest`.
pub fn parse_release(json: &str) -> Result<Release> {
    let raw: RawRelease =
        serde_json::from_str(json).map_err(|e| UpdateError::Parse(e.to_string()))?;
    Release::from_raw(raw)
}

/// Parses the JSON array of `releases`, skipping drafts and unparsable tags.
pub fn parse_release_list(json: &str) -> Result<Vec<Release>> {
    let raw: Vec<RawRelease> =
        serde_json::from_str(json).map_err(|e| UpdateError::Parse(e.to_string()))?;
    Ok(raw
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Release::from_raw(r).ok())
        .collect())
}

/// Highest version in the list, ignoring pre-releases unless `include_prerelease`.
pub fn newest(releases: Vec<Release>, include_prerelease: bool) -> Option<Release> {
    releases
        .into_iter()
        .filter(|r| include_prerelease || !r.prerelease)
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// True when `candidate` is strictly newer than the running version.
pub fn is_newer(current: &str, candidate: &Version) -> bool {
    Version::parse(current.trim_start_matches('v')).is_ok_and(|c| *candidate > c)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LATEST: &str = r#"{
      "tag_name": "v0.1.7", "name": "Hyper Clips 0.1.7", "draft": false, "prerelease": false,
      "body": "- fixed things",
      "assets": [
        {"name": "HyperClips-Setup-0.1.7.exe", "browser_download_url": "https://github.com/x/y/releases/download/v0.1.7/HyperClips-Setup-0.1.7.exe", "size": 100},
        {"name": "HyperClips-0.1.7-portable.zip", "browser_download_url": "https://github.com/x/y/a.zip", "size": 50},
        {"name": "checksums.txt", "browser_download_url": "https://github.com/x/y/c.txt", "size": 10}
      ]
    }"#;

    #[test]
    fn parses_a_release_and_picks_packages() {
        let r = parse_release(LATEST).unwrap();
        assert_eq!(r.version, Version::new(0, 1, 7));
        assert_eq!(r.notes, "- fixed things");
        assert_eq!(r.package(false).unwrap().name, "HyperClips-Setup-0.1.7.exe");
        assert_eq!(
            r.package(true).unwrap().name,
            "HyperClips-0.1.7-portable.zip"
        );
        assert!(r.asset("checksums.txt").is_some());
        assert!(r.asset("nope").is_none());
    }

    #[test]
    fn version_comparison_is_semver_not_text() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(is_newer("0.1.9", &v("0.1.10")));
        assert!(!is_newer("0.1.10", &v("0.1.9")));
        assert!(!is_newer("0.1.7", &v("0.1.7")));
        assert!(is_newer("v0.1.0", &v("0.2.0")));
        // An unparsable running version never triggers an update.
        assert!(!is_newer("garbage", &v("9.9.9")));
    }

    #[test]
    fn list_skips_drafts_and_respects_channel() {
        let json = r#"[
          {"tag_name":"v0.2.0-beta.1","prerelease":true,"draft":false,"assets":[]},
          {"tag_name":"v0.1.5","prerelease":false,"draft":false,"assets":[]},
          {"tag_name":"v9.9.9","prerelease":false,"draft":true,"assets":[]},
          {"tag_name":"not-a-version","prerelease":false,"draft":false,"assets":[]}
        ]"#;
        let list = parse_release_list(json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(newest(list.clone(), false).unwrap().tag, "v0.1.5");
        assert_eq!(newest(list, true).unwrap().tag, "v0.2.0-beta.1");
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(parse_release("{").is_err());
        assert!(parse_release(r#"{"tag_name":"x","assets":[]}"#).is_err());
    }
}
