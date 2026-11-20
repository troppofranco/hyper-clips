//! Video thread: paced capture → convert → encode.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use hc_capture::CaptureError;
use hc_capture::convert::{Nv12Converter, output_size};
use hc_capture::desktop::{DesktopCapture, create_device, select_output};
use hc_capture::screenshot::{self, ImageFormat};
use hc_core::media::{Packet, TIMEBASE, now_ticks};
use hc_encode::video::{VideoConfig, VideoEncoder};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread, INFINITE,
    SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_ABOVE_NORMAL, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};

use crate::sink::SinkMsg;
use crate::{Event, ServiceConfig};

pub enum VideoCmd {
    Screenshot { path: PathBuf, jpeg: bool },
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
                width: pipe.converter.out_width,
                height: pipe.converter.out_height,
                monitor: pipe.capture.info.device_name.clone(),
            });
            let _ = ready.send(Ok(()));
            pipe.run(&cfg, t0, &stop, &cmds, &sink, &events);
            tracing::info!("video thread stopped");
        })
}

struct Pipeline {
    capture: DesktopCapture,
    converter: Nv12Converter,
    encoder: VideoEncoder,
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
}

impl Pipeline {
    fn new(cfg: &ServiceConfig) -> Result<Self, CaptureError> {
        let out = select_output(cfg.monitor)?;
        let (device, context) = create_device(out.adapter_index)?;
        let capture = DesktopCapture::new(out.clone(), device.clone(), context.clone())?;
        let fps = cfg.capture.fps;
        let (w, h) = output_size(out.width(), out.height(), cfg.capture.height);
        let converter =
            Nv12Converter::new(&device, &context, out.width(), out.height(), w, h, fps, 4)?;
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
        tracing::info!(
            monitor = %out.device_name,
            adapter = %out.adapter_name,
            src = format!("{}x{}", out.width(), out.height()),
            out = format!("{w}x{h}@{fps}"),
            "video pipeline ready"
        );
        Ok(Self {
            capture,
            converter,
            encoder,
            device,
            context,
        })
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
        let mut lost_since: Option<i64> = None;
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
            let timeout = if self.capture.frame().is_none() {
                50
            } else {
                0
            };
            match self.capture.acquire(timeout) {
                Ok(_) => lost_since = None,
                Err(CaptureError::AccessLost) => {
                    // UAC prompt, mode switch, exclusive fullscreen: keep the
                    // last image flowing and retry quietly.
                    if lost_since.is_none() {
                        tracing::info!("desktop duplication paused (access lost)");
                    }
                    lost_since.get_or_insert(due);
                }
                Err(e) => tracing::warn!(error = %e, "capture error"),
            }

            if let Some(src) = self.capture.frame() {
                let packets = self
                    .converter
                    .convert(src)
                    .map_err(|e| e.to_string())
                    .and_then(|tex| {
                        self.encoder
                            .encode(&tex, due, frame_dur)
                            .map_err(|e| e.to_string())
                    });
                match packets {
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
            }
            frame += 1;
        }
    }

    fn handle(&mut self, cmd: VideoCmd, events: &Sender<Event>) {
        match cmd {
            VideoCmd::ForceKeyframe => self.encoder.force_keyframe(),
            VideoCmd::Screenshot { path, jpeg } => {
                let Some(frame) = self.capture.frame() else {
                    let _ = events.send(Event::Error("no frame captured yet".into()));
                    return;
                };
                let format = if jpeg {
                    ImageFormat::Jpeg
                } else {
                    ImageFormat::Png
                };
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match screenshot::save(&self.device, &self.context, frame, &path, format) {
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
