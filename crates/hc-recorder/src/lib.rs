//! The capture service: owns the video, audio and sink threads.
//!
//! ```text
//! video thread : Desktop Duplication → NV12 (GPU) → HW encoder ─┐
//! audio thread : WASAPI loopback + mic → mixer → AAC ×N ────────┼─▶ sink thread
//!                                                               │   ├ replay ring buffer
//!                                                               │   └ active recording (MP4)
//! ```
//! One encoder feeds both the replay buffer and a running recording.

mod audio_thread;
mod sink;
mod video_thread;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Sender, bounded, unbounded};
use hc_capture::desktop::MonitorSelect;
use hc_core::config::{Audio, Capture, Codec, RateControl};
use hc_core::media::{VideoCodec, now_ticks};
use hc_encode::video::Rate;

pub use sink::SinkCmd;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("capture: {0}")]
    Capture(#[from] hc_capture::CaptureError),
    #[error("encoder: {0}")]
    Encode(#[from] hc_encode::video::EncodeError),
    #[error("{0}")]
    Other(String),
}

/// Notifications for the UI thread.
#[derive(Debug, Clone)]
pub enum Event {
    Started {
        encoder: String,
        hardware: bool,
        width: u32,
        height: u32,
        monitor: String,
    },
    ClipSaved {
        path: PathBuf,
        seconds: f64,
    },
    RecordingStarted {
        path: PathBuf,
    },
    RecordingStopped {
        path: PathBuf,
        seconds: f64,
        markers: usize,
    },
    MarkerAdded {
        at_seconds: f64,
    },
    ScreenshotSaved {
        path: PathBuf,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub capture: Capture,
    pub audio: Audio,
    pub monitor: MonitorSelect,
    /// Seconds kept in RAM when Instant Replay is off (pre-roll for recordings).
    pub min_buffer_seconds: u32,
}

impl ServiceConfig {
    pub(crate) fn codec(&self) -> VideoCodec {
        match self.capture.codec {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::Hevc,
            Codec::Av1 => {
                tracing::warn!("AV1 capture not available yet, using H.264");
                VideoCodec::H264
            }
        }
    }

    pub(crate) fn rate(&self) -> Rate {
        let kbps = self.capture.bitrate_kbps;
        match self.capture.rate_control {
            RateControl::Cbr => Rate::Cbr { kbps },
            RateControl::Vbr => Rate::Vbr { kbps },
            RateControl::Cqp => Rate::Cqp {
                qp: self.capture.cqp,
            },
        }
    }

    pub(crate) fn buffer_seconds(&self) -> u32 {
        if self.capture.replay_enabled {
            self.capture.replay_seconds
        } else {
            self.min_buffer_seconds
        }
    }
}

pub struct CaptureService {
    sink: Sender<sink::SinkMsg>,
    video_cmd: Sender<video_thread::VideoCmd>,
    stop: Arc<AtomicBool>,
    recording: Arc<AtomicBool>,
    buffer_bytes: Arc<AtomicUsize>,
    threads: Vec<JoinHandle<()>>,
}

impl CaptureService {
    /// Start capturing. Returns once the video pipeline is initialised (or failed).
    pub fn start(cfg: ServiceConfig, events: Sender<Event>) -> Result<Self, ServiceError> {
        let stop = Arc::new(AtomicBool::new(false));
        let recording = Arc::new(AtomicBool::new(false));
        let buffer_bytes = Arc::new(AtomicUsize::new(0));
        let (sink_tx, sink_rx) = unbounded();
        let (video_tx, video_rx) = unbounded();
        let t0 = now_ticks();

        let mut threads = Vec::new();
        let sink_thread = sink::spawn(
            cfg.clone(),
            sink_rx,
            video_tx.clone(),
            events.clone(),
            recording.clone(),
            buffer_bytes.clone(),
        )
        .map_err(|e| ServiceError::Other(e.to_string()))?;
        threads.push(sink_thread);

        let (ready_tx, ready_rx) = bounded(1);
        let video = video_thread::spawn(
            cfg.clone(),
            t0,
            stop.clone(),
            video_rx,
            sink_tx.clone(),
            events.clone(),
            ready_tx,
        )
        .map_err(|e| ServiceError::Other(e.to_string()))?;
        threads.push(video);
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                stop.store(true, Ordering::SeqCst);
                let _ = sink_tx.send(sink::SinkMsg::Shutdown);
                for t in threads {
                    let _ = t.join();
                }
                return Err(ServiceError::Other(e));
            }
            Err(_) => return Err(ServiceError::Other("video thread died".into())),
        }

        if cfg.audio.system || cfg.audio.microphone {
            let audio = audio_thread::spawn(cfg, t0, stop.clone(), sink_tx.clone(), events)
                .map_err(|e| ServiceError::Other(e.to_string()))?;
            threads.push(audio);
        }
        Ok(Self {
            sink: sink_tx,
            video_cmd: video_tx,
            stop,
            recording,
            buffer_bytes,
            threads,
        })
    }

    pub fn command(&self, cmd: SinkCmd) {
        let _ = self.sink.send(sink::SinkMsg::Cmd(cmd));
    }

    pub fn screenshot(&self, path: PathBuf, jpeg: bool) {
        let _ = self
            .video_cmd
            .send(video_thread::VideoCmd::Screenshot { path, jpeg });
    }

    /// Bytes of encoded media currently held by the replay buffer.
    pub fn buffer_bytes(&self) -> usize {
        self.buffer_bytes.load(Ordering::Relaxed)
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }
}

impl Drop for CaptureService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Sink finalises an active recording on shutdown.
        let _ = self.sink.send(sink::SinkMsg::Shutdown);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
