use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::paths::InstallMode;

/// Upper bound for a single message; protects against corrupt length prefixes.
pub const MAX_FRAME: u32 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Codec(#[from] postcard::Error),
    #[error("frame too large: {0} bytes")]
    TooLarge(u32),
    #[error("core process is not running")]
    NotRunning,
    #[error("Windows error: {0}")]
    Os(#[from] windows::core::Error),
}

/// User-triggerable actions, shared by hotkeys, tray, CLI and studio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    SaveClip,
    ToggleRecording,
    Screenshot,
    AddMarker,
    OpenStudio,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::SaveClip,
        Action::ToggleRecording,
        Action::Screenshot,
        Action::AddMarker,
        Action::OpenStudio,
    ];

    /// Matches the ids in [`crate::config::Hotkeys::entries`].
    pub fn id(self) -> &'static str {
        match self {
            Action::SaveClip => "save-clip",
            Action::ToggleRecording => "toggle-recording",
            Action::Screenshot => "screenshot",
            Action::AddMarker => "add-marker",
            Action::OpenStudio => "open-studio",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Ping,
    Status,
    Run(Action),
    /// Re-read config.toml and re-register hotkeys.
    ReloadConfig,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyConflict {
    pub action: String,
    pub binding: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreStatus {
    pub version: String,
    pub mode: InstallMode,
    pub pid: u32,
    pub uptime_secs: u64,
    pub recording: bool,
    pub replay_active: bool,
    pub working_set_bytes: u64,
    pub private_bytes: u64,
    pub hotkey_conflicts: Vec<HotkeyConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Pong {
        version: String,
    },
    Status(CoreStatus),
    Ok,
    /// Config reloaded; lists hotkeys that could not be registered.
    Reloaded {
        conflicts: Vec<HotkeyConflict>,
    },
    /// The action exists but its subsystem is not available yet.
    Unavailable(String),
    Error(String),
}

/// Marker trait for types that can travel over the pipe.
pub trait Frame: Serialize + DeserializeOwned {}
impl Frame for Request {}
impl Frame for Response {}

pub fn write_frame<W: Write, T: Frame>(w: &mut W, msg: &T) -> Result<(), IpcError> {
    let body = postcard::to_stdvec(msg)?;
    let len = u32::try_from(body.len()).map_err(|_| IpcError::TooLarge(u32::MAX))?;
    if len > MAX_FRAME {
        return Err(IpcError::TooLarge(len));
    }
    // One write call per frame keeps message-mode pipes happy.
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

pub fn read_frame<R: Read, T: Frame>(r: &mut R) -> Result<T, IpcError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(IpcError::TooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(postcard::from_bytes(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_requests() {
        let reqs = [
            Request::Ping,
            Request::Status,
            Request::Run(Action::SaveClip),
            Request::ReloadConfig,
            Request::Shutdown,
        ];
        let mut buf = Vec::new();
        for r in &reqs {
            write_frame(&mut buf, r).unwrap();
        }
        let mut cur = Cursor::new(buf);
        for r in &reqs {
            let got: Request = read_frame(&mut cur).unwrap();
            assert_eq!(&got, r);
        }
    }

    #[test]
    fn roundtrip_status() {
        let resp = Response::Status(CoreStatus {
            version: "1.2.3".into(),
            mode: InstallMode::Portable,
            pid: 42,
            uptime_secs: 7,
            recording: true,
            replay_active: false,
            working_set_bytes: 1 << 24,
            private_bytes: 1 << 23,
            hotkey_conflicts: vec![HotkeyConflict {
                action: "save-clip".into(),
                binding: "F8".into(),
                reason: "taken".into(),
            }],
        });
        let mut buf = Vec::new();
        write_frame(&mut buf, &resp).unwrap();
        let got: Response = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, resp);
    }

    #[test]
    fn rejects_oversized_frame() {
        let mut buf = (MAX_FRAME + 1).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0; 8]);
        let err = read_frame::<_, Request>(&mut Cursor::new(buf)).unwrap_err();
        assert!(matches!(err, IpcError::TooLarge(_)));
    }

    #[test]
    fn truncated_frame_is_error() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::Run(Action::Screenshot)).unwrap();
        buf.pop();
        assert!(read_frame::<_, Request>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn action_ids_roundtrip() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        let cfg_ids: Vec<_> = crate::config::Hotkeys::default()
            .entries()
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let action_ids: Vec<_> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(cfg_ids, action_ids);
    }
}
