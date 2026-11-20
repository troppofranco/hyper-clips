//! Video thread: paced capture → (compose) → convert → encode.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use hc_capture::CaptureError;
use hc_capture::compose::{Compositor, DrawMode, Overlay, RenderTarget, srv_for};
use hc_capture::convert::{ConvertParams, Nv12Converter, output_size};
use hc_capture::desktop::{DesktopCapture, MonitorSelect, Pointer, create_device, select_output};
use hc_capture::screenshot::{self, ImageFormat};
use hc_capture::webcam::Webcam;
use hc_capture::window::WindowCapture;
use hc_core::config::OverlayPosition;
use hc_core::media::{Packet, TIMEBASE, now_ticks};
use hc_encode::video::{VideoConfig, VideoEncoder};
use windows::Win32::Foundation::{CloseHandle, HANDLE, RECT};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView,
    ID3D11Texture2D,
};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread, INFINITE,
    SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_ABOVE_NORMAL, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};
use windows::core::Interface;

use crate::sink::SinkMsg;
use crate::{CaptureTarget, Event, ServiceConfig};

pub enum VideoCmd {
    Screenshot { path: PathBuf, format: ImageFormat },
    ForceKeyframe,
}

/// High-resolution waitable timer for frame pacing (falls back to sleep).
struct Pacer(Option<HANDLE>);

impl Pacer {
    fn new() -> Self {
        // SAFETY: handle closed in Drop.
        let h = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        };
        Self(h.ok())
    }

    fn wait_until(&self, due: i64) {
        let delta = due - now_ticks();
        if delta <= 0 {
            return;
        }
        match self.0 {
            Some(h) => {
                let rel = -delta; // negative = relative, 100 ns units
                // SAFETY: valid timer handle.
                unsafe {
                    if SetWaitableTimer(h, &rel, 0, None, None, false).is_ok() {
                        WaitForSingleObject(h, INFINITE);
                    }
                }
            }
            None => std::thread::sleep(Duration::from_nanos((delta * 100) as u64)),
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

pub fn spawn(
    cfg: ServiceConfig,
    t0: i64,
    stop: Arc<AtomicBool>,
    cmds: Receiver<VideoCmd>,
    sink: Sender<SinkMsg>,
    events: Sender<Event>,
    ready: Sender<Result<(), String>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("video".into())
        .spawn(move || {
            hc_encode::mf::init_thread();
            // SAFETY: adjusting our own thread's priority.
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            }
            let mut pipe = match Pipeline::new(&cfg) {
                Ok(p) => p,
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = events.send(Event::Started {
                encoder: pipe.encoder.desc.name.clone(),
                hardware: pipe.encoder.desc.hardware,
                width: pipe.out.0,
                height: pipe.out.1,
                monitor: pipe.source.label(),
            });
            let _ = ready.send(Ok(()));
            pipe.run(&cfg, t0, &stop, &cmds, &sink, &events);
            tracing::info!("video thread stopped");
        })
}

enum Source {
    Desktop(DesktopCapture),
    Window(WindowCapture),
}

impl Source {
    fn acquire(&mut self, timeout: u32) -> Result<bool, CaptureError> {
        match self {
            Source::Desktop(d) => d.acquire(timeout),
            Source::Window(w) => w.acquire(timeout),
        }
    }
    fn frame(&self) -> Option<&ID3D11Texture2D> {
        match self {
            Source::Desktop(d) => d.frame(),
            Source::Window(w) => w.frame(),
        }
    }
    fn hdr(&self) -> bool {
        matches!(self, Source::Desktop(d) if d.is_hdr())
    }
    fn pointer(&self) -> Option<&Pointer> {
        match self {
            Source::Desktop(d) => Some(d.pointer()),
            Source::Window(_) => None, // WGC composes the cursor itself
        }
    }
    fn label(&self) -> String {
        match self {
            Source::Desktop(d) => d.info.device_name.clone(),
            Source::Window(_) => "window".into(),
        }
    }
}

fn tex_size(t: &ID3D11Texture2D) -> (u32, u32) {
    let mut d = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: plain descriptor query.
    unsafe { t.GetDesc(&mut d) };
    (d.Width, d.Height)
}

struct Pipeline {
    source: Source,
    crop: Option<RECT>,
    out: (u32, u32),
    fps: u32,
    converter: Option<((u32, u32), Nv12Converter)>,
    encoder: VideoEncoder,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    compositor: Option<Compositor>,
    compositor_failed: bool,
    composed: Option<RenderTarget>,
    frame_srv: Option<(usize, ID3D11ShaderResourceView)>,
    cursor: Option<(u64, Overlay)>,
    webcam: Option<Webcam>,
    cam_overlay: Option<(u64, Overlay)>,
    /// Last image fed to the encoder (BGRA), used for screenshots.
    last: Option<ID3D11Texture2D>,
}

impl Pipeline {
    fn new(cfg: &ServiceConfig) -> Result<Self, CaptureError> {
        let (select, region, window) = match cfg.target {
            CaptureTarget::Monitor { select, region } => (select, region, None),
            CaptureTarget::Window { hwnd, center } => (
                MonitorSelect::Containing(center.0, center.1),
                None,
                Some(hwnd),
            ),
        };
        let out = select_output(select)?;
        let (device, context) = create_device(out.adapter_index)?;
        let (source, base, crop) = match window {
            Some(hwnd) => {
                let w = WindowCapture::new(
                    hwnd,
                    device.clone(),
                    context.clone(),
                    cfg.capture.show_cursor,
                )?;
                let size = w.initial_size();
                (Source::Window(w), size, None)
            }
            None => {
                let d = DesktopCapture::new(out.clone(), device.clone(), context.clone())?;
                let crop = region.and_then(|(x, y, w, h)| {
                    // Region in virtual-desktop coordinates → output-relative.
                    let left = (x - out.rect.left).clamp(0, out.width() as i32);
                    let top = (y - out.rect.top).clamp(0, out.height() as i32);
                    let right = (left + w as i32).min(out.width() as i32);
                    let bottom = (top + h as i32).min(out.height() as i32);
                    (right - left >= 16 && bottom - top >= 16).then_some(RECT {
                        left,
                        top,
                        right,
                        bottom,
                    })
                });
                let base = crop.map_or((out.width(), out.height()), |r| {
                    ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
                });
                (Source::Desktop(d), base, crop)
            }
        };
        let fps = cfg.capture.fps;
        let (w, h) = output_size(base.0, base.1, cfg.capture.height);
        let vcfg = VideoConfig {
            codec: cfg.codec(),
            width: w,
            height: h,
            fps,
            rate: cfg.rate(),
            gop: fps,
        };
        let encoder = VideoEncoder::new(&device, out.adapter_luid, &vcfg)
            .map_err(|e| CaptureError::Other(e.to_string()))?;
        let webcam = cfg
            .webcam
            .enabled
            .then(|| Webcam::start(cfg.webcam.device.clone()));
        tracing::info!(
            source = %source.label(),
            adapter = %out.adapter_name,
            hdr = source.hdr(),
            src = format!("{}x{}", base.0, base.1),
            out = format!("{w}x{h}@{fps}"),
            "video pipeline ready"
        );
        Ok(Self {
            source,
            crop,
            out: (w, h),
            fps,
            converter: None,
            encoder,
            device,
            context,
            compositor: None,
            compositor_failed: false,
            composed: None,
            frame_srv: None,
            cursor: None,
            webcam,
            cam_overlay: None,
            last: None,
        })
    }

    fn compositor(&mut self) -> Option<&Compositor> {
        if self.compositor.is_none() && !self.compositor_failed {
            match Compositor::new(&self.device, &self.context) {
                Ok(c) => self.compositor = Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "compositor unavailable: no cursor/HDR/webcam overlay");
                    self.compositor_failed = true;
                }
            }
        }
        self.compositor.as_ref()
    }

    /// Build the BGRA image to encode: raw frame, or a composed copy with
    /// HDR tone-mapping, cursor and webcam.
    fn prepare(&mut self, cfg: &ServiceConfig) -> Result<Option<ID3D11Texture2D>, CaptureError> {
        let Some(frame) = self.source.frame().cloned() else {
            return Ok(None);
        };
        let hdr = self.source.hdr();
        let pointer = self
            .source
            .pointer()
            .filter(|p| cfg.capture.show_cursor && p.visible && p.image.is_some())
            .cloned();
        let cam = self.webcam.as_ref().and_then(Webcam::latest);
        if !hdr && pointer.is_none() && cam.is_none() {
            return Ok(Some(frame));
        }
        if self.compositor().is_none() {
            // Without shaders an HDR frame can't be shown correctly; SDR passes through.
            return Ok(if hdr { None } else { Some(frame) });
        }
        let (fw, fh) = tex_size(&frame);
        if self
            .composed
            .as_ref()
            .is_none_or(|c| (c.width, c.height) != (fw, fh))
        {
            self.composed = Some(RenderTarget::new(&self.device, fw, fh)?);
        }
        let (Some(comp), Some(target)) = (self.compositor.as_ref(), self.composed.as_ref()) else {
            return Ok(Some(frame));
        };
        let full = [0.0, 0.0, fw as f32, fh as f32];
        if hdr {
            let key = frame.as_raw() as usize;
            if self.frame_srv.as_ref().is_none_or(|(k, _)| *k != key) {
                self.frame_srv = Some((key, srv_for(&self.device, &frame)?));
            }
            if let Some((_, srv)) = &self.frame_srv {
                let scale = 80.0 / cfg.capture.sdr_white_nits.max(80) as f32;
                comp.draw(target, srv, full, DrawMode::ToneMap { scale });
            }
        } else {
            // SAFETY: same size and format (BGRA8) on the same device.
            unsafe { self.context.CopyResource(&target.texture, &frame) };
        }
        if let Some(p) = pointer
            && let Some(img) = &p.image
        {
            if self.cursor.as_ref().is_none_or(|(v, _)| *v != p.version) {
                let ov = Overlay::new(&self.device, img.width, img.height)?;
                ov.upload(&self.context, &img.bgra);
                self.cursor = Some((p.version, ov));
            }
            if let Some((_, ov)) = &self.cursor {
                let (x, y) = (p.x as f32, p.y as f32);
                comp.draw(
                    target,
                    &ov.srv,
                    [x, y, x + ov.width as f32, y + ov.height as f32],
                    DrawMode::Blend { opacity: 1.0 },
                );
            }
        }
        if let Some(cam) = cam {
            if self.cam_overlay.as_ref().is_none_or(|(v, o)| {
                *v != cam.version || (o.width, o.height) != (cam.width, cam.height)
            }) {
                let ov = match self.cam_overlay.take() {
                    Some((_, o)) if (o.width, o.height) == (cam.width, cam.height) => o,
                    _ => Overlay::new(&self.device, cam.width, cam.height)?,
                };
                ov.upload(&self.context, &cam.bgra);
                self.cam_overlay = Some((cam.version, ov));
            }
            if let Some((_, ov)) = &self.cam_overlay {
                let area = self.crop.map_or([0.0, 0.0, fw as f32, fh as f32], |r| {
                    [r.left as f32, r.top as f32, r.right as f32, r.bottom as f32]
                });
                let rect = webcam_rect(area, (ov.width, ov.height), &cfg.webcam);
                comp.draw(target, &ov.srv, rect, DrawMode::Blend { opacity: 1.0 });
            }
        }
        Ok(Some(target.texture.clone()))
    }

    fn convert_and_encode(
        &mut self,
        image: &ID3D11Texture2D,
        due: i64,
        frame_dur: i64,
    ) -> Result<Vec<hc_encode::video::EncodedVideo>, String> {
        let size = tex_size(image);
        if self.converter.as_ref().is_none_or(|(s, _)| *s != size) {
            // First frame, window resize or rotation: rebuild for the new input.
            let params = ConvertParams {
                in_width: size.0,
                in_height: size.1,
                crop: self.crop,
                out_width: self.out.0,
                out_height: self.out.1,
                fps: self.fps,
                pool_size: 4,
            };
            let conv = Nv12Converter::new(&self.device, &self.context, &params)
                .map_err(|e| e.to_string())?;
            self.converter = Some((size, conv));
        }
        let Some((_, conv)) = self.converter.as_mut() else {
            return Ok(Vec::new());
        };
        let nv12 = conv.convert(image).map_err(|e| e.to_string())?;
        self.encoder
            .encode(&nv12, due, frame_dur)
            .map_err(|e| e.to_string())
    }

    fn run(
        &mut self,
        cfg: &ServiceConfig,
        t0: i64,
        stop: &AtomicBool,
        cmds: &Receiver<VideoCmd>,
        sink: &Sender<SinkMsg>,
        events: &Sender<Event>,
    ) {
        let fps = i64::from(cfg.capture.fps.max(1));
        let frame_dur = TIMEBASE / fps;
        let pacer = Pacer::new();
        let mut frame: i64 = 0;
        let mut info_sent = false;
        let mut lost = false;
        while !stop.load(Ordering::Relaxed) {
            // Exact grid: pts = t0 + n/fps; skip ahead if we fell behind.
            let mut due = t0 + frame * TIMEBASE / fps;
            let now = now_ticks();
            if now - due > 2 * frame_dur {
                frame = (now - t0) * fps / TIMEBASE;
                due = t0 + frame * TIMEBASE / fps;
            }
            pacer.wait_until(due);

            while let Ok(cmd) = cmds.try_recv() {
                self.handle(cmd, events);
            }

            // Until the first image arrives, wait a little: a static screen
            // produces no further updates after the initial one.
            let timeout = if self.source.frame().is_none() { 50 } else { 0 };
            match self.source.acquire(timeout) {
                Ok(_) => lost = false,
                Err(CaptureError::AccessLost) => {
                    // UAC prompt, mode switch, exclusive fullscreen: keep the
                    // last image flowing and retry quietly.
                    if !lost {
                        tracing::info!("desktop duplication paused (access lost)");
                    }
                    lost = true;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "capture error");
                    let _ = events.send(Event::Error(format!("capture: {e}")));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }

            let image = match self.prepare(cfg) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!(error = %e, "compose failed");
                    self.source.frame().cloned()
                }
            };
            if let Some(image) = image {
                match self.convert_and_encode(&image, due, frame_dur) {
                    Ok(list) => {
                        for p in list {
                            if !info_sent && let Some(info) = self.encoder.video_info() {
                                let _ = sink.send(SinkMsg::VideoInfo(info));
                                info_sent = true;
                            }
                            let _ = sink.send(SinkMsg::Packet(Packet {
                                track: 0,
                                pts: p.pts,
                                dur: frame_dur,
                                key: p.key,
                                data: Arc::from(p.data.into_boxed_slice()),
                            }));
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "encode failed");
                        let _ = events.send(Event::Error(format!("encoder: {e}")));
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                self.last = Some(image);
            }
            frame += 1;
        }
    }

    fn handle(&mut self, cmd: VideoCmd, events: &Sender<Event>) {
        match cmd {
            VideoCmd::ForceKeyframe => self.encoder.force_keyframe(),
            VideoCmd::Screenshot { path, format } => {
                let Some(frame) = self.last.clone() else {
                    let _ = events.send(Event::Error("no frame captured yet".into()));
                    return;
                };
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match screenshot::save(&self.device, &self.context, &frame, &path, format) {
                    Ok(()) => {
                        let _ = events.send(Event::ScreenshotSaved { path });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("screenshot: {e}")));
                    }
                }
            }
        }
    }
}

/// Webcam rectangle inside `area` (pixels), keeping the camera aspect ratio.
fn webcam_rect(area: [f32; 4], cam: (u32, u32), cfg: &hc_core::config::Webcam) -> [f32; 4] {
    let (aw, ah) = (area[2] - area[0], area[3] - area[1]);
    let h = ah * cfg.size_percent as f32 / 100.0;
    let w = h * cam.0 as f32 / cam.1.max(1) as f32;
    let margin = ah * 0.02;
    let (x, y) = match cfg.position {
        OverlayPosition::TopLeft => (area[0] + margin, area[1] + margin),
        OverlayPosition::TopRight => (area[0] + aw - w - margin, area[1] + margin),
        OverlayPosition::BottomLeft => (area[0] + margin, area[1] + ah - h - margin),
        OverlayPosition::BottomRight => (area[0] + aw - w - margin, area[1] + ah - h - margin),
    };
    [x, y, x + w, y + h]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcam_bottom_right() {
        let cfg = hc_core::config::Webcam {
            size_percent: 25,
            position: OverlayPosition::BottomRight,
            ..Default::default()
        };
        let r = webcam_rect([0.0, 0.0, 1920.0, 1080.0], (640, 360), &cfg);
        assert!((r[3] - r[1] - 270.0).abs() < 0.01);
        assert!((r[2] - r[0] - 480.0).abs() < 0.01);
        assert!(r[2] <= 1920.0 && r[3] <= 1080.0 && r[0] > 1000.0);
    }
}
