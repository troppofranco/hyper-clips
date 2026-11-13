//! Capture -> encode -> replay buffer, and saving clips (Windows only).
//!
//! WGC delivers frames only when the screen changes, so the capture callback just stores
//! the newest frame and a worker thread encodes at a constant frame rate, repeating the
//! last frame when nothing moved.

use hc_buffer::{ReplayBuffer, TICKS_PER_SEC};
use hc_capture_win::{Frame, ScreenCapture};
use hc_core::error::{Error, Result};
use hc_encode_win::encoder::H264Encoder;
use hc_encode_win::{mux, VideoInfo};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct PipelineSettings {
    pub fps: u32,
    pub bitrate_bps: u32,
    pub replay_secs: u32,
}

type LatestFrame = Arc<Mutex<Option<Arc<Frame>>>>;

pub struct Pipeline {
    buffer: Arc<Mutex<ReplayBuffer>>,
    info: Arc<Mutex<Option<VideoInfo>>>,
    stop: Arc<AtomicBool>,
    capture: Mutex<Option<ScreenCapture>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Pipeline {
    pub fn start(settings: PipelineSettings) -> Result<Self> {
        let max_bytes = (u64::from(settings.bitrate_bps) / 8 * u64::from(settings.replay_secs) * 2
            + 32 * 1024 * 1024) as usize;
        let buffer = Arc::new(Mutex::new(ReplayBuffer::new(
            1,
            i64::from(settings.replay_secs) * TICKS_PER_SEC,
            max_bytes,
        )));
        let info = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let latest: LatestFrame = Arc::new(Mutex::new(None));

        let slot = Arc::clone(&latest);
        let capture = ScreenCapture::start_primary_monitor(move |frame| {
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(Arc::new(frame));
            }
        })?;
        let (w, h) = capture.size();
        tracing::info!("capturing the primary monitor at {w}x{h}");

        let worker = {
            let (buffer, info, stop) = (Arc::clone(&buffer), Arc::clone(&info), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("hc-encode".into())
                .spawn(move || encode_loop(settings, &latest, &buffer, &info, &stop))
                .map_err(|e| Error::System(format!("cannot start the encoder thread: {e}")))?
        };

        Ok(Self {
            buffer,
            info,
            stop,
            capture: Mutex::new(Some(capture)),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Writes the last `secs` seconds to `path` as MP4. Muxing runs on the calling thread.
    pub fn save_clip(&self, path: &Path, secs: u32) -> Result<()> {
        let snapshot = self
            .buffer
            .lock()
            .map_err(|_| Error::System("replay buffer lock poisoned".into()))?
            .snapshot(i64::from(secs) * TICKS_PER_SEC)
            .ok_or_else(|| Error::Invalid("the replay buffer is still empty".into()))?;
        let info = self
            .info
            .lock()
            .map_err(|_| Error::System("video info lock poisoned".into()))?
            .clone()
            .ok_or_else(|| Error::Invalid("the encoder has not started yet".into()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        mux::write_mp4(path, &info, &snapshot)?;
        tracing::info!(
            "saved {} ({:.1} s)",
            path.display(),
            snapshot.duration() as f64 / TICKS_PER_SEC as f64
        );
        Ok(())
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.capture.lock() {
            *c = None; // dropping stops the capture
        }
        if let Ok(mut w) = self.worker.lock() {
            if let Some(handle) = w.take() {
                let _ = handle.join();
            }
        }
    }
}

fn encode_loop(
    settings: PipelineSettings,
    latest: &LatestFrame,
    buffer: &Mutex<ReplayBuffer>,
    info: &Mutex<Option<VideoInfo>>,
    stop: &AtomicBool,
) {
    let fps = settings.fps.max(1);
    let frame_ticks = TICKS_PER_SEC / i64::from(fps);
    let started = Instant::now();
    let mut encoder: Option<H264Encoder> = None;
    let mut packets = Vec::new();
    let mut tick: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        let due = started + Duration::from_nanos(tick * 1_000_000_000 / u64::from(fps));
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        } else if now - due > Duration::from_millis(500) {
            // Encoding fell behind: skip ahead instead of bursting duplicates.
            tick += (now - due).as_nanos() as u64 * u64::from(fps) / 1_000_000_000;
        }
        let pts = tick as i64 * frame_ticks;
        tick += 1;

        let frame = latest.lock().ok().and_then(|g| g.clone());
        let Some(frame) = frame else { continue };

        if encoder.is_none() {
            let (w, h) = (frame.width & !1, frame.height & !1);
            match H264Encoder::new_software(w, h, fps, settings.bitrate_bps, fps) {
                Ok(e) => {
                    tracing::info!(
                        "encoder ready: {w}x{h} @ {fps} fps, {} bps",
                        settings.bitrate_bps
                    );
                    // A new stream (e.g. after a resolution change) cannot follow old packets.
                    if let Ok(mut b) = buffer.lock() {
                        b.clear();
                    }
                    encoder = Some(e);
                }
                Err(e) => {
                    tracing::error!("cannot create the encoder: {e}");
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
        }
        let Some(enc) = encoder.as_mut() else {
            continue;
        };

        packets.clear();
        match enc.encode_bgra(&frame.bgra, frame.width, frame.height, pts, &mut packets) {
            Ok(()) => {
                if let Ok(mut b) = buffer.lock() {
                    for p in packets.drain(..) {
                        b.push(0, p);
                    }
                }
                if let Ok(mut i) = info.lock() {
                    if i.as_ref() != Some(enc.info()) {
                        *i = Some(enc.info().clone());
                    }
                }
            }
            Err(e) => {
                tracing::warn!("encode failed, restarting the encoder: {e}");
                encoder = None;
            }
        }
    }
}
