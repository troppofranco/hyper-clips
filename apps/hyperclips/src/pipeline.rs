//! Capture -> encode -> replay buffer, and saving clips (Windows only).
//!
//! WGC delivers frames only when the screen changes, so the capture callback just stores
//! the newest frame and a worker thread encodes at a constant frame rate, repeating the
//! last frame when nothing moved.

use hc_audio_win::timeline::Timeline;
use hc_audio_win::{AudioCapture, Source};
use hc_buffer::Packet;
use hc_buffer::{ReplayBuffer, TICKS_PER_SEC};
use hc_capture_win::{crop_bgra, Frame, ScreenCapture, Target};
use hc_core::error::{Error, Result};
use hc_encode_win::aac::AacEncoder;
use hc_encode_win::encoder::H264Encoder;
use hc_encode_win::mux::{self, Mp4Writer};
use hc_encode_win::{annexb, AudioInfo, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderChoice {
    Auto,
    Hardware,
    Software,
}

impl EncoderChoice {
    pub fn parse(s: &str) -> Self {
        match s {
            "hardware" => Self::Hardware,
            "software" => Self::Software,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PipelineSettings {
    pub fps: u32,
    pub bitrate_bps: u32,
    pub replay_secs: u32,
    pub encoder: EncoderChoice,
    pub target: Target,
    /// Optional crop `[x, y, width, height]`.
    pub region: Option<[u32; 4]>,
    pub audio_system: bool,
    pub audio_mic: bool,
    pub audio_bitrate_bps: u32,
}

type LatestFrame = Arc<Mutex<Option<Arc<Frame>>>>;

/// Full-session recording that taps the same encoded packets as the replay buffer.
#[derive(Default)]
struct Recorder {
    /// Requested file; the writer starts at the next video keyframe.
    want: Option<PathBuf>,
    writer: Option<Mp4Writer>,
    path: Option<PathBuf>,
    origin: i64,
    last_video_pts: i64,
    video_info: Option<VideoInfo>,
    audio: Vec<(usize, AudioInfo)>,
}

impl Recorder {
    fn offer(&mut self, track: usize, p: &Packet) {
        if self.writer.is_none() {
            if !(track == 0 && p.keyframe && self.want.is_some() && self.video_info.is_some()) {
                return;
            }
            let (Some(path), Some(info)) = (self.want.take(), self.video_info.clone()) else {
                return;
            };
            let header = if info.sequence_header.is_empty() {
                annexb::sequence_header(&p.data)
            } else {
                info.sequence_header.clone()
            };
            match Mp4Writer::create(&path, &info, &header, &self.audio) {
                Ok(w) => {
                    self.writer = Some(w);
                    self.origin = p.pts;
                    self.path = Some(path);
                }
                Err(e) => {
                    tracing::error!("cannot start the recording: {e}");
                    return;
                }
            }
        }
        if track != 0 && p.pts < self.origin {
            return;
        }
        if track == 0 {
            self.last_video_pts = p.pts + p.duration;
        }
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.write(track, p, p.pts - self.origin) {
                tracing::error!("recording write failed, stopping: {e}");
                self.writer = None;
            }
        }
    }
}

/// Sends a packet to the recorder (if active) and to the replay buffer.
fn emit(buffer: &Mutex<ReplayBuffer>, rec: &Mutex<Recorder>, track: usize, packet: Packet) {
    if let Ok(mut r) = rec.lock() {
        r.offer(track, &packet);
    }
    if let Ok(mut b) = buffer.lock() {
        b.push(track, packet);
    }
}

pub struct Pipeline {
    buffer: Arc<Mutex<ReplayBuffer>>,
    info: Arc<Mutex<Option<VideoInfo>>>,
    rec: Arc<Mutex<Recorder>>,
    latest: LatestFrame,
    origin: Instant,
    /// Bookmarked moments on the shared clock (100 ns ticks since `origin`).
    bookmarks: Mutex<Vec<i64>>,
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
        let rec = Arc::new(Mutex::new(Recorder::default()));

        let slot = Arc::clone(&latest);
        let region = settings.region;
        let capture = ScreenCapture::start(settings.target, move |mut frame| {
            if let Some([x, y, w, h]) = region {
                if x < frame.width && y < frame.height {
                    let (bgra, cw, ch) =
                        crop_bgra(&frame.bgra, frame.width, frame.height, x, y, w, h);
                    frame.bgra = bgra;
                    frame.width = cw;
                    frame.height = ch;
                }
            }
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(Arc::new(frame));
            }
        })?;
        let (w, h) = capture.size();
        tracing::info!("capturing {:?} at {w}x{h}", settings.target);

        // Video and audio share this clock origin so the tracks stay in sync.
        let origin = Instant::now();

        // A missing audio device must not stop video: that track is skipped with a warning.
        let mut audio = Vec::new();
        let mut audio_infos = Vec::new();
        for source in sources {
            let track = audio.len() + 1;
            match start_audio_track(
                source,
                track,
                settings.audio_bitrate_bps,
                origin,
                &buffer,
                &rec,
            ) {
                Ok(capture) => {
                    audio.push(capture);
                    let format = AudioInfo {
                        sample_rate: hc_audio_win::timeline::SAMPLE_RATE,
                        channels: hc_audio_win::timeline::CHANNELS,
                        bitrate_bps: settings.audio_bitrate_bps,
                    };
                    audio_infos.push(format);
                    if let Ok(mut r) = rec.lock() {
                        r.audio.push((track, format));
                    }
                }
                Err(e) => tracing::warn!("audio source {source:?} unavailable: {e}"),
            }
        }

        let worker = {
            let (buffer, info, stop) = (Arc::clone(&buffer), Arc::clone(&info), Arc::clone(&stop));
            let (rec, latest) = (Arc::clone(&rec), Arc::clone(&latest));
            std::thread::Builder::new()
                .name("hc-encode".into())
                .spawn(move || encode_loop(settings, origin, &latest, &buffer, &info, &rec, &stop))
                .map_err(|e| Error::System(format!("cannot start the encoder thread: {e}")))?
        };

        Ok(Self {
            buffer,
            info,
            rec,
            latest,
            origin,
            bookmarks: Mutex::new(Vec::new()),
            stop,
            capture: Mutex::new(Some(capture)),
            audio_infos,
            audio: Mutex::new(audio),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Writes the last `secs` seconds to `path` as MP4. Muxing runs on the calling thread.
    pub fn save_clip(&self, path: &Path, secs: u32) -> Result<(i64, Vec<i64>)> {
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
        let end = snapshot.start_pts + snapshot.duration();
        let marks = self.bookmarks_between(snapshot.start_pts, end);
        Ok((snapshot.duration() / (TICKS_PER_SEC / 1000), marks))
    }

    /// Marks "now" so the moment can be found later; returns false if the list is full.
    pub fn add_bookmark(&self) -> bool {
        let now = (self.origin.elapsed().as_nanos() / 100) as i64;
        match self.bookmarks.lock() {
            Ok(mut b) if b.len() < 1000 => {
                b.push(now);
                true
            }
            _ => false,
        }
    }

    /// Bookmarks inside `[from, to]` (shared-clock ticks) as milliseconds from `from`.
    fn bookmarks_between(&self, from: i64, to: i64) -> Vec<i64> {
        self.bookmarks.lock().map_or_else(
            |_| Vec::new(),
            |b| {
                b.iter()
                    .filter(|t| **t >= from && **t <= to)
                    .map(|t| (*t - from) / (TICKS_PER_SEC / 1000))
                    .collect()
            },
        )
    }

    pub fn is_recording(&self) -> bool {
        self.rec
            .lock()
            .is_ok_and(|r| r.want.is_some() || r.writer.is_some())
    }

    /// Starts recording everything from the next video keyframe into `path`.
    pub fn start_recording(&self, path: PathBuf) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut r = self
            .rec
            .lock()
            .map_err(|_| Error::System("recorder lock poisoned".into()))?;
        if r.want.is_some() || r.writer.is_some() {
            return Err(Error::Invalid("already recording".into()));
        }
        r.want = Some(path);
        Ok(())
    }

    /// Stops the recording; returns the file and its length in ms (None if it never started).
    pub fn stop_recording(&self) -> Result<Option<(PathBuf, i64, Vec<i64>)>> {
        let mut r = self
            .rec
            .lock()
            .map_err(|_| Error::System("recorder lock poisoned".into()))?;
        r.want = None;
        let (Some(writer), Some(path)) = (r.writer.take(), r.path.take()) else {
            return Ok(None);
        };
        let ms = (r.last_video_pts - r.origin) / (TICKS_PER_SEC / 1000);
        let marks = self.bookmarks_between(r.origin, r.last_video_pts);
        writer.finish()?;
        Ok(Some((path, ms.max(0), marks)))
    }

    /// Writes the newest captured frame to `path` as PNG.
    pub fn screenshot(&self, path: &Path) -> Result<()> {
        let frame = self
            .latest
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| Error::Invalid("no frame captured yet".into()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut rgba = Vec::with_capacity(frame.bgra.len());
        for px in frame.bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut encoder = png::Encoder::new(file, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&rgba))
            .map_err(|e| Error::System(format!("cannot write the PNG: {e}")))?;
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.stop_recording();
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
    rec: &Arc<Mutex<Recorder>>,
) -> Result<AudioCapture> {
    const AAC_FRAME_TICKS: i64 = 1024 * TICKS_PER_SEC / 48_000;
    let buffer = Arc::clone(buffer);
    let rec = Arc::clone(rec);
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
        for mut p in packets.drain(..) {
            // Timestamps come from the frame count, not from the MFT.
            p.pts = emitted * AAC_FRAME_TICKS;
            p.duration = AAC_FRAME_TICKS;
            emitted += 1;
            emit(&buffer, &rec, track, p);
        }
    })
}

fn open_encoder(
    choice: EncoderChoice,
    w: u32,
    h: u32,
    fps: u32,
    bitrate_bps: u32,
) -> Result<H264Encoder> {
    if choice != EncoderChoice::Software {
        match H264Encoder::new_hardware(w, h, fps, bitrate_bps, fps) {
            Ok(e) => return Ok(e),
            Err(e) if choice == EncoderChoice::Auto => {
                tracing::info!("no usable hardware encoder ({e}); using software");
            }
            Err(e) => return Err(e),
        }
    }
    H264Encoder::new_software(w, h, fps, bitrate_bps, fps)
}

fn encode_loop(
    mut settings: PipelineSettings,
    started: Instant,
    latest: &LatestFrame,
    buffer: &Mutex<ReplayBuffer>,
    info: &Mutex<Option<VideoInfo>>,
    rec: &Mutex<Recorder>,
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
            match open_encoder(settings.encoder, w, h, fps, settings.bitrate_bps) {
                Ok(e) => {
                    tracing::info!(
                        "encoder ready ({}): {w}x{h} @ {fps} fps, {} bps",
                        if e.is_hardware() {
                            "hardware"
                        } else {
                            "software"
                        },
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
                // Publish the stream info first so a recording can start on this very packet.
                if let Ok(mut i) = info.lock() {
                    if i.as_ref() != Some(enc.info()) {
                        *i = Some(enc.info().clone());
                        if let Ok(mut r) = rec.lock() {
                            r.video_info = Some(enc.info().clone());
                        }
                    }
                }
                for p in packets.drain(..) {
                    emit(buffer, rec, 0, p);
                }
            }
            Err(e) => {
                tracing::warn!("encode failed, restarting the encoder: {e}");
                // A GPU encoder that fails at runtime is not trusted again this session.
                if enc.is_hardware() && settings.encoder == EncoderChoice::Auto {
                    settings.encoder = EncoderChoice::Software;
                }
                encoder = None;
            }
        }
    }
}
