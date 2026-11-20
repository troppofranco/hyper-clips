//! WASAPI audio capture (shared mode) and a clock-aligned mixer.
//!
//! Sources deliver 48 kHz stereo i16 chunks stamped with the QPC time of
//! their first frame. The [`Mixer`] places them on one sample grid shared
//! with video, fills gaps (loopback delivers nothing while the system is
//! silent) with zeros, and emits fixed 1024-frame blocks (one AAC frame).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Sender;
use hc_core::media::TIMEBASE;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, eCapture,
    eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Frames per mixer block (= one AAC frame).
pub const BLOCK: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    SystemLoopback,
    Microphone,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub source: SourceKind,
    /// QPC time of the first frame, in 100 ns ticks.
    pub pts: i64,
    /// Interleaved stereo samples.
    pub samples: Vec<i16>,
}

/// A running capture thread for one endpoint.
pub struct AudioSource {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioSource {
    pub fn start(kind: SourceKind, tx: Sender<Chunk>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("audio-{kind:?}"))
            .spawn(move || {
                // SAFETY: per-thread COM initialisation.
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                while !st.load(Ordering::Relaxed) {
                    if let Err(e) = run_endpoint(kind, &tx, &st) {
                        tracing::warn!(?kind, error = %e, "audio endpoint unavailable, retrying");
                    }
                    // Device missing or invalidated (unplugged, default changed): retry.
                    for _ in 0..10 {
                        if st.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for AudioSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_endpoint(
    kind: SourceKind,
    tx: &Sender<Chunk>,
    stop: &AtomicBool,
) -> windows::core::Result<()> {
    // SAFETY: WASAPI calls on interfaces owned by this function; the capture
    // buffer is released after each copy.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let flow = match kind {
            SourceKind::SystemLoopback => eRender,
            SourceKind::Microphone => eCapture,
        };
        let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * (CHANNELS as u32) * 2,
            nBlockAlign: (CHANNELS as u16) * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut flags =
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if kind == SourceKind::SystemLoopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &format, None)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        tracing::info!(?kind, "audio capture started");
        let result = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
            let mut failed = None;
            loop {
                match capture.GetNextPacketSize() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut buf_flags = 0u32;
                let mut qpc = 0u64;
                if let Err(e) =
                    capture.GetBuffer(&mut data, &mut frames, &mut buf_flags, None, Some(&mut qpc))
                {
                    failed = Some(e);
                    break;
                }
                let n = frames as usize * CHANNELS;
                let samples =
                    if buf_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        vec![0i16; n]
                    } else {
                        std::slice::from_raw_parts(data as *const i16, n).to_vec()
                    };
                let _ = capture.ReleaseBuffer(frames);
                // qpc is already in 100 ns units.
                let _ = tx.try_send(Chunk {
                    source: kind,
                    pts: qpc as i64,
                    samples,
                });
            }
            if let Some(e) = failed {
                break Err(e);
            }
        };
        let _ = client.Stop();
        result
    }
}

/// One mixer output block.
#[derive(Debug, Clone)]
pub struct Block {
    pub pts: i64,
    /// Per-source samples, in the order sources were registered.
    pub sources: Vec<Vec<i16>>,
    pub mix: Vec<i16>,
}

struct Queue {
    kind: SourceKind,
    gain: f32,
    /// Absolute frame index (on the grid) of `buf[0]`.
    start: i64,
    buf: VecDeque<i16>,
}

impl Queue {
    fn end(&self) -> i64 {
        self.start + (self.buf.len() / CHANNELS) as i64
    }
}

/// Aligns sources on a shared 48 kHz grid starting at `t0`.
pub struct Mixer {
    t0: i64,
    queues: Vec<Queue>,
    /// Next block index to emit.
    next: i64,
    latency_frames: i64,
}

/// Tolerated jitter before inserting silence or dropping overlap.
const SLACK: i64 = 480;

fn frame_of(t0: i64, pts: i64) -> i64 {
    let d = i128::from(pts - t0) * i128::from(RATE);
    ((d + i128::from(TIMEBASE) / 2) / i128::from(TIMEBASE)) as i64
}

impl Mixer {
    pub fn new(t0: i64, sources: &[(SourceKind, f32)], latency: Duration) -> Self {
        Self {
            t0,
            queues: sources
                .iter()
                .map(|(kind, gain)| Queue {
                    kind: *kind,
                    gain: *gain,
                    start: 0,
                    buf: VecDeque::new(),
                })
                .collect(),
            next: 0,
            latency_frames: (latency.as_secs_f64() * f64::from(RATE)) as i64,
        }
    }

    pub fn set_gain(&mut self, kind: SourceKind, gain: f32) {
        for q in &mut self.queues {
            if q.kind == kind {
                q.gain = gain;
            }
        }
    }

    pub fn block_pts(&self, index: i64) -> i64 {
        self.t0
            + ((i128::from(index) * BLOCK as i128 * i128::from(TIMEBASE)) / i128::from(RATE)) as i64
    }

    pub fn push(&mut self, chunk: &Chunk) {
        let consumed = self.next * BLOCK as i64;
        let Some(q) = self.queues.iter_mut().find(|q| q.kind == chunk.source) else {
            return;
        };
        let at = frame_of(self.t0, chunk.pts);
        let mut samples: &[i16] = &chunk.samples;
        if q.buf.is_empty() {
            q.start = q.start.max(consumed);
            if at > q.start {
                // First data or after a silent gap: pad up to `at`.
                let pad = (at - q.start) as usize * CHANNELS;
                q.buf.extend(std::iter::repeat_n(0, pad));
            }
        }
        let end = q.end();
        if at > end + SLACK {
            q.buf
                .extend(std::iter::repeat_n(0, (at - end) as usize * CHANNELS));
        } else if at < end - SLACK {
            let skip = ((end - at) as usize * CHANNELS).min(samples.len());
            samples = &samples[skip..];
        }
        q.buf.extend(samples.iter().copied());
    }

    /// Emit every block that is complete as of `now` (minus latency).
    pub fn pull(&mut self, now: i64) -> Vec<Block> {
        let ready_until = frame_of(self.t0, now) - self.latency_frames;
        let mut out = Vec::new();
        while (self.next + 1) * BLOCK as i64 <= ready_until {
            let from = self.next * BLOCK as i64;
            let mut mix = vec![0f32; BLOCK * CHANNELS];
            let mut sources = Vec::with_capacity(self.queues.len());
            for q in &mut self.queues {
                // Drop anything before the block start.
                if q.start < from {
                    let drop = (((from - q.start) as usize) * CHANNELS).min(q.buf.len());
                    q.buf.drain(..drop);
                    q.start = from;
                }
                let mut block = vec![0i16; BLOCK * CHANNELS];
                let offset = ((q.start - from).max(0) as usize * CHANNELS).min(block.len());
                let take = (block.len() - offset).min(q.buf.len());
                for (i, s) in q.buf.drain(..take).enumerate() {
                    block[offset + i] = s;
                }
                q.start = from + BLOCK as i64;
                let gain = q.gain;
                for (m, s) in mix.iter_mut().zip(&block) {
                    *m += f32::from(*s) * gain;
                }
                if (gain - 1.0).abs() > f32::EPSILON {
                    for s in &mut block {
                        *s = (f32::from(*s) * gain).clamp(-32768.0, 32767.0) as i16;
                    }
                }
                sources.push(block);
            }
            out.push(Block {
                pts: self.block_pts(self.next),
                sources,
                mix: mix
                    .iter()
                    .map(|v| v.clamp(-32768.0, 32767.0) as i16)
                    .collect(),
            });
            self.next += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_000_000_000;

    fn ticks(frames: i64) -> i64 {
        frames * TIMEBASE / i64::from(RATE)
    }

    fn chunk(kind: SourceKind, frame: i64, frames: usize, value: i16) -> Chunk {
        Chunk {
            source: kind,
            pts: T0 + ticks(frame),
            samples: vec![value; frames * CHANNELS],
        }
    }

    #[test]
    fn emits_silence_when_source_is_quiet() {
        let mut m = Mixer::new(T0, &[(SourceKind::SystemLoopback, 1.0)], Duration::ZERO);
        let blocks = m.pull(T0 + ticks(4096));
        assert_eq!(blocks.len(), 4);
        assert!(blocks.iter().all(|b| b.mix.iter().all(|s| *s == 0)));
        assert_eq!(blocks[1].pts, T0 + ticks(1024));
    }

    #[test]
    fn places_chunks_on_the_grid() {
        let mut m = Mixer::new(T0, &[(SourceKind::SystemLoopback, 1.0)], Duration::ZERO);
        // Data starts half-way into block 0.
        m.push(&chunk(SourceKind::SystemLoopback, 512, 1536, 100));
        let b = m.pull(T0 + ticks(2048));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].mix[0], 0);
        assert_eq!(b[0].mix[511 * 2], 0);
        assert_eq!(b[0].mix[512 * 2], 100);
        assert!(b[1].mix.iter().all(|s| *s == 100));
    }

    #[test]
    fn mixes_and_applies_gain() {
        let mut m = Mixer::new(
            T0,
            &[
                (SourceKind::SystemLoopback, 1.0),
                (SourceKind::Microphone, 0.5),
            ],
            Duration::ZERO,
        );
        m.push(&chunk(SourceKind::SystemLoopback, 0, 1024, 1000));
        m.push(&chunk(SourceKind::Microphone, 0, 1024, 1000));
        let b = m.pull(T0 + ticks(1024));
        assert_eq!(b.len(), 1);
        assert!(b[0].mix.iter().all(|s| *s == 1500));
        assert!(b[0].sources[0].iter().all(|s| *s == 1000));
        assert!(b[0].sources[1].iter().all(|s| *s == 500));
    }

    #[test]
    fn mix_clips_instead_of_wrapping() {
        let mut m = Mixer::new(
            T0,
            &[
                (SourceKind::SystemLoopback, 1.0),
                (SourceKind::Microphone, 1.0),
            ],
            Duration::ZERO,
        );
        m.push(&chunk(SourceKind::SystemLoopback, 0, 1024, 30000));
        m.push(&chunk(SourceKind::Microphone, 0, 1024, 30000));
        assert!(m.pull(T0 + ticks(1024))[0].mix.iter().all(|s| *s == 32767));
    }

    #[test]
    fn respects_latency() {
        let mut m = Mixer::new(
            T0,
            &[(SourceKind::Microphone, 1.0)],
            Duration::from_millis(100),
        );
        assert!(m.pull(T0 + ticks(4800)).is_empty());
        assert_eq!(m.pull(T0 + ticks(4800 + 1024)).len(), 1);
    }

    #[test]
    fn gap_and_jitter_handling() {
        let mut m = Mixer::new(T0, &[(SourceKind::SystemLoopback, 1.0)], Duration::ZERO);
        m.push(&chunk(SourceKind::SystemLoopback, 0, 1024, 7));
        // Small jitter (10 frames late) is absorbed: stays contiguous.
        m.push(&chunk(SourceKind::SystemLoopback, 1034, 1024, 9));
        // Big gap (silence) before the next chunk.
        m.push(&chunk(SourceKind::SystemLoopback, 4096, 1024, 5));
        let b = m.pull(T0 + ticks(5120));
        assert_eq!(b.len(), 5);
        assert!(b[0].mix.iter().all(|s| *s == 7));
        assert!(b[1].mix.iter().all(|s| *s == 9));
        assert!(b[2].mix.iter().all(|s| *s == 0));
        assert!(b[4].mix.iter().all(|s| *s == 5));
    }
}
