//! Audio thread: WASAPI sources → mic DSP → aligned mixer → AAC per track.
//!
//! Track layout is fixed for the lifetime of the service (MP4 tracks cannot
//! appear mid-stream). With separate tracks enabled:
//!
//! | track | content                                   | in mix |
//! |-------|-------------------------------------------|--------|
//! | Mix   | game + other sounds + mic                 |   —    |
//! | Game  | game process tree                         |  yes   |
//! | Chat  | voice chat app (Discord, …) process tree  |   no*  |
//! | Other | everything except the game (incl. chat)   |  yes   |
//! | Mic   | microphone (if enabled)                   |  yes   |
//!
//! *chat is already contained in "Other", so it is not added twice.
//! Without a known game, "Other" captures the whole system.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use hc_capture::audio::{AudioSource, Chunk, Endpoint, Mixer, SourceKind};
use hc_capture::dsp::{Denoiser, NoiseGate};
use hc_core::media::{AudioInfo, AudioRole, Packet, now_ticks};
use hc_encode::aac::{AacEncoder, CHANNELS, SAMPLE_RATE};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use crate::sink::SinkMsg;
use crate::{Event, ServiceConfig};

/// Late-arriving WASAPI packets are tolerated up to this delay.
const LATENCY: Duration = Duration::from_millis(150);
const CHAT_REFRESH: Duration = Duration::from_secs(5);

pub enum AudioCmd {
    SetGame(Option<u32>),
}

/// Microphone processing chain.
struct MicChain {
    denoise: Option<Denoiser>,
    gate: Option<NoiseGate>,
    ptt_vk: Option<u16>,
}

impl MicChain {
    fn new(cfg: &ServiceConfig) -> Self {
        let ptt_vk = hc_hotkeys::Hotkey::parse_optional(&cfg.audio.push_to_talk)
            .ok()
            .flatten()
            .map(|h| match h.key {
                hc_hotkeys::Key::Vk(vk) => vk,
                hc_hotkeys::Key::Mouse(hc_hotkeys::MouseButton::Middle) => 0x04,
                hc_hotkeys::Key::Mouse(hc_hotkeys::MouseButton::X1) => 0x05,
                hc_hotkeys::Key::Mouse(hc_hotkeys::MouseButton::X2) => 0x06,
            });
        Self {
            denoise: cfg.audio.noise_suppression.then(Denoiser::new),
            gate: cfg
                .audio
                .noise_gate
                .then(|| NoiseGate::new(cfg.audio.noise_gate_db)),
            ptt_vk,
        }
    }

    fn process(&mut self, samples: &mut [i16]) {
        if let Some(d) = &mut self.denoise {
            d.process(samples);
        }
        if let Some(g) = &mut self.gate {
            g.process(samples);
        }
        if let Some(vk) = self.ptt_vk {
            // Polling key state needs no hook and works in fullscreen games.
            // SAFETY: GetAsyncKeyState has no preconditions.
            let held = unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
            if !held {
                samples.fill(0);
            }
        }
    }
}

struct Sources {
    tx: Sender<Chunk>,
    separate: bool,
    chat_apps: Vec<String>,
    game: Option<AudioSource>,
    chat: Option<(u32, AudioSource)>,
    other: Option<AudioSource>,
    _mic: Option<AudioSource>,
    game_pid: Option<u32>,
}

impl Sources {
    fn start(cfg: &ServiceConfig, tx: Sender<Chunk>) -> Self {
        let separate = cfg.audio.system && cfg.audio.separate_tracks;
        let mut s = Self {
            tx: tx.clone(),
            separate,
            chat_apps: cfg.audio.chat_apps.clone(),
            game: None,
            chat: None,
            other: None,
            _mic: cfg
                .audio
                .microphone
                .then(|| AudioSource::start(SourceKind::Microphone, Endpoint::DefaultCapture, tx)),
            game_pid: None,
        };
        if cfg.audio.system {
            s.set_game(cfg.game_pid);
        }
        s.refresh_chat();
        s
    }

    fn set_game(&mut self, pid: Option<u32>) {
        if self.other.is_some() && pid == self.game_pid {
            return;
        }
        self.game_pid = pid;
        // Drop old sources first so their threads stop before new ones start.
        self.game = None;
        self.other = None;
        match (self.separate, pid) {
            (true, Some(pid)) => {
                tracing::info!(pid, "per-process audio: game + everything else");
                self.game = Some(AudioSource::start(
                    SourceKind::Game,
                    Endpoint::Process { pid, include: true },
                    self.tx.clone(),
                ));
                self.other = Some(AudioSource::start(
                    SourceKind::SystemLoopback,
                    Endpoint::Process {
                        pid,
                        include: false,
                    },
                    self.tx.clone(),
                ));
            }
            _ => {
                self.other = Some(AudioSource::start(
                    SourceKind::SystemLoopback,
                    Endpoint::DefaultRender,
                    self.tx.clone(),
                ));
            }
        }
    }

    fn refresh_chat(&mut self) {
        if !self.separate {
            return;
        }
        let root = hc_core::sys::process_tree_roots(&self.chat_apps)
            .into_iter()
            .next();
        match (root, &self.chat) {
            (Some(pid), Some((cur, _))) if *cur == pid => {}
            (Some(pid), _) => {
                tracing::info!(pid, "voice chat detected, capturing its own track");
                self.chat = Some((
                    pid,
                    AudioSource::start(
                        SourceKind::Chat,
                        Endpoint::Process { pid, include: true },
                        self.tx.clone(),
                    ),
                ));
            }
            (None, Some(_)) => self.chat = None,
            (None, None) => {}
        }
    }
}

pub fn spawn(
    cfg: ServiceConfig,
    t0: i64,
    stop: Arc<AtomicBool>,
    sink: Sender<SinkMsg>,
    events: Sender<Event>,
    cmds: Receiver<AudioCmd>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("audio-mix".into())
        .spawn(move || {
            hc_encode::mf::init_thread();
            let separate = cfg.audio.system && cfg.audio.separate_tracks;
            let sys_gain = cfg.audio.system_volume as f32 / 100.0;
            let mic_gain = cfg.audio.microphone_volume as f32 / 100.0;

            // (queue kind, gain, track role) in stem order.
            let mut stems: Vec<(SourceKind, f32, AudioRole)> = Vec::new();
            if separate {
                stems.push((SourceKind::Game, sys_gain, AudioRole::Game));
                stems.push((SourceKind::Chat, sys_gain, AudioRole::Chat));
                stems.push((SourceKind::SystemLoopback, sys_gain, AudioRole::System));
            } else if cfg.audio.system {
                stems.push((SourceKind::SystemLoopback, sys_gain, AudioRole::System));
            }
            if cfg.audio.microphone {
                stems.push((SourceKind::Microphone, mic_gain, AudioRole::Microphone));
            }
            // A single source needs no separate stem: the mix is that source.
            let mut roles = vec![AudioRole::Mix];
            if stems.len() > 1 {
                roles.extend(stems.iter().map(|s| s.2));
            }
            let bitrate = cfg.audio.bitrate_kbps * 1000;
            let mut encoders = Vec::new();
            for _ in &roles {
                match AacEncoder::new(bitrate) {
                    Ok(e) => encoders.push(e),
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("AAC encoder unavailable: {e}")));
                        return;
                    }
                }
            }
            let infos: Vec<AudioInfo> = roles
                .iter()
                .zip(&encoders)
                .map(|(role, enc)| AudioInfo {
                    role: *role,
                    sample_rate: SAMPLE_RATE,
                    channels: CHANNELS,
                    asc: enc.asc(),
                    bitrate: enc.bitrate,
                })
                .collect();
            let _ = sink.send(SinkMsg::AudioInfo(infos));

            let kinds: Vec<(SourceKind, f32)> = stems.iter().map(|s| (s.0, s.1)).collect();
            let mut mixer = Mixer::new(t0, &kinds, LATENCY);
            mixer.set_in_mix(SourceKind::Chat, false);

            let (tx, rx) = bounded(512);
            let mut sources = Sources::start(&cfg, tx);
            let mut mic = MicChain::new(&cfg);
            let mut last_chat_check = Instant::now();
            // Per-track level meter for diagnostics (debug log every 2 s).
            let mut meter = vec![(0f64, 0u64); roles.len()];
            let mut last_meter = Instant::now();

            while !stop.load(Ordering::Relaxed) {
                while let Ok(cmd) = cmds.try_recv() {
                    match cmd {
                        AudioCmd::SetGame(pid) => sources.set_game(pid),
                    }
                }
                if last_chat_check.elapsed() > CHAT_REFRESH {
                    sources.refresh_chat();
                    last_chat_check = Instant::now();
                }
                if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(10)) {
                    for mut c in std::iter::once(chunk).chain(rx.try_iter()) {
                        if c.source == SourceKind::Microphone {
                            mic.process(&mut c.samples);
                        }
                        mixer.push(&c);
                    }
                }
                for block in mixer.pull(now_ticks()) {
                    for (track, enc) in encoders.iter_mut().enumerate() {
                        let pcm = if track == 0 {
                            &block.mix
                        } else {
                            &block.sources[track - 1]
                        };
                        let m = &mut meter[track];
                        m.0 += pcm.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
                        m.1 += pcm.len() as u64;
                        match enc.encode(pcm, block.pts) {
                            Ok(frames) => {
                                for f in frames {
                                    let _ = sink.send(SinkMsg::Packet(Packet {
                                        track: (track + 1) as u8,
                                        pts: f.pts,
                                        dur: f.dur,
                                        key: true,
                                        data: Arc::from(f.data.into_boxed_slice()),
                                    }));
                                }
                            }
                            Err(e) => tracing::warn!(track, error = %e, "AAC encode failed"),
                        }
                    }
                }
                if last_meter.elapsed() > Duration::from_secs(2) {
                    log_levels(&roles, &mut meter);
                    last_meter = Instant::now();
                }
            }
            tracing::info!("audio thread stopped");
        })
}

fn log_levels(roles: &[AudioRole], meter: &mut [(f64, u64)]) {
    let levels: Vec<String> = roles
        .iter()
        .zip(meter.iter_mut())
        .map(|(r, m)| {
            let rms = (m.0 / m.1.max(1) as f64).sqrt();
            *m = (0.0, 0);
            let db = if rms <= 0.0 {
                -120.0
            } else {
                20.0 * (rms / 32768.0).log10()
            };
            format!("{}={db:.1}dB", r.label())
        })
        .collect();
    tracing::debug!(levels = %levels.join(" "), "audio levels");
}
