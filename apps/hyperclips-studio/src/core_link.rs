//! Connection to the core process: periodic status polling on a background
//! thread, plus synchronous one-off requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use hc_core::ipc::{CoreStatus, IpcError, Request, Response, one_shot};

const POLL_EVERY: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Shared {
    status: Option<CoreStatus>,
}

pub struct CoreLink {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl CoreLink {
    pub fn start(ctx: egui::Context) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (shared.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("core-poll".into())
            .spawn(move || {
                while !st.load(Ordering::Relaxed) {
                    let status = match one_shot(&Request::Status, TIMEOUT) {
                        Ok(Response::Status(s)) => Some(s),
                        _ => None,
                    };
                    if let Ok(mut g) = s.lock() {
                        g.status = status;
                    }
                    ctx.request_repaint();
                    // Sleep in small steps so closing the window is not delayed.
                    for _ in 0..20 {
                        if st.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(POLL_EVERY / 20);
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "cannot start core poller");
        }
        Self { shared, stop }
    }

    pub fn status(&self) -> Option<CoreStatus> {
        self.shared.lock().ok().and_then(|g| g.status.clone())
    }

    pub fn request(&self, req: &Request) -> Result<Response, IpcError> {
        one_shot(req, TIMEOUT)
    }
}

impl Drop for CoreLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
