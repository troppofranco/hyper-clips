//! Synchronous named-pipe transport (byte mode, one thread per client).

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::protocol::{IpcError, Request, Response, read_frame, write_frame};

const BUFFER_SIZE: u32 = 64 * 1024;

/// `\\.\pipe\HyperClips.<session>`: one core per interactive session.
pub fn pipe_name() -> String {
    let mut session = 0u32;
    // SAFETY: plain out-parameter call.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    format!(r"\\.\pipe\HyperClips.{session}")
}

/// A connected pipe end. Server-side streams disconnect on drop.
pub struct PipeStream {
    handle: HANDLE,
    server: bool,
}

// SAFETY: the pipe handle is owned exclusively by this value; Windows pipe
// handles may be used from any thread.
unsafe impl Send for PipeStream {}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0u32;
        // SAFETY: buffer and out-param are valid for the duration of the call.
        match unsafe { ReadFile(self.handle, Some(buf), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut written = 0u32;
        // SAFETY: buffer and out-param are valid for the duration of the call.
        unsafe { WriteFile(self.handle, Some(buf), Some(&mut written), None) }
            .map_err(std::io::Error::other)?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        unsafe {
            if self.server {
                let _ = FlushFileBuffers(self.handle);
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Accepts connections on the per-session pipe.
pub struct PipeServer {
    name: HSTRING,
    first: AtomicBool,
    stopping: AtomicBool,
    security: SecurityDescriptor,
    /// An instance always waiting for the next client, so back-to-back
    /// connections never find the name missing.
    pending: std::sync::Mutex<Option<PipeStream>>,
}

impl PipeServer {
    pub fn new(name: &str) -> Result<Self, IpcError> {
        let server = Self {
            name: HSTRING::from(name),
            first: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            security: SecurityDescriptor::current_user_only()?,
            pending: std::sync::Mutex::new(None),
        };
        // Create the first instance now: claims the name (anti-squatting)
        // and lets clients connect before the accept loop runs.
        let first = server.create_instance()?;
        if let Ok(mut p) = server.pending.lock() {
            *p = Some(first);
        }
        Ok(server)
    }

    fn create_instance(&self) -> Result<PipeStream, IpcError> {
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if self.first.swap(false, Ordering::SeqCst) {
            // Fail if another process already owns this pipe name (squatting).
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.0.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: all pointers outlive the call; the returned handle is owned by PipeStream.
        let handle = unsafe {
            CreateNamedPipeW(
                &self.name,
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                Some(&sa),
            )
        };
        if handle.is_invalid() {
            return Err(IpcError::Os(windows::core::Error::from_thread()));
        }
        Ok(PipeStream {
            handle,
            server: true,
        })
    }

    /// Block until a client connects. Returns `Ok(None)` after [`Self::stop`].
    pub fn accept(&self) -> Result<Option<PipeStream>, IpcError> {
        let pending = self.pending.lock().ok().and_then(|mut p| p.take());
        let stream = match pending {
            Some(s) => s,
            None => self.create_instance()?,
        };
        // SAFETY: valid pipe handle, synchronous mode.
        match unsafe { ConnectNamedPipe(stream.handle, None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(e) => return Err(IpcError::Os(e)),
        }
        if self.stopping.load(Ordering::SeqCst) {
            return Ok(None);
        }
        // Prepare the next listener before serving this client.
        match self.create_instance() {
            Ok(next) => {
                if let Ok(mut p) = self.pending.lock() {
                    *p = Some(next);
                }
            }
            Err(e) => tracing::warn!(error = %e, "cannot pre-create next pipe instance"),
        }
        Ok(Some(stream))
    }
    /// Unblock a pending [`Self::accept`] so the accept loop can exit.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // The accept loop may be between instances (no pipe exists for a few
        // microseconds), so retry briefly until a connection lands.
        let name = self.name.to_string_lossy();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match PipeClient::open(&name, Duration::from_millis(200)) {
                Ok(_) => return,
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        tracing::warn!("pipe server did not acknowledge stop");
    }
}

/// Client side, used by the studio and the CLI.
pub struct PipeClient {
    stream: PipeStream,
}

impl PipeClient {
    pub fn connect(timeout: Duration) -> Result<Self, IpcError> {
        Self::open(&pipe_name(), timeout)
    }

    fn open(name: &str, timeout: Duration) -> Result<Self, IpcError> {
        let wide = HSTRING::from(name);
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: standard CreateFileW call; handle ownership moves into PipeStream.
            let res = unsafe {
                CreateFileW(
                    &wide,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    // Never let the server impersonate us.
                    FILE_FLAGS_AND_ATTRIBUTES(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                    None,
                )
            };
            match res {
                Ok(handle) => {
                    return Ok(Self {
                        stream: PipeStream {
                            handle,
                            server: false,
                        },
                    });
                }
                Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                    return Err(IpcError::NotRunning);
                }
                Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(IpcError::Os(e));
                    }
                    let ms = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(1);
                    // SAFETY: valid wide string.
                    let _ = unsafe { WaitNamedPipeW(&wide, ms) };
                }
                Err(e) => return Err(IpcError::Os(e)),
            }
        }
    }

    pub fn request(&mut self, req: &Request) -> Result<Response, IpcError> {
        write_frame(&mut self.stream, req)?;
        read_frame(&mut self.stream)
    }
}

/// Convenience: connect, send one request, return the response.
pub fn one_shot(req: &Request, timeout: Duration) -> Result<Response, IpcError> {
    PipeClient::connect(timeout)?.request(req)
}

/// Owned security descriptor: full access for the current user only,
/// explicit deny for network logons.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable after creation and freed once on drop.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    fn current_user_only() -> Result<Self, IpcError> {
        let sid = current_user_sid()?;
        let sddl = HSTRING::from(format!("D:P(D;;GA;;;NU)(A;;GA;;;{sid})"));
        let mut psd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: valid SDDL string; psd is freed with LocalFree in Drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut psd,
                None,
            )?;
        }
        Ok(Self(psd))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0.0)));
        }
    }
}

fn current_user_sid() -> Result<String, IpcError> {
    // SAFETY: token handle and buffers are managed locally and released before return.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let res = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        res?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_str)?;
        let s = PCWSTR(sid_str.0).to_string();
        let _ = LocalFree(Some(HLOCAL(sid_str.0.cast())));
        s.map_err(|e| IpcError::Io(std::io::Error::other(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Action;

    #[test]
    fn sid_lookup_works() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }

    #[test]
    fn end_to_end_over_real_pipe() {
        let name = format!(r"\\.\pipe\HyperClips.test.{}", std::process::id());
        let server = std::sync::Arc::new(PipeServer::new(&name).unwrap());
        let srv = server.clone();
        let t = std::thread::spawn(move || {
            while let Ok(Some(mut s)) = srv.accept() {
                let req: Request = read_frame(&mut s).unwrap();
                let resp = match req {
                    Request::Ping => Response::Pong {
                        version: "t".into(),
                    },
                    _ => Response::Ok,
                };
                write_frame(&mut s, &resp).unwrap();
            }
        });
        // Give the server a moment to create the first instance.
        let mut client = loop {
            match PipeClient::open(&name, Duration::from_secs(2)) {
                Ok(c) => break c,
                Err(IpcError::NotRunning) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("{e}"),
            }
        };
        assert_eq!(
            client.request(&Request::Ping).unwrap(),
            Response::Pong {
                version: "t".into()
            }
        );
        drop(client);
        let mut c2 = PipeClient::open(&name, Duration::from_secs(2)).unwrap();
        assert_eq!(
            c2.request(&Request::Run(Action::SaveClip)).unwrap(),
            Response::Ok
        );
        drop(c2);
        server.stop();
        t.join().unwrap();
    }

    #[test]
    fn missing_pipe_reports_not_running() {
        let err = PipeClient::open(
            r"\\.\pipe\HyperClips.nobody.here",
            Duration::from_millis(50),
        )
        .err()
        .unwrap();
        assert!(matches!(err, IpcError::NotRunning));
    }
}
