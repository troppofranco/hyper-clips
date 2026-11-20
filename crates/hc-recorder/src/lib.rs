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
use hc_capture::screenshot::ImageFormat;
use hc_core::config::{Audio, Capture, Codec, RateControl, Webcam};
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

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// A monitor, optionally cropped to a virtual-desktop region (x, y, w, h).
    Monitor {
        select: MonitorSelect,
        region: Option<(i32, i32, u32, u32)>,
    },
    /// One window (WGC); center picks the GPU/monitor it lives on.
    Window { hwnd: isize, center: (i32, i32) },
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub capture: Capture,
    pub audio: Audio,
    pub webcam: Webcam,
    pub target: CaptureTarget,
    /// Process of the game in focus, for per-process audio.
    pub game_pid: Option<u32>,
    /// Where a disk-backed replay buffer may live (deleted on close).
    pub tmp_dir: std::path::PathBuf,
    /// Seconds kept in RAM when Instant Replay is off (pre-roll for recordings).
    pub min_buffer_seconds: u32,
}

impl ServiceConfig {
    pub(crate) fn codec(&self) -> VideoCodec {
        match self.capture.codec {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::Hevc,
            Codec::Av1 => VideoCodec::Av1,
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
    audio_cmd: Option<Sender<audio_thread::AudioCmd>>,
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

        let mut audio_cmd = None;
        if cfg.audio.system || cfg.audio.microphone {
            let (tx, rx) = unbounded();
            let audio = audio_thread::spawn(cfg, t0, stop.clone(), sink_tx.clone(), events, rx)
                .map_err(|e| ServiceError::Other(e.to_string()))?;
            threads.push(audio);
            audio_cmd = Some(tx);
        }
        Ok(Self {
            sink: sink_tx,
            video_cmd: video_tx,
            stop,
            recording,
            buffer_bytes,
            audio_cmd,
            threads,
        })
    }

    pub fn command(&self, cmd: SinkCmd) {
        let _ = self.sink.send(sink::SinkMsg::Cmd(cmd));
    }

    pub fn screenshot(&self, path: PathBuf, format: ImageFormat) {
        let _ = self
            .video_cmd
            .send(video_thread::VideoCmd::Screenshot { path, format });
    }

    /// Tell the audio thread which process is the game (per-process tracks).
    pub fn set_game_pid(&self, pid: Option<u32>) {
        if let Some(tx) = &self.audio_cmd {
            let _ = tx.send(audio_thread::AudioCmd::SetGame(pid));
        }
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
