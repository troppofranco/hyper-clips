//! H.264 encoder on top of an `IMFTransform`.
//!
//! Phase 2 uses Microsoft's software H.264 encoder (synchronous MFT, works on every PC).
//! Hardware MFTs (NVENC / AMF / QuickSync) are asynchronous and come in the next step.

use crate::annexb;
use crate::convert::{bgra_to_nv12, nv12_size};
use crate::mf::{h264_type, set_pair, sys};
use crate::VideoInfo;
use hc_buffer::Packet;
use hc_core::error::{Error, Result};
use std::mem::ManuallyDrop;
use windows::core::{Interface, GUID};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VARIANT;

/// CLSID_MSH264EncoderMFT, spelled out so the code does not depend on the constant's name.
const MS_H264_ENCODER: GUID = GUID::from_u128(0x6ca50344_051a_4ded_9779_a43305165e35);

pub struct H264Encoder {
    transform: IMFTransform,
    info: VideoInfo,
    frame_duration: i64,
    provides_samples: bool,
    output_size: u32,
    nv12: Vec<u8>,
}

impl H264Encoder {
    /// `width`/`height` must be even. `gop_frames` is the keyframe interval.
    pub fn new_software(
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
        gop_frames: u32,
    ) -> Result<Self> {
        crate::mf::init()?;
        let mut info = VideoInfo {
            width,
            height,
            fps,
            bitrate_bps,
            sequence_header: Vec::new(),
        };

        // SAFETY: standard COM activation of an in-process MFT.
        let transform: IMFTransform =
            unsafe { CoCreateInstance(&MS_H264_ENCODER, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| sys("create H.264 encoder", e))?;

        // SAFETY: COM calls on the transform created above; media types are built here.
        unsafe {
            // Best-effort tuning: low latency, no B-frames, regular keyframes.
            if let Ok(api) = transform.cast::<ICodecAPI>() {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
                let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(0u32));
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(gop_frames));
            }

            // The output type must be set before the input type for this encoder.
            let out_type = h264_type(&info, &[])?;
            out_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
                .map_err(|e| sys("profile", e))?;
            transform
                .SetOutputType(0, &out_type, 0)
                .map_err(|e| sys("SetOutputType", e))?;

            let in_type = MFCreateMediaType().map_err(|e| sys("input type", e))?;
            let in_attrs: &IMFAttributes = &in_type;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| sys("in major", e))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| sys("in subtype", e))?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| sys("in interlace", e))?;
            set_pair(&in_attrs, &MF_MT_FRAME_SIZE, width, height)?;
            set_pair(&in_attrs, &MF_MT_FRAME_RATE, fps, 1)?;
            set_pair(&in_attrs, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            transform
                .SetInputType(0, &in_type, 0)
                .map_err(|e| sys("SetInputType", e))?;

            // The sequence header (SPS/PPS) may already be exposed on the output type.
            if let Ok(current) = transform.GetOutputCurrentType(0) {
                if let Ok(len) = current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    let mut buf = vec![0u8; len as usize];
                    if current
                        .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut buf, None)
                        .is_ok()
                    {
                        info.sequence_header = buf;
                    }
                }
            }

            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| sys("begin streaming", e))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| sys("start of stream", e))?;
        }

        // SAFETY: plain query on a configured transform.
        let stream = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|e| sys("GetOutputStreamInfo", e))?;
        let provides_samples = stream.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;

        Ok(Self {
            transform,
            info,
            frame_duration: 10_000_000 / i64::from(fps.max(1)),
            provides_samples,
            output_size: stream.cbSize.max(1 << 20),
            nv12: Vec::with_capacity(nv12_size(width, height)),
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Encodes one BGRA frame (any size >= the configured one is cropped; smaller is rejected).
    /// New packets are appended to `out`.
    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        src_width: u32,
        src_height: u32,
        pts: i64,
        out: &mut Vec<Packet>,
    ) -> Result<()> {
        if src_width < self.info.width || src_height < self.info.height {
            return Err(Error::Invalid(
                "frame is smaller than the encoder size".into(),
            ));
        }
        let mut nv12 = std::mem::take(&mut self.nv12);
        let (w, h) = bgra_to_nv12(bgra, src_width, src_height, &mut nv12);
        // Source larger than the encoder size: the converter cropped only to even size,
        // so a mismatch here means the capture size changed; the caller must restart.
        if (w, h) != (self.info.width, self.info.height) {
            self.nv12 = nv12;
            return Err(Error::Invalid(
                "capture size changed; encoder must be recreated".into(),
            ));
        }
        let result = self.feed(&nv12, pts, out);
        self.nv12 = nv12;
        result
    }

    fn feed(&mut self, nv12: &[u8], pts: i64, out: &mut Vec<Packet>) -> Result<()> {
        // SAFETY: the memory buffer is created, filled within its bounds and released here.
        let sample = unsafe {
            let buffer =
                MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| sys("input buffer", e))?;
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|e| sys("lock input", e))?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
            buffer.Unlock().map_err(|e| sys("unlock input", e))?;
            buffer
                .SetCurrentLength(nv12.len() as u32)
                .map_err(|e| sys("input length", e))?;
            let sample = MFCreateSample().map_err(|e| sys("input sample", e))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| sys("add buffer", e))?;
            sample
                .SetSampleTime(pts)
                .map_err(|e| sys("sample time", e))?;
            sample
                .SetSampleDuration(self.frame_duration)
                .map_err(|e| sys("sample duration", e))?;
            sample
        };

        // SAFETY: valid sample for stream 0.
        let first = unsafe { self.transform.ProcessInput(0, &sample, 0) };
        if let Err(e) = first {
            if e.code() == MF_E_NOTACCEPTING {
                // Output is pending: drain it, then submit again.
                self.drain(out)?;
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|e| sys("ProcessInput", e))?;
            } else {
                return Err(sys("ProcessInput", e));
            }
        }
        self.drain(out)
    }

    /// Pulls every packet the encoder has ready.
    fn drain(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        loop {
            // SAFETY: the output buffer array and status are valid for the call; the sample
            // inside is moved out with `ManuallyDrop::take` exactly once.
            let sample = unsafe {
                let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
                if !self.provides_samples {
                    let buffer = MFCreateMemoryBuffer(self.output_size)
                        .map_err(|e| sys("output buffer", e))?;
                    let sample = MFCreateSample().map_err(|e| sys("output sample", e))?;
                    sample
                        .AddBuffer(&buffer)
                        .map_err(|e| sys("add output", e))?;
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
                        return Err(sys("ProcessOutput", e));
                    }
                }
            };
            if let Some(sample) = sample {
                let packet = self.sample_to_packet(&sample)?;
                if self.info.sequence_header.is_empty() && packet.keyframe {
                    self.info.sequence_header = annexb::sequence_header(&packet.data);
                }
                out.push(packet);
            }
        }
    }

    fn sample_to_packet(&self, sample: &IMFSample) -> Result<Packet> {
        // SAFETY: the buffer is locked, copied within its reported length and unlocked.
        unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| sys("contiguous buffer", e))?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut data, None, Some(&mut len))
                .map_err(|e| sys("lock output", e))?;
            let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
            buffer.Unlock().map_err(|e| sys("unlock output", e))?;

            let pts = sample.GetSampleTime().map_err(|e| sys("output time", e))?;
            let duration = sample.GetSampleDuration().unwrap_or(self.frame_duration);
            let clean = sample
                .GetUINT32(&MFSampleExtension_CleanPoint)
                .map(|v| v != 0)
                .unwrap_or(false);
            let keyframe = clean || annexb::has_idr(&bytes);
            Ok(Packet {
                data: bytes,
                pts,
                duration,
                keyframe,
            })
        }
    }
}
