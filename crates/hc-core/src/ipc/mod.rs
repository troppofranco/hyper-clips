//! Local IPC between the core process, the studio and the CLI.
//!
//! Transport: a per-session named pipe whose DACL only admits the current
//! user and which rejects remote clients. Framing: little-endian `u32`
//! length followed by a `postcard` payload.

mod pipe;
mod protocol;

pub use pipe::{PipeClient, PipeServer, PipeStream, one_shot, pipe_name};
pub use protocol::{
    Action, CoreStatus, Frame, HotkeyConflict, IpcError, MAX_FRAME, Request, Response, read_frame,
    write_frame,
};
