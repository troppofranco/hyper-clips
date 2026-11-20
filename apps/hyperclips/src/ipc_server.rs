//! Pipe server: one thread accepts, one short-lived thread per client.
//! Requests that touch UI state are marshalled to the main (window) thread.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use hc_core::ipc::{
    IpcError, PipeServer, PipeStream, Request, Response, pipe_name, read_frame, write_frame,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::tray::WM_APP_IPC;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// A request waiting to be executed on the main thread.
pub struct Envelope {
    pub request: Request,
    pub reply: Sender<Response>,
}

pub struct IpcServer {
    server: Arc<PipeServer>,
    thread: Option<JoinHandle<()>>,
}

/// Window handle that may cross threads (only used with PostMessageW).
#[derive(Clone, Copy)]
pub struct SendHwnd(pub isize);

impl SendHwnd {
    fn post(self, msg: u32) -> bool {
        // SAFETY: PostMessageW is thread-safe and tolerates stale handles.
        unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), msg, WPARAM(0), LPARAM(0)).is_ok() }
    }
}

impl IpcServer {
    pub fn start(hwnd: HWND, to_main: Sender<Envelope>) -> Result<Self, IpcError> {
        let server = Arc::new(PipeServer::new(&pipe_name())?);
        let srv = server.clone();
        let hwnd = SendHwnd(hwnd.0 as isize);
        let thread = std::thread::Builder::new()
            .name("ipc-accept".into())
            .spawn(move || accept_loop(&srv, hwnd, &to_main))
            .map_err(IpcError::Io)?;
        Ok(Self {
            server,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.server.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(server: &PipeServer, hwnd: SendHwnd, to_main: &Sender<Envelope>) {
    loop {
        match server.accept() {
            Ok(Some(stream)) => {
                let tx = to_main.clone();
                let spawned = std::thread::Builder::new()
                    .name("ipc-client".into())
                    .spawn(move || serve_client(stream, hwnd, &tx));
                if let Err(e) = spawned {
                    tracing::error!(error = %e, "cannot spawn IPC client thread");
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "IPC accept failed");
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    tracing::debug!("IPC accept loop stopped");
}

fn serve_client(mut stream: PipeStream, hwnd: SendHwnd, to_main: &Sender<Envelope>) {
    loop {
        let request: Request = match read_frame(&mut stream) {
            Ok(r) => r,
            Err(IpcError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return,
            Err(e) => {
                tracing::debug!(error = %e, "IPC client read ended");
                return;
            }
        };
        let response = match request {
            Request::Ping => Response::Pong {
                version: hc_core::VERSION.into(),
            },
            other => forward(other, hwnd, to_main),
        };
        if let Err(e) = write_frame(&mut stream, &response) {
            tracing::debug!(error = %e, "IPC client write failed");
            return;
        }
    }
}

pub fn forward(request: Request, hwnd: SendHwnd, to_main: &Sender<Envelope>) -> Response {
    let (reply, rx) = bounded(1);
    if to_main.send(Envelope { request, reply }).is_err() || !hwnd.post(WM_APP_IPC) {
        return Response::Error("core is shutting down".into());
    }
    rx.recv_timeout(REPLY_TIMEOUT)
        .unwrap_or_else(|_| Response::Error("core did not answer in time".into()))
}
