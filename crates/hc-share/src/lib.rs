//! Temporary LAN sharing of a single clip, with a QR code.
//!
//! This crate lets Hyper Clips hand one clip to a phone (or any other device)
//! on the same local network without uploading it anywhere. It is meant to be
//! wired to an explicit user action; nothing here runs unless the application
//! calls [`Share::start`].
//!
//! # What it does
//!
//! * **Off by default.** A share exists only between [`Share::start`] and
//!   [`Share::stop`] (or drop, or expiry).
//! * **LAN only, by nature.** The server binds `0.0.0.0:<port>` and the URL
//!   advertised is the machine's local IPv4 address, so it is reachable from
//!   the local network but not from the internet unless the user has
//!   deliberately forwarded the port. On Windows, the first bind may trigger a
//!   Windows Defender Firewall prompt asking whether to allow the app on
//!   private/public networks; declining it blocks other devices (but not
//!   `127.0.0.1`).
//! * **Secret token in the URL.** Every URL has the form
//!   `http://<lan-ip>:<port>/<token>/...` where `<token>` is 128 bits from the
//!   OS random number generator. Any request without the exact token gets an
//!   empty `404`. The token is compared in constant time and is never logged.
//! * **Auto-expiry.** The server shuts itself down after
//!   [`ShareOptions::expires`] (15 minutes by default) or after
//!   [`ShareOptions::max_downloads`] complete downloads, whichever comes first.
//!   In-flight transfers are aborted on shutdown.
//! * **Exactly one file.** Only the chosen clip can be served; there is no
//!   directory listing and no path is ever taken from the request.
//!
//! # What it does not do
//!
//! **Traffic is not encrypted.** This is plain HTTP on the local network:
//! anyone able to observe that network (for example on shared or public
//! Wi-Fi) can see the URL, including the token, and the clip itself. The
//! token protects against devices guessing their way in, not against
//! eavesdroppers. Only use it on networks you trust.
//!
//! # Example
//!
//! ```no_run
//! use hc_share::{Share, ShareOptions, qr_matrix};
//!
//! # fn main() -> Result<(), hc_share::Error> {
//! let share = Share::start(ShareOptions::new("clip.mp4", "Ace clutch"))?;
//! let qr = qr_matrix(&share.url())?;
//! // render `qr` in the UI; the share stops on its own or when dropped.
//! # drop(qr); Ok(()) }
//! ```

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Default lifetime of a share.
pub const DEFAULT_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// How often the server thread wakes up to check for expiry / stop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum number of requests served concurrently.
const MAX_WORKERS: usize = 16;

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The path to share does not exist or is not a regular file.
    #[error("not a regular file: {0}")]
    NotAFile(PathBuf),
    /// Generic I/O failure.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    /// The HTTP server could not bind its socket.
    #[error("could not start the share server: {0}")]
    Bind(String),
    /// The operating system random number generator failed.
    #[error("random number generator failure: {0}")]
    Random(String),
    /// The text could not be encoded as a QR code (usually: too long).
    #[error("QR encoding failed: {0}")]
    Qr(String),
}

/// Convenience result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Parameters of a share.
#[derive(Debug, Clone)]
pub struct ShareOptions {
    /// The clip file to serve.
    pub file: PathBuf,
    /// Human-readable title, shown on the page and used for the download name.
    pub display_name: String,
    /// How long the share stays up.
    pub expires: Duration,
    /// Stop after this many complete (non-range) downloads; `None` = unlimited.
    pub max_downloads: Option<u32>,
    /// TCP port to bind; `None` lets the OS choose a free one.
    pub port: Option<u16>,
}

impl ShareOptions {
    /// Options with the default expiry (15 minutes), unlimited downloads and
    /// an OS-chosen port.
    pub fn new(file: impl Into<PathBuf>, display_name: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            display_name: display_name.into(),
            expires: DEFAULT_EXPIRY,
            max_downloads: None,
            port: None,
        }
    }
}

/// Shared state between the [`Share`] handle, the server thread and workers.
struct Inner {
    token: String,
    file: PathBuf,
    display_name: String,
    kind: MediaKind,
    download_name: String,
    expires_at: Instant,
    max_downloads: Option<u32>,
    stop: AtomicBool,
    running: AtomicBool,
    downloads: AtomicU32,
    requests: AtomicU64,
}

impl Inner {
    fn limit_reached(&self) -> bool {
        self.max_downloads
            .is_some_and(|max| self.downloads.load(Ordering::SeqCst) >= max)
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
            || Instant::now() >= self.expires_at
            || self.limit_reached()
    }
}

/// A running (or finished) LAN share. Dropping it stops the server.
pub struct Share {
    inner: Arc<Inner>,
    host: Ipv4Addr,
    port: u16,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omits the token / URL.
        f.debug_struct("Share")
            .field("port", &self.port)
            .field("running", &self.is_running())
            .field("downloads", &self.downloads())
            .finish_non_exhaustive()
    }
}

impl Share {
    /// Starts serving `opts.file` on all interfaces.
    ///
    /// On Windows the first bind may show a firewall prompt.
    pub fn start(opts: ShareOptions) -> Result<Share> {
        let meta = std::fs::metadata(&opts.file)?;
        if !meta.is_file() {
            return Err(Error::NotAFile(opts.file));
        }
        let token = new_token()?;
        let server = Server::http(("0.0.0.0", opts.port.unwrap_or(0)))
            .map_err(|e| Error::Bind(e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a: SocketAddr| a.port())
            .ok_or_else(|| Error::Bind("server has no IP address".into()))?;

        let ext = extension_of(&opts.file);
        let kind = MediaKind::from_extension(&ext);
        let download_name = download_filename(&opts.display_name, &ext);
        let inner = Arc::new(Inner {
            token,
            file: opts.file,
            display_name: opts.display_name,
            kind,
            download_name,
            expires_at: Instant::now() + opts.expires,
            max_downloads: opts.max_downloads,
            stop: AtomicBool::new(false),
            running: AtomicBool::new(true),
            downloads: AtomicU32::new(0),
            requests: AtomicU64::new(0),
        });

        let thread_inner = Arc::clone(&inner);
        let thread = std::thread::Builder::new()
            .name("hc-share".into())
            .spawn(move || serve(server, port, thread_inner))?;

        tracing::info!(
            port,
            expires_secs = opts.expires.as_secs(),
            max_downloads = ?opts.max_downloads,
            "share started"
        );

        Ok(Share {
            inner,
            host: lan_ipv4().unwrap_or(Ipv4Addr::LOCALHOST),
            port,
            thread: Some(thread),
        })
    }

    /// The page URL to give to the other device (contains the secret token).
    pub fn url(&self) -> String {
        format!("http://{}:{}/{}/", self.host, self.port, self.inner.token)
    }

    /// The TCP port the server is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// When the share expires on its own.
    pub fn expires_at(&self) -> Instant {
        self.inner.expires_at
    }

    /// Number of complete downloads served so far.
    pub fn downloads(&self) -> u32 {
        self.inner.downloads.load(Ordering::SeqCst)
    }

    /// Whether the server is still accepting requests.
    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Stops the server, aborting in-flight transfers, and waits for it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("share server thread panicked");
            self.inner.running.store(false, Ordering::SeqCst);
        }
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Server thread main loop.
fn serve(server: Server, port: u16, inner: Arc<Inner>) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !inner.should_stop() {
        let request = match server.recv_timeout(POLL_INTERVAL) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(error = %e, "share server receive error");
                break;
            }
        };
        inner.requests.fetch_add(1, Ordering::Relaxed);
        workers.retain(|w| !w.is_finished());
        if workers.len() >= MAX_WORKERS {
            let _ = request.respond(with_security_headers(Response::empty(503)));
            continue;
        }
        let worker_inner = Arc::clone(&inner);
        match std::thread::Builder::new()
            .name("hc-share-worker".into())
            .spawn(move || handle(request, &worker_inner))
        {
            Ok(handle) => workers.push(handle),
            Err(e) => tracing::warn!(error = %e, "could not spawn share worker"),
        }
    }
    // Abort in-flight transfers and stop accepting.
    inner.stop.store(true, Ordering::SeqCst);
    server.unblock();
    for w in workers {
        let _ = w.join();
    }
    drop(server);
    // tiny_http wakes its accept thread by connecting to the bound address,
    // which fails on Windows for `0.0.0.0`; poke it via loopback instead so
    // the listening socket is actually closed.
    let wake = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    if let Ok(s) = TcpStream::connect_timeout(&wake, Duration::from_millis(500)) {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
    inner.running.store(false, Ordering::SeqCst);
    tracing::info!(
        requests = inner.requests.load(Ordering::Relaxed),
        downloads = inner.downloads.load(Ordering::SeqCst),
        "share stopped"
    );
}

/// Handles one request on a worker thread.
fn handle(request: Request, inner: &Inner) {
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let path = path.strip_prefix('/').unwrap_or(path);
    let (token, rest) = path.split_once('/').unwrap_or((path, ""));

    if !constant_time_eq(token.as_bytes(), inner.token.as_bytes()) {
        let _ = request.respond(with_security_headers(Response::empty(404)));
        return;
    }
    if !matches!(request.method(), Method::Get | Method::Head) {
        let mut resp = with_security_headers(Response::empty(405));
        push_header(&mut resp, "Allow", "GET, HEAD");
        let _ = request.respond(resp);
        return;
    }
    if inner.should_stop() {
        let _ = request.respond(with_security_headers(Response::empty(410)));
        return;
    }

    match rest {
        "" => serve_page(request, inner),
        "file" => {
            let download = query.split('&').any(|p| p == "download=1");
            if let Err(e) = serve_file(request, inner, download) {
                tracing::debug!(error = %e, "share transfer ended early");
            }
        }
        _ => {
            let _ = request.respond(with_security_headers(Response::empty(404)));
        }
    }
}

fn serve_page(request: Request, inner: &Inner) {
    let html = render_page(inner);
    let mut resp = with_security_headers(Response::from_data(html.into_bytes()));
    push_header(&mut resp, "Content-Type", "text/html; charset=utf-8");
    push_header(
        &mut resp,
        "Content-Security-Policy",
        "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; media-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    );
    let _ = request.respond(resp);
}

fn render_page(inner: &Inner) -> String {
    let title = html_escape(&inner.display_name);
    let src = format!("/{}/file", inner.token);
    let media = match inner.kind {
        MediaKind::Video(_) => {
            format!("<video src=\"{src}\" controls playsinline preload=\"metadata\"></video>")
        }
        MediaKind::Image(_) => format!("<img src=\"{src}\" alt=\"{title}\">"),
        MediaKind::Other => String::new(),
    };
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"referrer\" content=\"no-referrer\">\
<title>{title}</title><style>\
body{{margin:0;padding:16px;font-family:system-ui,sans-serif;background:#111;color:#eee;\
display:flex;flex-direction:column;align-items:center;gap:16px}}\
h1{{font-size:1.2rem;margin:0;word-break:break-word;text-align:center}}\
video,img{{max-width:100%;max-height:75vh;border-radius:8px;background:#000}}\
a{{display:inline-block;padding:12px 20px;border-radius:8px;background:#3b82f6;color:#fff;\
text-decoration:none;font-weight:600}}\
p{{font-size:.8rem;color:#999;margin:0;text-align:center}}\
</style></head><body><h1>{title}</h1>{media}\
<a href=\"{src}?download=1\" download>Download</a>\
<p>Shared from Hyper Clips over the local network. This link expires automatically.</p>\
</body></html>\n"
    )
}

fn serve_file(request: Request, inner: &Inner, download: bool) -> io::Result<()> {
    let mut file = File::open(&inner.file)?;
    let len = file.metadata()?.len();
    let range_header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_owned());

    let disposition = if download {
        format!("attachment; filename=\"{}\"", inner.download_name)
    } else {
        format!("inline; filename=\"{}\"", inner.download_name)
    };

    let (status, start, count) = match range_header.as_deref().map(|r| parse_range(r, len)) {
        None | Some(RangeSpec::Ignore) => (200u16, 0u64, len),
        Some(RangeSpec::Satisfiable(start, end)) => (206, start, end - start + 1),
        Some(RangeSpec::Unsatisfiable) => {
            let mut resp = with_security_headers(Response::empty(416));
            push_header(&mut resp, "Content-Range", &format!("bytes */{len}"));
            return request.respond(resp);
        }
    };
    file.seek(SeekFrom::Start(start))?;
    let reader = StopReader {
        inner: file.take(count),
        stop: &inner.stop,
    };
    let count_usize = usize::try_from(count).map_err(io::Error::other)?;
    let mut resp = with_security_headers(
        Response::new(
            StatusCode(status),
            Vec::new(),
            reader,
            Some(count_usize),
            None,
        )
        .with_chunked_threshold(usize::MAX),
    );
    push_header(&mut resp, "Content-Type", inner.kind.content_type());
    push_header(&mut resp, "Content-Disposition", &disposition);
    push_header(&mut resp, "Accept-Ranges", "bytes");
    if status == 206 {
        push_header(
            &mut resp,
            "Content-Range",
            &format!("bytes {start}-{}/{len}", start + count - 1),
        );
    }
    let is_get = *request.method() == Method::Get;
    request.respond(resp)?;
    if status == 200 && is_get {
        inner.downloads.fetch_add(1, Ordering::SeqCst);
    }
    Ok(())
}

/// Wraps the file reader so that stopping the share aborts in-flight transfers.
struct StopReader<'a, R> {
    inner: R,
    stop: &'a AtomicBool,
}

impl<R: Read> Read for StopReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "share stopped",
            ));
        }
        self.inner.read(buf)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RangeSpec {
    /// Malformed or multi-range: serve the whole file (RFC 9110 allows this).
    Ignore,
    /// Inclusive byte range.
    Satisfiable(u64, u64),
    Unsatisfiable,
}

/// Parses a single `bytes=` range against a file of `len` bytes.
fn parse_range(value: &str, len: u64) -> RangeSpec {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeSpec::Ignore;
    };
    if spec.contains(',') {
        return RangeSpec::Ignore;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return RangeSpec::Ignore;
    };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // Suffix range: last N bytes.
        let Ok(n) = b.parse::<u64>() else {
            return RangeSpec::Ignore;
        };
        if n == 0 || len == 0 {
            return RangeSpec::Unsatisfiable;
        }
        let n = n.min(len);
        return RangeSpec::Satisfiable(len - n, len - 1);
    }
    let Ok(start) = a.parse::<u64>() else {
        return RangeSpec::Ignore;
    };
    let end = if b.is_empty() {
        len.saturating_sub(1)
    } else {
        match b.parse::<u64>() {
            Ok(e) if e >= start => e.min(len.saturating_sub(1)),
            _ => return RangeSpec::Ignore,
        }
    };
    if start >= len {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Satisfiable(start, end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Video(&'static str),
    Image(&'static str),
    Other,
}

impl MediaKind {
    fn from_extension(ext: &str) -> Self {
        match ext {
            "mp4" | "m4v" => Self::Video("video/mp4"),
            "webm" => Self::Video("video/webm"),
            "gif" => Self::Image("image/gif"),
            "webp" => Self::Image("image/webp"),
            "png" => Self::Image("image/png"),
            "jpg" | "jpeg" => Self::Image("image/jpeg"),
            _ => Self::Other,
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Video(t) | Self::Image(t) => t,
            Self::Other => "application/octet-stream",
        }
    }
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// Builds an ASCII-only, header-safe filename from the display name.
fn download_filename(display_name: &str, ext: &str) -> String {
    let mut stem: String = display_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = stem.trim_matches(|c| c == '_' || c == '.');
    stem = trimmed.chars().take(80).collect();
    if stem.is_empty() {
        stem = "clip".into();
    }
    let ext: String = ext.chars().filter(char::is_ascii_alphanumeric).collect();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn push_header<R: Read>(resp: &mut Response<R>, name: &str, value: &str) {
    if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
        resp.add_header(h);
    }
}

fn with_security_headers<R: Read>(mut resp: Response<R>) -> Response<R> {
    push_header(&mut resp, "Cache-Control", "no-store");
    push_header(&mut resp, "X-Content-Type-Options", "nosniff");
    push_header(&mut resp, "Referrer-Policy", "no-referrer");
    resp
}

/// Generates a 128-bit random token encoded as 32 lowercase hex characters.
fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
    let mut s = String::with_capacity(32);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// Compares two byte strings without short-circuiting on the first mismatch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // The token length is public (always 32), so leaking it is harmless.
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// Primary IPv4 address of the interface holding the default route.
///
/// Uses a connected UDP socket towards `192.0.2.1` (RFC 5737 TEST-NET-1):
/// `connect` on UDP only selects a route, no packet is sent. Returns `None`
/// when there is no usable non-loopback address (e.g. offline).
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_unspecified() && !a.ip().is_loopback() => Some(*a.ip()),
        _ => None,
    }
}

/// A QR code as a square grid of modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    /// Width and height in modules (excluding any quiet zone).
    pub size: usize,
    /// Row-major modules, `true` = dark. Length is `size * size`.
    pub modules: Vec<bool>,
}

impl QrMatrix {
    /// Whether the module at column `x`, row `y` is dark. Out of range = light.
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.size || y >= self.size {
            return false;
        }
        self.modules
            .get(y * self.size + x)
            .copied()
            .unwrap_or(false)
    }
}

/// Encodes `text` as a QR code with medium (M) error correction.
///
/// Renderers should add a quiet zone of 4 light modules around the matrix.
pub fn qr_matrix(text: &str) -> Result<QrMatrix> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| Error::Qr(e.to_string()))?;
    let size = code.width();
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { size, modules })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;
    struct HttpResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl HttpResponse {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    fn get(port: u16, path: &str, extra: &str) -> io::Result<HttpResponse> {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}\r\n"
        )?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)?;
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| io::Error::other("no header end"))?;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let body = raw[split + 4..].to_vec();
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| io::Error::other("bad status line"))?;
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn temp_clip(name: &str, len: usize) -> (PathBuf, Vec<u8>) {
        let dir = std::env::temp_dir().join(format!(
            "hc-share-test-{}-{}",
            std::process::id(),
            name.replace('.', "_")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        (path, data)
    }

    fn token_of(share: &Share) -> String {
        share.inner.token.clone()
    }

    /// The listener closes shortly after the server thread ends.
    fn wait_refused(port: u16) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn wait_stopped(share: &Share, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if !share.is_running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn page_served_with_token_and_404_otherwise() {
        let (path, _) = temp_clip("page.mp4", 1000);
        let share = Share::start(ShareOptions::new(&path, "My <clip>")).unwrap();
        let port = share.port();
        let token = token_of(&share);
        assert!(share.url().ends_with(&format!(":{port}/{token}/")));

        let page = get(port, &format!("/{token}/"), "").unwrap();
        assert_eq!(page.status, 200);
        assert_eq!(page.header("Cache-Control"), Some("no-store"));
        assert_eq!(page.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(page.header("Referrer-Policy"), Some("no-referrer"));
        let html = String::from_utf8(page.body).unwrap();
        assert!(html.contains("My &lt;clip&gt;"));
        assert!(html.contains("<video"));
        assert!(html.contains("?download=1"));

        for bad in ["/", "/file", "/deadbeef/", &format!("/{token}x/file")] {
            let r = get(port, bad, "").unwrap();
            assert_eq!(r.status, 404, "{bad}");
            assert!(r.body.is_empty());
        }
        let mut wrong = token.clone().into_bytes();
        wrong[0] = if wrong[0] == b'a' { b'b' } else { b'a' };
        let wrong = String::from_utf8(wrong).unwrap();
        assert_eq!(
            get(port, &format!("/{wrong}/file"), "").unwrap().status,
            404
        );
        assert_eq!(
            get(port, &format!("/{token}/nope"), "").unwrap().status,
            404
        );
        share.stop();
    }

    #[test]
    fn full_download_and_range() {
        let (path, data) = temp_clip("full.webm", 100_000);
        let share = Share::start(ShareOptions::new(&path, "Clip")).unwrap();
        let port = share.port();
        let token = token_of(&share);

        let full = get(port, &format!("/{token}/file?download=1"), "").unwrap();
        assert_eq!(full.status, 200);
        assert_eq!(full.header("Content-Type"), Some("video/webm"));
        assert_eq!(
            full.header("Content-Disposition"),
            Some("attachment; filename=\"Clip.webm\"")
        );
        assert_eq!(full.body, data);
        assert_eq!(share.downloads(), 1);

        let part = get(
            port,
            &format!("/{token}/file"),
            "Range: bytes=1000-1999\r\n",
        )
        .unwrap();
        assert_eq!(part.status, 206);
        assert_eq!(part.header("Content-Range"), Some("bytes 1000-1999/100000"));
        assert!(
            part.header("Content-Disposition")
                .unwrap()
                .starts_with("inline")
        );
        assert_eq!(part.body, &data[1000..2000]);

        let tail = get(port, &format!("/{token}/file"), "Range: bytes=-10\r\n").unwrap();
        assert_eq!(tail.status, 206);
        assert_eq!(tail.body, &data[data.len() - 10..]);

        let bad = get(port, &format!("/{token}/file"), "Range: bytes=200000-\r\n").unwrap();
        assert_eq!(bad.status, 416);

        // Range requests do not count as downloads.
        assert_eq!(share.downloads(), 1);
        share.stop();
    }

    #[test]
    fn max_downloads_stops_server() {
        let (path, data) = temp_clip("once.gif", 5000);
        let mut opts = ShareOptions::new(&path, "Once");
        opts.max_downloads = Some(1);
        let share = Share::start(opts).unwrap();
        let port = share.port();
        let token = token_of(&share);

        let page = get(port, &format!("/{token}/"), "").unwrap();
        assert!(String::from_utf8(page.body).unwrap().contains("<img"));

        let full = get(port, &format!("/{token}/file"), "").unwrap();
        assert_eq!(full.header("Content-Type"), Some("image/gif"));
        assert_eq!(full.body, data);
        assert!(wait_stopped(&share, Duration::from_secs(5)));
        assert_eq!(share.downloads(), 1);
        assert!(wait_refused(port));
    }

    #[test]
    fn expiry_stops_server() {
        let (path, _) = temp_clip("exp.png", 10);
        let mut opts = ShareOptions::new(&path, "Exp");
        opts.expires = Duration::from_millis(300);
        let share = Share::start(opts).unwrap();
        assert!(share.is_running());
        assert!(wait_stopped(&share, Duration::from_secs(5)));
        assert!(Instant::now() >= share.expires_at());
        assert!(wait_refused(share.port()));
    }

    #[test]
    fn drop_stops_server() {
        let (path, _) = temp_clip("drop.mp4", 10);
        let share = Share::start(ShareOptions::new(&path, "Drop")).unwrap();
        let port = share.port();
        drop(share);
        assert!(wait_refused(port));
    }

    #[test]
    fn rejects_missing_file() {
        let err = Share::start(ShareOptions::new("definitely/not/here.mp4", "x"));
        assert!(err.is_err());
    }

    #[test]
    fn qr_matrix_is_square() {
        let qr = qr_matrix("http://192.168.1.20:51234/0123456789abcdef0123456789abcdef/").unwrap();
        assert!(qr.size >= 21);
        assert_eq!(qr.modules.len(), qr.size * qr.size);
        assert!(qr.modules.iter().any(|&m| m));
        // Finder pattern corner is dark.
        assert!(qr.get(0, 0));
        assert!(!qr.get(qr.size, 0));
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_range("bytes=0-", 10), RangeSpec::Satisfiable(0, 9));
        assert_eq!(parse_range("bytes=5-100", 10), RangeSpec::Satisfiable(5, 9));
        assert_eq!(parse_range("bytes=0-1,4-5", 10), RangeSpec::Ignore);
        assert_eq!(parse_range("bytes=10-", 10), RangeSpec::Unsatisfiable);
        assert_eq!(
            download_filename("Ace \"clutch\"/1", "mp4"),
            "Ace__clutch__1.mp4"
        );
        assert_eq!(download_filename("æ—¥æœ¬", "webm"), "clip.webm");
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        let t = new_token().unwrap();
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        let _ = lan_ipv4();
    }
}
