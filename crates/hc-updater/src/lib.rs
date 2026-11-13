//! Self-update support. Privacy notes: the only requests made are plain HTTPS GETs to
//! `api.github.com` (release metadata) and the asset URLs GitHub returns for the release
//! (served from `github.com` / `*.githubusercontent.com`). No identifiers are sent.

pub mod net;
pub mod release;
pub mod verify;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("network error: {0}")]
    Http(String),
    #[error("cannot read release data: {0}")]
    Parse(String),
    #[error("verification failed: {0}")]
    Verify(String),
    #[error("this build has no update signing key, updates are disabled")]
    NoKey,
    #[error("the release has no suitable asset: {0}")]
    MissingAsset(String),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

/// Public minisign key trusted for updates. The file is replaced by the project owner
/// (see docs/RELEASING.md); while it is empty, updates are refused.
pub const PUBLIC_KEY: &str = include_str!("../minisign.pub");
