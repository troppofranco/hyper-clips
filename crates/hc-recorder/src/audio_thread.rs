//! Audio thread: WASAPI sources → aligned mixer → one AAC encoder per track.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use hc_capture::audio::{AudioSource, Mixer, SourceKind};
use hc_core::media::{AudioInfo, AudioRole, Packet, now_ticks};
use hc_encode::aac::{AacEncoder, CHANNELS, SAMPLE_RATE};

use crate::sink::SinkMsg;
use crate::{Event, ServiceConfig};

/// Late-arriving WASAPI packets are tolerated up to this delay.
const LATENCY: Duration = Duration::from_millis(150);

pub fn spawn(
    cfg: ServiceConfig,
    t0: i64,
    stop: Arc<AtomicBool>,
    sink: Sender<SinkMsg>,
    events: Sender<Event>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("audio-mix".into())
        .spawn(move || {
            hc_encode::mf::init_thread();
            let mut kinds = Vec::new();
            if cfg.audio.system {
                kinds.push((
                    SourceKind::SystemLoopback,
                    cfg.audio.system_volume as f32 / 100.0,
                ));
            }
            if cfg.audio.microphone {
                kinds.push((
                    SourceKind::Microphone,
                    cfg.audio.microphone_volume as f32 / 100.0,
                ));
            }
            // Track layout: Mix first (what players play), then stems if >1 source.
            let mut roles = vec![AudioRole::Mix];
            if kinds.len() > 1 {
                for (k, _) in &kinds {
                    roles.push(match k {
                        SourceKind::SystemLoopback => AudioRole::System,
                        SourceKind::Microphone => AudioRole::Microphone,
                    });
                }
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

            let (tx, rx) = bounded(512);
            let _sources: Vec<AudioSource> = kinds
                .iter()
                .map(|(k, _)| AudioSource::start(*k, tx.clone()))
                .collect();
            let mut mixer = Mixer::new(t0, &kinds, LATENCY);
            while !stop.load(Ordering::Relaxed) {
                if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(10)) {
                    mixer.push(&chunk);
                    while let Ok(c) = rx.try_recv() {
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
            }
            tracing::info!("audio thread stopped");
        })
}
