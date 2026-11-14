//! AAC-LC encoder (Microsoft's AAC MFT, synchronous): 48 kHz stereo 16-bit PCM in, raw AAC out.

use crate::mf::{init, sys};
use crate::AudioInfo;
use hc_buffer::Packet;
use hc_core::error::Result;
use std::mem::ManuallyDrop;
use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

/// CLSID_AACMFTEncoder, spelled out so the code does not depend on the constant's name.
const AAC_ENCODER: GUID = GUID::from_u128(0x93af0c51_2275_45d2_a35b_f2ba21caed00);
/// One AAC frame is 1024 samples per channel.
const FRAME_SAMPLES: i64 = 1024;

/// HEAACWAVEINFO tail (payload 0 = raw, level 0x29) followed by the AudioSpecificConfig of
/// AAC-LC / 48 kHz / stereo. The MP4 sink needs this blob to write the `esds` box.
const USER_DATA_48K_STEREO: [u8; 14] = [0, 0, 0x29, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x90];

pub struct AacEncoder {
    transform: IMFTransform,
    info: AudioInfo,
    output_size: u32,
    provides_samples: bool,
}

/// Media type describing the encoded AAC stream (also used for the MP4 sink).
pub(crate) fn aac_type(info: &AudioInfo) -> Result<IMFMediaType> {
    // SAFETY: plain COM calls on an object created here.
    unsafe {
        let t = MFCreateMediaType().map_err(|e| sys("aac type", e))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| sys("major", e))?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(|e| sys("subtype", e))?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| sys("bits", e))?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, info.sample_rate)
            .map_err(|e| sys("rate", e))?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, info.channels)
            .map_err(|e| sys("channels", e))?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, info.bitrate_bps / 8)
            .map_err(|e| sys("avg bytes", e))?;
        t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)
            .map_err(|e| sys("align", e))?;
        t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)
            .map_err(|e| sys("payload", e))?;
        t.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)
            .map_err(|e| sys("profile level", e))?;
        t.SetBlob(&MF_MT_USER_DATA, &USER_DATA_48K_STEREO)
            .map_err(|e| sys("user data", e))?;
        Ok(t)
    }
}

impl AacEncoder {
    /// `bitrate_bps` must be one of 96000, 128000, 160000, 192000.
    pub fn new(bitrate_bps: u32) -> Result<Self> {
        init()?;
        let info = AudioInfo {
            sample_rate: 48_000,
            channels: 2,
            bitrate_bps,
        };
        // SAFETY: standard COM activation of an in-process MFT.
        let transform: IMFTransform =
            unsafe { CoCreateInstance(&AAC_ENCODER, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| sys("create AAC encoder", e))?;
        // SAFETY: COM calls on the transform created above.
        unsafe {
            transform
                .SetOutputType(0, &aac_type(&info)?, 0)
                .map_err(|e| sys("AAC SetOutputType", e))?;
            let pcm = MFCreateMediaType().map_err(|e| sys("pcm type", e))?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| sys("pcm major", e))?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| sys("pcm subtype", e))?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(|e| sys("pcm bits", e))?;
            pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, info.sample_rate)
                .map_err(|e| sys("pcm rate", e))?;
            pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, info.channels)
                .map_err(|e| sys("pcm ch", e))?;
            pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)
                .map_err(|e| sys("pcm align", e))?;
            pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 48_000 * 4)
                .map_err(|e| sys("pcm avg", e))?;
            transform
                .SetInputType(0, &pcm, 0)
                .map_err(|e| sys("AAC SetInputType", e))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| sys("begin", e))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| sys("start", e))?;
        }
        // SAFETY: plain query on a configured transform.
        let stream =
            unsafe { transform.GetOutputStreamInfo(0) }.map_err(|e| sys("output info", e))?;
        Ok(Self {
            transform,
            info,
            output_size: stream.cbSize.max(8192),
            provides_samples: stream.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0,
        })
    }

    pub fn info(&self) -> AudioInfo {
        self.info
    }

    /// Encodes interleaved stereo samples starting at `pts` (100 ns ticks). Packets are
    /// appended to `out`.
    pub fn encode(&mut self, pcm: &[i16], pts: i64, out: &mut Vec<Packet>) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let bytes = pcm.len() * 2;
        let frames = (pcm.len() / 2) as i64;
        // SAFETY: the buffer is created, filled within its bounds and released here.
        let sample = unsafe {
            let buffer = MFCreateMemoryBuffer(bytes as u32).map_err(|e| sys("pcm buffer", e))?;
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|e| sys("lock pcm", e))?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, data, bytes);
            buffer.Unlock().map_err(|e| sys("unlock pcm", e))?;
            buffer
                .SetCurrentLength(bytes as u32)
                .map_err(|e| sys("pcm length", e))?;
            let sample = MFCreateSample().map_err(|e| sys("pcm sample", e))?;
            sample.AddBuffer(&buffer).map_err(|e| sys("add pcm", e))?;
            sample.SetSampleTime(pts).map_err(|e| sys("pcm time", e))?;
            sample
                .SetSampleDuration(frames * 10_000_000 / i64::from(self.info.sample_rate))
                .map_err(|e| sys("pcm duration", e))?;
            sample
        };
        // SAFETY: valid sample for stream 0.
        let first = unsafe { self.transform.ProcessInput(0, &sample, 0) };
        if let Err(e) = first {
            if e.code() == MF_E_NOTACCEPTING {
                self.drain(out)?;
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|e| sys("AAC input", e))?;
            } else {
                return Err(sys("AAC input", e));
            }
        }
        self.drain(out)
    }

    fn drain(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        loop {
            // SAFETY: see the video encoder; the sample is moved out exactly once.
            let sample = unsafe {
                let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
                if !self.provides_samples {
                    let buffer = MFCreateMemoryBuffer(self.output_size)
                        .map_err(|e| sys("aac out buffer", e))?;
                    let sample = MFCreateSample().map_err(|e| sys("aac out sample", e))?;
                    sample
                        .AddBuffer(&buffer)
                        .map_err(|e| sys("aac add out", e))?;
                    buffers[0].pSample = ManuallyDrop::new(Some(sample));
                }
                let mut status = 0u32;
                match self.transform.ProcessOutput(0, &mut buffers, &mut status) {
                    Ok(()) => {
                        let _events = ManuallyDrop::take(&mut buffers[0].pEvents);
                        ManuallyDrop::take(&mut buffers[0].pSample)
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        let _ = ManuallyDrop::take(&mut buffers[0].pSample);
                        let _ = ManuallyDrop::take(&mut buffers[0].pEvents);
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = ManuallyDrop::take(&mut buffers[0].pSample);
                        let _ = ManuallyDrop::take(&mut buffers[0].pEvents);
                        return Err(sys("AAC ProcessOutput", e));
                    }
                }
            };
            if let Some(sample) = sample {
                // SAFETY: lock, copy within the reported length, unlock.
                let packet = unsafe {
                    let buffer = sample
                        .ConvertToContiguousBuffer()
                        .map_err(|e| sys("aac contiguous", e))?;
                    let mut data = std::ptr::null_mut();
                    let mut len = 0u32;
                    buffer
                        .Lock(&mut data, None, Some(&mut len))
                        .map_err(|e| sys("aac lock", e))?;
                    let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
                    buffer.Unlock().map_err(|e| sys("aac unlock", e))?;
                    Packet {
                        data: bytes,
                        pts: sample.GetSampleTime().unwrap_or(0),
                        duration: sample.GetSampleDuration().unwrap_or(
                            FRAME_SAMPLES * 10_000_000 / i64::from(self.info.sample_rate),
                        ),
                        keyframe: true,
                    }
                };
                out.push(packet);
            }
        }
    }
}
