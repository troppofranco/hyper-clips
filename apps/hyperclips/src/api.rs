//! Optional local HTTP API (off by default).
//!
//! Listens on 127.0.0.1 only, so other machines cannot reach it. Every
//! request needs `Authorization: Bearer <token>`; the token is random and
//! stored in `<data>\api-token.txt` (readable only by the user's profile).
//! Requests carrying an `Origin` header or a foreign `Host` are refused, so a
//! web page in a browser cannot drive the API (CSRF / DNS rebinding).
//!
//! Endpoints (JSON):
//!   GET  /v1/status              core status
//!   GET  /v1/clips?limit=N       newest library items
//!   POST /v1/clip                save the replay buffer
//!   POST /v1/recording/toggle    start/stop recording
//!   POST /v1/screenshot          take a screenshot
//!   POST /v1/marker              add a marker

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Sender;
use hc_core::ipc::{Action, Request, Response};
use tiny_http::{Header, Method, Response as HttpResponse, Server};

use crate::ipc_server::{Envelope, SendHwnd, forward};

pub const TOKEN_FILE: &str = "api-token.txt";

pub struct Api {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
}

impl Api {
    pub fn start(
        port: u16,
        data_dir: &Path,
        library_db: PathBuf,
        hwnd: isize,
        to_main: Sender<Envelope>,
    ) -> Result<Self, String> {
        let token = load_or_create_token(data_dir)?;
        let server = Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let hwnd = SendHwnd(hwnd);
        let thread = std::thread::Builder::new()
            .name("local-api".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match server.recv_timeout(Duration::from_millis(250)) {
                        Ok(Some(req)) => handle(req, port, &token, &library_db, hwnd, &to_main),
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "local API receive failed");
                            break;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        tracing::info!(port, "local API listening on 127.0.0.1");
        Ok(Self {
            stop,
            thread: Some(thread),
            port,
        })
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        tracing::info!("local API stopped");
    }
}

pub fn load_or_create_token(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join(TOKEN_FILE);
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_owned();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let mut raw = [0u8; 24];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&path, &token).map_err(|e| e.to_string())?;
    Ok(token)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn header<'a>(req: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// Accept only requests addressed to the loopback name we listen on and
/// not coming from a web page.
fn allowed_origin(req: &tiny_http::Request, port: u16) -> bool {
    if header(req, "Origin").is_some() {
        return false;
    }
    match header(req, "Host") {
        Some(h) => [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
            .iter()
            .any(|ok| ok.eq_ignore_ascii_case(h)),
        None => false,
    }
}

fn authorized(req: &tiny_http::Request, token: &str) -> bool {
    header(req, "Authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t.trim().as_bytes(), token.as_bytes()))
}

fn json(status: u16, body: String) -> HttpResponse<std::io::Cursor<Vec<u8>>> {
    let mut r = HttpResponse::from_data(body.into_bytes()).with_status_code(status);
    for (k, v) in [
        ("Content-Type", "application/json; charset=utf-8"),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
    ] {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            r.add_header(h);
        }
    }
    r
}

fn err(status: u16, msg: &str) -> HttpResponse<std::io::Cursor<Vec<u8>>> {
    json(status, serde_json::json!({ "error": msg }).to_string())
}

fn handle(
    req: tiny_http::Request,
    port: u16,
    token: &str,
    library_db: &Path,
    hwnd: SendHwnd,
    to_main: &Sender<Envelope>,
) {
    let resp = if !allowed_origin(&req, port) {
        err(403, "forbidden")
    } else if !authorized(&req, token) {
        err(401, "missing or wrong bearer token")
    } else {
        route(&req, library_db, hwnd, to_main)
    };
    let _ = req.respond(resp);
}

fn route(
    req: &tiny_http::Request,
    library_db: &Path,
    hwnd: SendHwnd,
    to_main: &Sender<Envelope>,
) -> HttpResponse<std::io::Cursor<Vec<u8>>> {
    let (path, query) = req.url().split_once('?').unwrap_or((req.url(), ""));
    let action = match (req.method(), path) {
        (Method::Get, "/v1/status") => {
            return match forward(Request::Status, hwnd, to_main) {
                Response::Status(s) => json(200, serde_json::to_string(&s).unwrap_or_default()),
                other => err(500, &format!("{other:?}")),
            };
        }
        (Method::Get, "/v1/clips") => return clips(library_db, query),
        (Method::Post, "/v1/clip") => Action::SaveClip,
        (Method::Post, "/v1/recording/toggle") => Action::ToggleRecording,
        (Method::Post, "/v1/screenshot") => Action::Screenshot,
        (Method::Post, "/v1/marker") => Action::AddMarker,
        (
            _,
            "/v1/status"
            | "/v1/clips"
            | "/v1/clip"
            | "/v1/recording/toggle"
            | "/v1/screenshot"
            | "/v1/marker",
        ) => {
            return err(405, "method not allowed");
        }
        _ => return err(404, "not found"),
    };
    match forward(Request::Run(action), hwnd, to_main) {
        Response::Ok => json(200, r#"{"ok":true}"#.into()),
        Response::Unavailable(m) => err(409, &m),
        Response::Error(m) => err(500, &m),
        other => err(500, &format!("{other:?}")),
    }
}

fn clips(db: &Path, query: &str) -> HttpResponse<std::io::Cursor<Vec<u8>>> {
    let limit = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("limit="))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let lib = match hc_library::Library::open(db) {
        Ok(l) => l,
        Err(e) => return err(500, &e.to_string()),
    };
    match lib.list(
        &hc_library::Filter::default(),
        hc_library::Sort::Newest,
        limit,
        0,
    ) {
        Ok(items) => {
            let v: Vec<serde_json::Value> = items
                .iter()
                .map(|it| {
                    serde_json::json!({
                        "id": it.id,
                        "kind": it.kind.as_str(),
                        "title": it.title,
                        "game": it.game,
                        "path": it.path,
                        "created_at": it.created_at,
                        "duration_ms": it.duration_ms,
                        "size_bytes": it.size_bytes,
                        "favorite": it.favorite,
                        "tags": it.tags,
                    })
                })
                .collect();
            json(200, serde_json::Value::Array(v).to_string())
        }
        Err(e) => err(500, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[test]
    fn token_is_persisted() {
        let dir = std::env::temp_dir().join(format!("hc-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = load_or_create_token(&dir).unwrap();
        assert_eq!(a.len(), 48);
        assert_eq!(load_or_create_token(&dir).unwrap(), a);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
