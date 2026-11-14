//! Capture -> encode -> replay buffer, and saving clips (Windows only).
//!
//! WGC delivers frames only when the screen changes, so the capture callback just stores
//! the newest frame and a worker thread encodes at a constant frame rate, repeating the
//! last frame when nothing moved.

use hc_audio_win::timeline::Timeline;
use hc_audio_win::{AudioCapture, Source};
use hc_buffer::{ReplayBuffer, TICKS_PER_SEC};
use hc_capture_win::{Frame, ScreenCapture};
use hc_core::error::{Error, Result};
use hc_encode_win::aac::AacEncoder;
use hc_encode_win::encoder::H264Encoder;
use hc_encode_win::{mux, AudioInfo, VideoInfo};
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
    pub audio_system: bool,
    pub audio_mic: bool,
    pub audio_bitrate_bps: u32,
}

type LatestFrame = Arc<Mutex<Option<Arc<Frame>>>>;

pub struct Pipeline {
    buffer: Arc<Mutex<ReplayBuffer>>,
    info: Arc<Mutex<Option<VideoInfo>>>,
    stop: Arc<AtomicBool>,
    capture: Mutex<Option<ScreenCapture>>,
    /// One per audio track (track index = position + 1).
    audio_infos: Vec<AudioInfo>,
    audio: Mutex<Vec<AudioCapture>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Pipeline {
    pub fn start(settings: PipelineSettings) -> Result<Self> {
        let sources: Vec<Source> = [
            settings.audio_system.then_some(Source::System),
            settings.audio_mic.then_some(Source::Microphone),
        ]
        .into_iter()
        .flatten()
        .collect();
        let total_bps = u64::from(settings.bitrate_bps)
            + u64::from(settings.audio_bitrate_bps) * sources.len() as u64;
        let max_bytes =
            (total_bps / 8 * u64::from(settings.replay_secs) * 2 + 32 * 1024 * 1024) as usize;
        let buffer = Arc::new(Mutex::new(ReplayBuffer::new(
            1 + sources.len(),
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

        // Video and audio share this clock origin so the tracks stay in sync.
        let origin = Instant::now();

        // A missing audio device must not stop video: that track is skipped with a warning.
        let mut audio = Vec::new();
        let mut audio_infos = Vec::new();
        for source in sources {
            let track = audio.len() + 1;
            match start_audio_track(source, track, settings.audio_bitrate_bps, origin, &buffer) {
                Ok(capture) => {
                    audio.push(capture);
                    audio_infos.push(AudioInfo {
                        sample_rate: hc_audio_win::timeline::SAMPLE_RATE,
                        channels: hc_audio_win::timeline::CHANNELS,
                        bitrate_bps: settings.audio_bitrate_bps,
                    });
                }
                Err(e) => tracing::warn!("audio source {source:?} unavailable: {e}"),
            }
        }

        let worker = {
            let (buffer, info, stop) = (Arc::clone(&buffer), Arc::clone(&info), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("hc-encode".into())
                .spawn(move || encode_loop(settings, origin, &latest, &buffer, &info, &stop))
                .map_err(|e| Error::System(format!("cannot start the encoder thread: {e}")))?
        };

        Ok(Self {
            buffer,
            info,
            stop,
            capture: Mutex::new(Some(capture)),
            audio_infos,
            audio: Mutex::new(audio),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Writes the last `secs` seconds to `path` as MP4. Muxing runs on the calling thread.
    pub fn save_clip(&self, path: &Path, secs: u32) -> Result<i64> {
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
        mux::write_mp4(path, &info, &self.audio_infos, &snapshot)?;
        tracing::info!(
            "saved {} ({:.1} s)",
            path.display(),
            snapshot.duration() as f64 / TICKS_PER_SEC as f64
        );
        Ok(snapshot.duration() / (TICKS_PER_SEC / 1000))
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.capture.lock() {
            *c = None; // dropping stops the capture
        }
        if let Ok(mut a) = self.audio.lock() {
            a.clear(); // dropping stops the audio threads
        }
        if let Ok(mut w) = self.worker.lock() {
            if let Some(handle) = w.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Starts one audio source and routes its AAC packets into `track` of the replay buffer.
fn start_audio_track(
    source: Source,
    track: usize,
    bitrate_bps: u32,
    origin: Instant,
    buffer: &Arc<Mutex<ReplayBuffer>>,
) -> Result<AudioCapture> {
    const AAC_FRAME_TICKS: i64 = 1024 * TICKS_PER_SEC / 48_000;
    let buffer = Arc::clone(buffer);
    let mut encoder: Option<AacEncoder> = None;
    let mut timeline = Timeline::new();
    let mut packets = Vec::new();
    let mut emitted: i64 = 0;
    let mut failed = false;

    AudioCapture::start(source, move |pcm| {
        if failed {
            return;
        }
        if encoder.is_none() {
            match AacEncoder::new(bitrate_bps) {
                Ok(e) => encoder = Some(e),
                Err(e) => {
                    tracing::error!("cannot create the AAC encoder: {e}");
                    failed = true;
                    return;
                }
            }
        }
        let Some(enc) = encoder.as_mut() else { return };
        let wall = (origin.elapsed().as_nanos() / 100) as i64;
        packets.clear();

        // Keep the track continuous while the source is silent (loopback sends nothing).
        let silence = timeline.silence_needed(wall);
        let mut result = Ok(());
        if silence > 0 {
            let zeros = vec![0i16; silence as usize * 2];
            result = enc.encode(&zeros, timeline.now_ticks(), &mut packets);
            timeline.advance(silence);
        }
        if result.is_ok() && !pcm.is_empty() {
            result = enc.encode(pcm, timeline.now_ticks(), &mut packets);
            timeline.advance(pcm.len() as u64 / 2);
        }
        if let Err(e) = result {
            tracing::warn!("audio encode failed: {e}");
            encoder = None; // recreate on the next chunk
            return;
        }
        if packets.is_empty() {
            return;
        }
        if let Ok(mut b) = buffer.lock() {
            for mut p in packets.drain(..) {
                // Timestamps come from the frame count, not from the MFT.
                p.pts = emitted * AAC_FRAME_TICKS;
                p.duration = AAC_FRAME_TICKS;
                emitted += 1;
                b.push(track, p);
            }
        }
    })
}

fn encode_loop(
    settings: PipelineSettings,
    started: Instant,
    latest: &LatestFrame,
    buffer: &Mutex<ReplayBuffer>,
    info: &Mutex<Option<VideoInfo>>,
    stop: &AtomicBool,
) {
    let fps = settings.fps.max(1);
    let frame_ticks = TICKS_PER_SEC / i64::from(fps);
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
