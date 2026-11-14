//! Shared-mode WASAPI capture, polled every 10 ms.
//!
//! The stream is requested as 48 kHz / stereo / 16-bit PCM with automatic conversion, so
//! whatever the device mix format is, consumers always get the same layout.

use crate::timeline::{CHANNELS, SAMPLE_RATE};
use hc_core::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

// Stream flags, spelled as numbers so they do not depend on the crate's constant types.
const FLAG_LOOPBACK: u32 = 0x0002_0000;
const FLAG_AUTOCONVERTPCM: u32 = 0x8000_0000;
const FLAG_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const BUFFERFLAGS_SILENT: u32 = 0x2;

fn sys<E: std::fmt::Display>(what: &str, e: E) -> Error {
    Error::System(format!("{what}: {e}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Everything played on the default output device.
    System,
    /// Default recording device.
    Microphone,
}

/// A running capture. Dropping it stops the thread.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// `on_pcm` runs on the capture thread roughly every 10 ms with interleaved 16-bit stereo
    /// samples. The slice is EMPTY when nothing arrived in that period, so the consumer can
    /// keep its timeline moving.
    pub fn start(source: Source, mut on_pcm: impl FnMut(&[i16]) + Send + 'static) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name(format!("hc-audio-{source:?}"))
            .spawn(move || {
                // SAFETY: plain FFI; "already initialised" is harmless.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let mut first = true;
                while !thread_stop.load(Ordering::Relaxed) {
                    match open(source) {
                        Ok((client, capture)) => {
                            if first {
                                let _ = ready_tx.send(Ok(()));
                                first = false;
                            }
                            if let Err(e) = run(&client, &capture, &mut on_pcm, &thread_stop) {
                                tracing::warn!("audio {source:?} interrupted: {e}");
                            }
                            // SAFETY: stopping a started client.
                            let _ = unsafe { client.Stop() };
                        }
                        Err(e) => {
                            if first {
                                let _ = ready_tx.send(Err(e));
                                return;
                            }
                            tracing::warn!("audio {source:?} cannot reopen: {e}");
                        }
                    }
                    // Device changed or vanished: retry shortly.
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .map_err(|e| sys("cannot start the audio thread", e))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(Error::System("audio thread ended unexpectedly".into())),
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open(source: Source) -> Result<(IAudioClient, IAudioCaptureClient)> {
    // SAFETY: standard WASAPI activation sequence; every pointer passed is valid for the
    // call, and the format struct outlives `Initialize`.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| sys("device enumerator", e))?;
        let (flow, flags) = match source {
            Source::System => (eRender, FLAG_LOOPBACK),
            Source::Microphone => (eCapture, 0),
        };
        let device = enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
            .map_err(|e| sys("default audio device", e))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| sys("activate audio client", e))?;

        let block = (CHANNELS * 2) as u16;
        let format = WAVEFORMATEX {
            wFormatTag: 1, // WAVE_FORMAT_PCM
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags | FLAG_AUTOCONVERTPCM | FLAG_SRC_DEFAULT_QUALITY,
                2_000_000, // 200 ms buffer
                0,
                &format,
                None,
            )
            .map_err(|e| sys("audio Initialize", e))?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| sys("IAudioCaptureClient", e))?;
        client.Start().map_err(|e| sys("audio Start", e))?;
        Ok((client, capture))
    }
}

fn run(
    _client: &IAudioClient,
    capture: &IAudioCaptureClient,
    on_pcm: &mut impl FnMut(&[i16]),
    stop: &AtomicBool,
) -> Result<()> {
    let mut samples: Vec<i16> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        samples.clear();
        // SAFETY: GetBuffer returns a pointer valid until ReleaseBuffer; we copy
        // `frames * CHANNELS` samples out of it (or zeros when flagged silent) and release.
        unsafe {
            loop {
                let pending = capture
                    .GetNextPacketSize()
                    .map_err(|e| sys("GetNextPacketSize", e))?;
                if pending == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| sys("GetBuffer", e))?;
                let count = frames as usize * CHANNELS as usize;
                if flags & BUFFERFLAGS_SILENT != 0 || data.is_null() {
                    samples.resize(samples.len() + count, 0);
                } else {
                    let src = std::slice::from_raw_parts(data as *const i16, count);
                    samples.extend_from_slice(src);
                }
                capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| sys("ReleaseBuffer", e))?;
            }
        }
        on_pcm(&samples);
    }
    Ok(())
}
