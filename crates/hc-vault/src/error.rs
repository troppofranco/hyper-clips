//! Error type for vault operations.

use std::io;

use crate::EntryId;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while working with a vault.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An underlying filesystem or OS error (including failure of the OS
    /// random number generator).
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The password did not unlock the vault header.
    ///
    /// AEAD cannot distinguish "wrong password" from "header was tampered
    /// with"; both produce this error.
    #[error("wrong password")]
    WrongPassword,

    /// Encrypted data failed authentication or is structurally invalid
    /// (tampering, truncation, reordering or disk corruption).
    #[error("vault data is corrupt or has been tampered with")]
    Corrupt,

    /// The requested entry does not exist (or its encrypted file is gone).
    #[error("entry not found")]
    NotFound,

    /// The vault header or a parameter is malformed or unsupported.
    #[error("invalid vault format: {0}")]
    Format(String),

    /// [`crate::Vault::create`] was called on a directory that already holds
    /// a vault.
    #[error("a vault already exists at this location")]
    AlreadyExists,

    /// The file was encrypted and added to the vault as `id`, but removing
    /// the plaintext original failed. The entry is valid and usable.
    #[error("entry {id} was added but the original file could not be deleted: {source}")]
    OriginalNotDeleted {
        /// The id of the entry that was successfully added.
        id: EntryId,
        /// Why the original could not be deleted.
        source: io::Error,
    },
}
