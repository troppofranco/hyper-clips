//! AAC-LC encoding with the Windows built-in AAC encoder MFT (synchronous,
//! software, part of Media Foundation: no third-party codec).

use std::mem::ManuallyDrop;

use hc_core::media::TIMEBASE;
use windows::Win32::Media::MediaFoundation::*;

use crate::mf::{collect_activates, init_thread, sample_bytes};
use crate::mp4::aac_lc_asc;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// PCM frames per AAC frame.
pub const AAC_FRAME: i64 = 1024;

#[derive(Debug, Clone)]
pub struct EncodedAudio {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dur: i64,
}

pub struct AacEncoder {
    transform: IMFTransform,
    out_size: u32,
    pub bitrate: u32,
}

impl AacEncoder {
    /// `bitrate` in bits/s; the MFT supports 96, 128, 160 and 192 kbps.
    pub fn new(bitrate: u32) -> windows::core::Result<Self> {
        init_thread();
        let bytes_per_sec = match bitrate {
            0..=111_999 => 12_000,
            112_000..=143_999 => 16_000,
            144_000..=175_999 => 20_000,
            _ => 24_000,
        };
        // SAFETY: Media Foundation calls on objects we own.
        unsafe {
            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Audio,
                guidSubtype: MFAudioFormat_PCM,
            };
            let output = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Audio,
                guidSubtype: MFAudioFormat_AAC,
            };
            let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_AUDIO_ENCODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                Some(&output),
                &mut ptr,
                &mut count,
            )?;
            let activate = collect_activates(ptr, count)
                .into_iter()
                .next()
                .ok_or_else(windows::core::Error::empty)?;
            let transform: IMFTransform = activate.ActivateObject()?;

            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
            out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(CHANNELS))?;
            out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_sec)?;
            out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?; // raw AAC frames
            transform.SetOutputType(0, &out, 0)?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
            inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(CHANNELS))?;
            inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, u32::from(CHANNELS) * 2)?;
            inp.SetUINT32(
                &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                SAMPLE_RATE * u32::from(CHANNELS) * 2,
            )?;
            transform.SetInputType(0, &inp, 0)?;

            let info = transform.GetOutputStreamInfo(0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(Self {
                transform,
                out_size: info.cbSize.max(8192),
                bitrate: bytes_per_sec * 8,
            })
        }
    }

    pub fn asc(&self) -> Vec<u8> {
        aac_lc_asc(SAMPLE_RATE, CHANNELS)
    }

    /// Encode interleaved stereo i16 PCM starting at `pts`.
    pub fn encode(&mut self, pcm: &[i16], pts: i64) -> windows::core::Result<Vec<EncodedAudio>> {
        let frames = (pcm.len() / usize::from(CHANNELS)) as i64;
        let bytes = pcm.len() * 2;
        // SAFETY: buffer locked/unlocked around the copy; MFT contract for sync transforms.
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr().cast::<u8>(), ptr, bytes);
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(frames * TIMEBASE / i64::from(SAMPLE_RATE))?;
            self.transform.ProcessInput(0, &sample, 0)?;
        }
        let mut out = Vec::new();
        while let Some(p) = self.pull()? {
            out.push(p);
        }
        Ok(out)
    }

    fn pull(&mut self) -> windows::core::Result<Option<EncodedAudio>> {
        // SAFETY: output buffer fields released manually below.
        unsafe {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(Some(s)),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0;
            let res = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let [mut b] = buffers;
            let sample = ManuallyDrop::take(&mut b.pSample);
            ManuallyDrop::drop(&mut b.pEvents);
            match res {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) => return Err(e),
            }
            let Some(sample) = sample else {
                return Ok(None);
            };
            let data = sample_bytes(&sample)?;
            if data.is_empty() {
                return Ok(None);
            }
            Ok(Some(EncodedAudio {
                data,
                pts: sample.GetSampleTime().unwrap_or(0),
                dur: AAC_FRAME * TIMEBASE / i64::from(SAMPLE_RATE),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_sine() {
        // Windows Server images may lack Media Foundation: skip there.
        let Ok(mut enc) = AacEncoder::new(160_000) else {
            eprintln!("AAC encoder unavailable, skipping");
            return;
        };
        let mut frames = Vec::new();
        let mut pts = 0i64;
        for chunk in 0..50 {
            let pcm: Vec<i16> = (0..960)
                .flat_map(|i| {
                    let t = (chunk * 960 + i) as f32 / SAMPLE_RATE as f32;
                    let v = ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16;
                    [v, v]
                })
                .collect();
            frames.extend(enc.encode(&pcm, pts).unwrap());
            pts += 960 * TIMEBASE / i64::from(SAMPLE_RATE);
        }
        // 48_000 PCM frames -> ~46 AAC frames (encoder keeps some look-ahead).
        assert!(frames.len() >= 40, "{}", frames.len());
        assert!(frames.windows(2).all(|w| w[1].pts > w[0].pts));
        assert_eq!(enc.asc(), vec![0x11, 0x90]);
    }
}
