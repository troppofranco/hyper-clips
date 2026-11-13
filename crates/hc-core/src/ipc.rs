//! Service <-> UI/CLI protocol: JSON lines over a per-user named pipe.

pub mod frame;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::BufReader;

/// Upper bound for a single message (requests and responses are tiny).
pub const MAX_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    GetStatus,
    OpenUi,
    SaveClip,
    ToggleRecording,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub version: String,
    pub uptime_secs: u64,
    pub replay_active: bool,
    pub portable: bool,
    pub working_set_kb: Option<u64>,
    pub private_kb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Status(ServiceStatus),
    Ok,
    /// A clip was written; `path` is the MP4 file.
    Saved {
        path: String,
    },
    Error {
        message: String,
    },
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(msg).map_err(|e| Error::Ipc(e.to_string()))
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| Error::Ipc(e.to_string()))
}

/// Connects to the service pipe, sends one request and waits for the reply.
///
/// Limitation (Phase 1): blocking I/O without timeout; the server answers
/// immediately, so this is acceptable until the overlapped-I/O rewrite.
pub fn send_request(pipe_path: &str, request: &Request) -> Result<Response> {
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path)?;
    let mut writer = &pipe;
    frame::write_frame(&mut writer, &encode(request)?)?;
    let mut reader = BufReader::new(&pipe);
    let bytes = frame::read_frame(&mut reader, MAX_FRAME)?
        .ok_or_else(|| Error::Ipc("service closed the connection".into()))?;
    decode(&bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_stable() {
        assert_eq!(
            encode(&Request::GetStatus).unwrap(),
            br#"{"type":"get_status"}"#
        );
        let r = Response::Error {
            message: "x".into(),
        };
        assert_eq!(decode::<Response>(&encode(&r).unwrap()).unwrap(), r);
    }

    #[test]
    fn status_round_trips() {
        let s = Response::Status(ServiceStatus {
            version: "0.1.0".into(),
            uptime_secs: 5,
            replay_active: false,
            portable: true,
            working_set_kb: Some(1234),
            private_kb: None,
        });
        assert_eq!(decode::<Response>(&encode(&s).unwrap()).unwrap(), s);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode::<Request>(b"not json").is_err());
        assert!(decode::<Request>(br#"{"type":"format_disk"}"#).is_err());
    }
}
