//! Crate-wide error type.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse error: {0}")]
    ConfigParse(#[from] toml::de::Error),
    #[error("config serialize error: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),
    #[error("IPC error: {0}")]
    Ipc(String),
    #[error("invalid hotkey: {0}")]
    Hotkey(String),
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("another instance is already running")]
    AlreadyRunning,
    #[error("database error: {0}")]
    Database(String),
    #[error("system error: {0}")]
    System(String),
}

pub type Result<T> = std::result::Result<T, Error>;
