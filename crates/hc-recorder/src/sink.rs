//! Sink thread: replay ring buffer, clip saving, live recording, markers.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use hc_buffer::ReplayBuffer;
use hc_core::media::{AudioInfo, Packet, StreamSet, TIMEBASE, VideoInfo, now_ticks};
use hc_encode::mp4::Mp4Writer;

use crate::video_thread::VideoCmd;
use crate::{Event, ServiceConfig};

/// Give late audio this long to arrive before writing a clip.
const SAVE_DELAY: i64 = 3 * TIMEBASE / 10;
/// Markers older than this are forgotten.
const MARKER_HISTORY: i64 = 15 * 60 * TIMEBASE;

#[derive(Debug, Clone)]
pub enum SinkCmd {
    SaveClip { path: PathBuf, seconds: Option<u32> },
    StartRecording { path: PathBuf },
    StopRecording,
    AddMarker,
    SetReplaySeconds(u32),
}

pub enum SinkMsg {
    VideoInfo(VideoInfo),
    AudioInfo(Vec<AudioInfo>),
    Packet(Packet),
    Cmd(SinkCmd),
    Shutdown,
}

struct PendingSave {
    path: PathBuf,
    seconds: Option<u32>,
    requested_at: i64,
}

struct Recording {
    writer: Mp4Writer<BufWriter<File>>,
    path: PathBuf,
    origin: i64,
    started_at: i64,
    markers: Vec<f64>,
}

struct Sink {
    ring: ReplayBuffer,
    video: Option<VideoInfo>,
    audio: Vec<AudioInfo>,
    pending: Vec<PendingSave>,
    recording: Option<Recording>,
    /// Waiting for the next keyframe to start a recording.
    pending_recording: Option<PathBuf>,
    markers: Vec<i64>,
    events: Sender<Event>,
    video_cmd: Sender<VideoCmd>,
    recording_flag: Arc<AtomicBool>,
    buffer_bytes: Arc<AtomicUsize>,
}

fn ring_bytes(cfg: &ServiceConfig, seconds: u32) -> usize {
    let video = u64::from(cfg.capture.bitrate_kbps.max(20_000)) * 1000 / 8 * 2;
    let audio = 4 * 24_000;
    ((video + audio) * u64::from(seconds) + 32 * 1024 * 1024) as usize
}

pub fn spawn(
    cfg: ServiceConfig,
    rx: Receiver<SinkMsg>,
    video_cmd: Sender<VideoCmd>,
    events: Sender<Event>,
    recording_flag: Arc<AtomicBool>,
    buffer_bytes: Arc<AtomicUsize>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("sink".into())
        .spawn(move || {
            let secs = cfg.buffer_seconds();
            let mut sink = Sink {
                ring: ReplayBuffer::new(secs, ring_bytes(&cfg, secs)),
                video: None,
                audio: Vec::new(),
                pending: Vec::new(),
                recording: None,
                pending_recording: None,
                markers: Vec::new(),
                events,
                video_cmd,
                recording_flag,
                buffer_bytes,
            };
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(SinkMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(msg) => sink.handle(msg, &cfg),
                    Err(RecvTimeoutError::Timeout) => {}
                }
                sink.flush_pending_saves();
            }
            sink.stop_recording();
            tracing::info!("sink thread stopped");
        })
}

impl Sink {
    fn streams(&self) -> Option<StreamSet> {
        Some(StreamSet {
            video: self.video.clone()?,
            audio: self.audio.clone(),
        })
    }

    fn handle(&mut self, msg: SinkMsg, cfg: &ServiceConfig) {
        match msg {
            SinkMsg::VideoInfo(v) => self.video = Some(v),
            SinkMsg::AudioInfo(a) => self.audio = a,
            SinkMsg::Packet(p) => self.on_packet(p),
            SinkMsg::Cmd(c) => self.on_cmd(c, cfg),
            SinkMsg::Shutdown => {}
        }
    }

    fn on_packet(&mut self, p: Packet) {
        if p.track == 0
            && p.key
            && let Some(path) = self.pending_recording.take()
        {
            self.begin_recording(path, &[]);
        }
        let mut failed = None;
        if let Some(rec) = self.recording.as_mut()
            && p.pts >= rec.origin
            && let Err(e) = rec.writer.write(&p)
        {
            failed = Some(e.to_string());
        }
        if let Some(e) = failed {
            let _ = self
                .events
                .send(Event::Error(format!("recording stopped: {e}")));
            self.stop_recording();
        }
        self.ring.push(p);
        self.buffer_bytes
            .store(self.ring.bytes(), Ordering::Relaxed);
    }

    fn on_cmd(&mut self, cmd: SinkCmd, cfg: &ServiceConfig) {
        match cmd {
            SinkCmd::SaveClip { path, seconds } => self.pending.push(PendingSave {
                path,
                seconds,
                requested_at: now_ticks(),
            }),
            SinkCmd::StartRecording { path } => {
                if self.recording.is_some() || self.pending_recording.is_some() {
                    return;
                }
                let preroll = self.ring.since_last_keyframe();
                if preroll.is_empty() {
                    // No keyframe buffered yet: start on the next one.
                    let _ = self.video_cmd.send(VideoCmd::ForceKeyframe);
                    self.pending_recording = Some(path);
                    self.recording_flag.store(true, Ordering::SeqCst);
                } else {
                    self.begin_recording(path, &preroll);
                }
            }
            SinkCmd::StopRecording => {
                self.pending_recording = None;
                self.stop_recording();
            }
            SinkCmd::AddMarker => {
                let now = now_ticks();
                self.markers.retain(|m| now - m < MARKER_HISTORY);
                self.markers.push(now);
                let at = self
                    .recording
                    .as_mut()
                    .map(|r| {
                        let s = (now - r.origin) as f64 / TIMEBASE as f64;
                        r.markers.push(s);
                        s
                    })
                    .unwrap_or(0.0);
                let _ = self.events.send(Event::MarkerAdded { at_seconds: at });
            }
            SinkCmd::SetReplaySeconds(s) => {
                let s = if cfg.capture.replay_enabled {
                    s
                } else {
                    cfg.min_buffer_seconds
                };
                self.ring.set_duration(s);
            }
        }
    }

    fn begin_recording(&mut self, path: PathBuf, preroll: &[Packet]) {
        let Some(streams) = self.streams() else {
            let _ = self
                .events
                .send(Event::Error("recording: stream not ready yet".into()));
            return;
        };
        let result = (|| -> Result<Recording, String> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let file = File::create(&path).map_err(|e| e.to_string())?;
            let writer = Mp4Writer::new(BufWriter::with_capacity(1 << 20, file), streams)
                .map_err(|e| e.to_string())?;
            let origin = preroll.first().map_or_else(now_ticks, |p| p.pts);
            let mut rec = Recording {
                writer,
                path: path.clone(),
                origin,
                started_at: origin,
                markers: Vec::new(),
            };
            for p in preroll {
                rec.writer.write(p).map_err(|e| e.to_string())?;
            }
            Ok(rec)
        })();
        match result {
            Ok(rec) => {
                tracing::info!(path = %hc_core::paths::redact(&path), "recording started");
                self.recording = Some(rec);
                self.recording_flag.store(true, Ordering::SeqCst);
                let _ = self.events.send(Event::RecordingStarted { path });
            }
            Err(e) => {
                self.recording_flag.store(false, Ordering::SeqCst);
                let _ = self.events.send(Event::Error(format!("recording: {e}")));
            }
        }
    }

    fn stop_recording(&mut self) {
        self.recording_flag.store(false, Ordering::SeqCst);
        let Some(rec) = self.recording.take() else {
            return;
        };
        let seconds = rec.writer.video_duration_ticks() as f64 / TIMEBASE as f64;
        let _ = rec.started_at;
        match rec.writer.finish() {
            Ok(_) => {
                write_markers(&rec.path, &rec.markers);
                tracing::info!(seconds, "recording stopped");
                let _ = self.events.send(Event::RecordingStopped {
                    path: rec.path,
                    seconds,
                    markers: rec.markers.len(),
                });
            }
            Err(e) => {
                let _ = self
                    .events
                    .send(Event::Error(format!("recording finalise: {e}")));
            }
        }
    }

    fn flush_pending_saves(&mut self) {
        let now = now_ticks();
        let (ready, waiting): (Vec<_>, Vec<_>) = self
            .pending
            .drain(..)
            .partition(|p| now - p.requested_at >= SAVE_DELAY);
        self.pending = waiting;
        for save in ready {
            let Some(streams) = self.streams() else {
                let _ = self.events.send(Event::Error("nothing to save yet".into()));
                continue;
            };
            let end = save.requested_at;
            let packets: Vec<Packet> = self
                .ring
                .snapshot(save.seconds)
                .into_iter()
                .filter(|p| p.pts <= end)
                .collect();
            if packets.is_empty() {
                let _ = self
                    .events
                    .send(Event::Error("replay buffer is empty".into()));
                continue;
            }
            let start = packets[0].pts;
            let markers: Vec<f64> = self
                .markers
                .iter()
                .filter(|m| **m >= start && **m <= end)
                .map(|m| (m - start) as f64 / TIMEBASE as f64)
                .collect();
            let events = self.events.clone();
            // Writing never blocks capture: packets are shared, not copied.
            let spawned = std::thread::Builder::new()
                .name("clip-writer".into())
                .spawn(move || match write_clip(&save.path, streams, &packets) {
                    Ok(seconds) => {
                        write_markers(&save.path, &markers);
                        let _ = events.send(Event::ClipSaved {
                            path: save.path,
                            seconds,
                        });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("saving clip: {e}")));
                    }
                });
            if let Err(e) = spawned {
                tracing::error!(error = %e, "cannot spawn clip writer");
            }
        }
    }
}

fn write_clip(path: &Path, streams: StreamSet, packets: &[Packet]) -> Result<f64, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("mp4.part");
    let file = File::create(&part).map_err(|e| e.to_string())?;
    let mut w = Mp4Writer::new(BufWriter::with_capacity(1 << 20, file), streams)
        .map_err(|e| e.to_string())?;
    for p in packets {
        w.write(p).map_err(|e| e.to_string())?;
    }
    let seconds = w.video_duration_ticks() as f64 / TIMEBASE as f64;
    w.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&part, path).map_err(|e| e.to_string())?;
    Ok(seconds)
}

/// Sidecar `<clip>.markers.json`, imported into the library in phase 3.
fn write_markers(clip: &Path, markers: &[f64]) {
    if markers.is_empty() {
        return;
    }
    let list: Vec<String> = markers.iter().map(|m| format!("{m:.3}")).collect();
    let json = format!("{{\"version\":1,\"markers\":[{}]}}\n", list.join(","));
    let mut path = clip.as_os_str().to_owned();
    path.push(".markers.json");
    if let Err(e) = std::fs::write(PathBuf::from(path), json) {
        tracing::warn!(error = %e, "cannot write markers");
    }
}
