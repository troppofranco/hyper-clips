//! Named-pipe server for the service <-> UI/CLI protocol (blocking I/O, one thread per client).
//!
//! Known Phase 1 limitation: the pipe uses the default security descriptor.
//! Remote clients are rejected, but an explicit DACL (current user only) is still to do.

use crate::wide;
use hc_core::error::{Error, Result};
use hc_core::ipc::{decode, encode, frame, Request, Response, MAX_FRAME};
use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// Serves requests until an unrecoverable error. Run it on its own thread.
pub fn serve(path: &str, handler: Handler) -> Result<()> {
    let wpath = wide::str(path);
    let mut first = true;
    loop {
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            // Fail if another process already owns this pipe name (squatting protection).
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `wpath` is NUL-terminated and outlives the call; no security attributes.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wpath.as_ptr()),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                8,
                4096,
                4096,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(Error::System(format!(
                "cannot create pipe {path}: {}",
                std::io::Error::last_os_error()
            )));
        }
        first = false;

        // SAFETY: `handle` is a valid pipe handle created above.
        let connected = match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => true,
            // The client connected between CreateNamedPipe and ConnectNamedPipe.
            Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
        };
        if !connected {
            // SAFETY: we still own the handle.
            let _ = unsafe { CloseHandle(handle) };
            continue;
        }

        // SAFETY: ownership of the handle moves into `OwnedHandle`, which closes it on drop.
        let file = File::from(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        let handler = Arc::clone(&handler);
        if let Err(e) = std::thread::Builder::new()
            .name("hc-pipe-client".into())
            .spawn(move || handle_client(&file, &handler))
        {
            tracing::warn!("cannot spawn pipe client thread: {e}");
        }
    }
}

fn handle_client(file: &File, handler: &Handler) {
    let mut reader = BufReader::new(file);
    let response = match frame::read_frame(&mut reader, MAX_FRAME) {
        Ok(Some(bytes)) => match decode::<Request>(&bytes) {
            Ok(request) => handler(request),
            Err(e) => Response::Error {
                message: e.to_string(),
            },
        },
        _ => return,
    };
    let Ok(bytes) = encode(&response) else {
        return;
    };
    let mut writer = file;
    if let Err(e) = frame::write_frame(&mut writer, &bytes) {
        tracing::debug!("pipe write failed: {e}");
    }
}
