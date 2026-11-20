//! `plugin.toml` manifest parsing and plugin discovery.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{API_VERSION, MANIFEST_FILE, PluginError, WASM_FILE};

/// Maximum size of a `plugin.toml` file in bytes.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// A capability a plugin may request in its manifest.
///
/// These are the only capabilities that exist. Logging is always allowed and
/// therefore not a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// May call `hc.add_tag` to attach tags to the saved clip.
    Tags,
    /// May call `hc.notify` to ask the host to show a notification.
    Notify,
}

/// Contents of a plugin's `plugin.toml`.
///
/// ```toml
/// id = "auto-tagger"
/// name = "Auto Tagger"
/// version = "1.0.0"
/// description = "Tags clips by game"
/// author = "Someone"
/// api = 1
/// permissions = ["tags", "notify"]
/// ```
///
/// Unknown keys are ignored (forward compatibility); unknown permissions are
/// an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Stable identifier: 1-64 chars of `[a-z0-9_.-]`, must equal the folder name.
    pub id: String,
    /// Human readable name.
    pub name: String,
    /// Plugin version (free-form, semver recommended).
    pub version: String,
    /// Short description.
    #[serde(default)]
    pub description: String,
    /// Author name / contact.
    #[serde(default)]
    pub author: String,
    /// Plugin ABI version. Only [`API_VERSION`] (1) is supported.
    pub api: u32,
    /// Requested capabilities.
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

impl PluginManifest {
    /// Parses and validates a manifest from TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self, PluginError> {
        let manifest: Self =
            toml::from_str(text).map_err(|e| PluginError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Reads `<dir>/plugin.toml`, parses it and checks that `id` matches the
    /// folder name.
    pub fn from_dir(dir: &Path) -> Result<Self, PluginError> {
        let path = dir.join(MANIFEST_FILE);
        let meta = fs::metadata(&path).map_err(|e| PluginError::io(&path, e))?;
        if meta.len() > MAX_MANIFEST_BYTES {
            return Err(PluginError::Manifest(format!(
                "{MANIFEST_FILE} is larger than {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        let text = fs::read_to_string(&path).map_err(|e| PluginError::io(&path, e))?;
        let manifest = Self::from_toml_str(&text)?;
        let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if folder != manifest.id {
            return Err(PluginError::Manifest(format!(
                "id {:?} does not match folder name {folder:?}",
                manifest.id
            )));
        }
        Ok(manifest)
    }

    /// Returns `true` if the manifest grants `permission`.
    pub fn has_permission(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    fn validate(&self) -> Result<(), PluginError> {
        if !is_valid_id(&self.id) {
            return Err(PluginError::Manifest(format!(
                "invalid id {:?}: use 1-64 chars of [a-z0-9_.-], not starting with '.'",
                self.id
            )));
        }
        if self.name.trim().is_empty() {
            return Err(PluginError::Manifest("name must not be empty".into()));
        }
        if self.api != API_VERSION {
            return Err(PluginError::UnsupportedApi(self.api));
        }
        Ok(())
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

/// Scans `dir` for plugin folders (sub-directories containing `plugin.toml`).
///
/// Returns one entry per sub-directory that contains a `plugin.toml`: the
/// parsed manifest, or the folder path and the reason it is invalid
/// (including a missing `plugin.wasm`). Entries are sorted by folder name.
/// A missing `dir` yields an empty list; an unreadable one yields a single
/// error entry for `dir`. Folders without `plugin.toml` are ignored.
pub fn discover(dir: &Path) -> Vec<Result<PluginManifest, (PathBuf, PluginError)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![Err((dir.to_path_buf(), PluginError::io(dir, e)))],
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST_FILE).is_file())
        .collect();
    folders.sort();
    folders
        .into_iter()
        .map(|folder| {
            let manifest = PluginManifest::from_dir(&folder).map_err(|e| (folder.clone(), e))?;
            let wasm = folder.join(WASM_FILE);
            if !wasm.is_file() {
                return Err((
                    folder,
                    PluginError::io(
                        &wasm,
                        std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
                    ),
                ));
            }
            Ok(manifest)
        })
        .collect()
}
