//! H.264 / HEVC encoding through Media Foundation encoder MFTs.
//!
//! Hardware MFTs (NVENC, AMF, QuickSync, registered by the GPU drivers) are
//! tried first on the capture adapter and receive D3D11 NV12 textures with
//! zero copies. The Microsoft software H.264 encoder is the fallback.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use hc_core::media::{VideoCodec, VideoInfo};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Variant::{VARIANT, VT_UI4};
use windows::core::{GUID, Interface};

use crate::bitstream::{annexb_to_avcc, normalize_param_sets, split_annexb};
use crate::mf::{collect_activates, friendly_name, init_thread, sample_bytes};

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("Media Foundation: {0}")]
    Os(#[from] windows::core::Error),
    #[error("no encoder available for {0:?}")]
    NoEncoder(VideoCodec),
    #[error("encoder did not request input in time")]
    Stalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    Cbr {
        kbps: u32,
    },
    Vbr {
        kbps: u32,
    },
    /// Constant quality, 1 (best) ..= 51.
    Cqp {
        qp: u8,
    },
}

#[derive(Debug, Clone)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub rate: Rate,
    /// Keyframe interval in frames (1 s is a good replay granularity).
    pub gop: u32,
}

#[derive(Debug, Clone)]
pub struct EncodedVideo {
    /// Length-prefixed NAL units.
    pub data: Vec<u8>,
    pub pts: i64,
    pub key: bool,
}

/// Description of an available encoder (for UI / diagnostics).
#[derive(Debug, Clone)]
pub struct EncoderDesc {
    pub name: String,
    pub hardware: bool,
    pub codec: VideoCodec,
}

fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: writing the VT_UI4 member of a zeroed VARIANT; no VariantClear needed.
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

fn subtype(codec: VideoCodec) -> GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

fn enumerate(codec: VideoCodec, hardware: bool, luid: Option<LUID>) -> Vec<IMFActivate> {
    init_thread();
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype(codec),
    };
    let flags = if hardware {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
    } else {
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    };
    let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: out-params receive a CoTaskMem array owned by collect_activates.
    let res = unsafe {
        let mut attrs = None;
        let attrs = match (luid, MFCreateAttributes(&mut attrs, 1)) {
            (Some(l), Ok(())) => attrs.inspect(|a| {
                let bytes: [u8; 8] = std::mem::transmute(l);
                let _ = a.SetBlob(&MFT_ENUM_ADAPTER_LUID, &bytes);
            }),
            _ => None,
        };
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            attrs.as_ref(),
            &mut ptr,
            &mut count,
        )
    };
    if res.is_err() {
        return Vec::new();
    }
    // SAFETY: ptr/count come straight from MFTEnum2.
    unsafe { collect_activates(ptr, count) }
}

/// List encoders usable on the given adapter (hardware first).
pub fn available_encoders(luid: Option<LUID>) -> Vec<EncoderDesc> {
    let mut out = Vec::new();
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
        for hw in [true, false] {
            for a in enumerate(codec, hw, if hw { luid } else { None }) {
                out.push(EncoderDesc {
                    name: friendly_name(&a),
                    hardware: hw,
                    codec,
                });
            }
        }
    }
    out
}

pub struct VideoEncoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    need_input: u32,
    provides_samples: bool,
    out_buffer_size: u32,
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    param_sets: Vec<Vec<u8>>,
    pub desc: EncoderDesc,
    // Keeps the D3D manager alive for the transform's lifetime.
    _manager: Option<IMFDXGIDeviceManager>,
}

impl VideoEncoder {
    /// Create the best encoder for `cfg`: hardware on `luid` first, then software.
    pub fn new(device: &ID3D11Device, luid: LUID, cfg: &VideoConfig) -> Result<Self, EncodeError> {
        init_thread();
        let mut last_err = None;
        for hw in [true, false] {
            for activate in enumerate(cfg.codec, hw, if hw { Some(luid) } else { None }) {
                let name = friendly_name(&activate);
                match Self::open(&activate, device, cfg, hw, &name) {
                    Ok(enc) => {
                        tracing::info!(encoder = %name, hardware = hw, codec = ?cfg.codec, "video encoder ready");
                        return Ok(enc);
                    }
                    Err(e) => {
                        tracing::warn!(encoder = %name, error = %e, "encoder rejected configuration");
                        // SAFETY: releases the MFT created by ActivateObject.
                        unsafe {
                            let _ = activate.ShutdownObject();
                        }
                        last_err = Some(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or(EncodeError::NoEncoder(cfg.codec)))
    }

    fn open(
        activate: &IMFActivate,
        device: &ID3D11Device,
        cfg: &VideoConfig,
        hardware: bool,
        name: &str,
    ) -> Result<Self, EncodeError> {
        // SAFETY: Media Foundation calls on interfaces we own; types are fully initialised.
        unsafe {
            let transform: IMFTransform = activate.ActivateObject()?;
            let attrs = transform.GetAttributes().ok();
            let is_async = attrs
                .as_ref()
                .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                .unwrap_or(0)
                != 0;
            if is_async && let Some(a) = &attrs {
                a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let d3d_aware = attrs
                .as_ref()
                .and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok())
                .unwrap_or(0)
                != 0;
            let mut manager = None;
            if d3d_aware {
                let mut token = 0u32;
                let mut m: Option<IMFDXGIDeviceManager> = None;
                MFCreateDXGIDeviceManager(&mut token, &mut m)?;
                if let Some(m) = m {
                    m.ResetDevice(device, token)?;
                    transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize)?;
                    manager = Some(m);
                }
            }

            let frame_size = (u64::from(cfg.width) << 32) | u64::from(cfg.height);
            let frame_rate = (u64::from(cfg.fps) << 32) | 1;
            let bitrate_kbps = match cfg.rate {
                Rate::Cbr { kbps } | Rate::Vbr { kbps } => kbps,
                Rate::Cqp { .. } => 20_000,
            };

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &subtype(cfg.codec))?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_kbps * 1000)?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            let profile = match cfg.codec {
                VideoCodec::H264 => eAVEncH264VProfile_High.0 as u32,
                VideoCodec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
                VideoCodec::Av1 => 0, // Main
            };
            if cfg.codec != VideoCodec::Av1 {
                out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, profile)?;
            }
            transform.SetOutputType(0, &out_type, 0)?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &in_type, 0)?;

            let codec_api: Option<ICodecAPI> = transform.cast().ok();
            if let Some(api) = &codec_api {
                let set = |guid: &GUID, v: u32| {
                    if let Err(e) = api.SetValue(guid, &variant_u32(v)) {
                        tracing::debug!(?guid, value = v, error = %e, "codec property not supported");
                    }
                };
                match cfg.rate {
                    Rate::Cbr { kbps } => {
                        set(
                            &CODECAPI_AVEncCommonRateControlMode,
                            eAVEncCommonRateControlMode_CBR.0 as u32,
                        );
                        set(&CODECAPI_AVEncCommonMeanBitRate, kbps * 1000);
                    }
                    Rate::Vbr { kbps } => {
                        set(
                            &CODECAPI_AVEncCommonRateControlMode,
                            eAVEncCommonRateControlMode_PeakConstrainedVBR.0 as u32,
                        );
                        set(&CODECAPI_AVEncCommonMeanBitRate, kbps * 1000);
                        set(&CODECAPI_AVEncCommonMaxBitRate, kbps * 1500);
                    }
                    Rate::Cqp { qp } => {
                        set(
                            &CODECAPI_AVEncCommonRateControlMode,
                            eAVEncCommonRateControlMode_Quality.0 as u32,
                        );
                        let quality = 100u32.saturating_sub(u32::from(qp) * 100 / 51).max(1);
                        set(&CODECAPI_AVEncCommonQuality, quality);
                    }
                }
                set(&CODECAPI_AVEncMPVGOPSize, cfg.gop.max(1));
                set(&CODECAPI_AVEncMPVDefaultBPictureCount, 0);
                set(&CODECAPI_AVLowLatencyMode, 1);
            }

            let info = transform.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0;

            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let events = if is_async {
                Some(transform.cast()?)
            } else {
                None
            };
            let mut enc = Self {
                transform,
                codec_api,
                events,
                need_input: 0,
                provides_samples,
                out_buffer_size: info.cbSize.max(cfg.width * cfg.height),
                codec: cfg.codec,
                width: cfg.width,
                height: cfg.height,
                fps: cfg.fps,
                param_sets: Vec::new(),
                desc: EncoderDesc {
                    name: name.to_owned(),
                    hardware,
                    codec: cfg.codec,
                },
                _manager: manager,
            };
            enc.read_sequence_header();
            Ok(enc)
        }
    }

    /// Stream description, available once the parameter sets are known.
    pub fn video_info(&self) -> Option<VideoInfo> {
        if self.param_sets.is_empty() {
            return None;
        }
        Some(VideoInfo {
            codec: self.codec,
            width: self.width,
            height: self.height,
            fps: self.fps,
            param_sets: self.param_sets.clone(),
        })
    }

    pub fn force_keyframe(&self) {
        if let Some(api) = &self.codec_api {
            // SAFETY: plain property set.
            let _ = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1)) };
        }
    }

    fn read_sequence_header(&mut self) {
        // SAFETY: blob query on the negotiated output type.
        unsafe {
            let Ok(t) = self.transform.GetOutputCurrentType(0) else {
                return;
            };
            let Ok(size) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
                return;
            };
            let mut blob = vec![0u8; size as usize];
            if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
                .is_ok()
            {
                let sets: Vec<Vec<u8>> = split_annexb(&blob)
                    .into_iter()
                    .map(<[u8]>::to_vec)
                    .collect();
                let sets = normalize_param_sets(self.codec, &sets);
                if !sets.is_empty() {
                    self.param_sets = sets;
                }
            }
        }
    }

    /// Submit one NV12 texture; returns any packets that became ready.
    pub fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        pts: i64,
        dur: i64,
    ) -> Result<Vec<EncodedVideo>, EncodeError> {
        let mut out = Vec::new();
        // SAFETY: sample wraps a texture on the encoder's device; MF keeps it alive.
        let sample = unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
            if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
                buffer.SetCurrentLength(b2.GetContiguousLength()?)?;
            }
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(dur)?;
            sample
        };
        if self.events.is_some() {
            let deadline = Instant::now() + Duration::from_millis(1000);
            loop {
                self.drain_events(&mut out)?;
                if self.need_input > 0 {
                    break;
                }
                if Instant::now() > deadline {
                    return Err(EncodeError::Stalled);
                }
                std::thread::sleep(Duration::from_micros(500));
            }
            self.need_input -= 1;
            // SAFETY: input requested by the async MFT.
            unsafe { self.transform.ProcessInput(0, &sample, 0)? };
            self.drain_events(&mut out)?;
        } else {
            // SAFETY: synchronous MFT contract.
            unsafe { self.transform.ProcessInput(0, &sample, 0)? };
            while let Some(p) = self.pull_output()? {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Process pending async events without blocking.
    fn drain_events(&mut self, out: &mut Vec<EncodedVideo>) -> Result<(), EncodeError> {
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        loop {
            // SAFETY: non-blocking event query.
            let ev = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(ev) => ev,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            // SAFETY: event object owned here.
            let kind = unsafe { ev.GetType()? };
            if kind == METransformNeedInput.0 as u32 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 as u32
                && let Some(p) = self.pull_output()?
            {
                out.push(p);
            }
        }
    }

    fn pull_output(&mut self) -> Result<Option<EncodedVideo>, EncodeError> {
        // SAFETY: MFT_OUTPUT_DATA_BUFFER fields are released manually below.
        unsafe {
            let provided = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample()?;
                let b = MFCreateMemoryBuffer(self.out_buffer_size)?;
                s.AddBuffer(&b)?;
                Some(s)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(provided),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let res = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let [mut b] = buffers;
            let sample = ManuallyDrop::take(&mut b.pSample);
            ManuallyDrop::drop(&mut b.pEvents);
            match res {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let t = self.transform.GetOutputAvailableType(0, 0)?;
                    self.transform.SetOutputType(0, &t, 0)?;
                    self.read_sequence_header();
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            let Some(sample) = sample else {
                return Ok(None);
            };
            let pts = sample.GetSampleTime().unwrap_or(0);
            let bytes = sample_bytes(&sample)?;
            let conv = annexb_to_avcc(self.codec, &bytes);
            if !conv.param_sets.is_empty() {
                let sets = normalize_param_sets(self.codec, &conv.param_sets);
                if !sets.is_empty() {
                    self.param_sets = sets;
                }
            } else if self.param_sets.is_empty() {
                self.read_sequence_header();
            }
            if conv.avcc.is_empty() {
                return Ok(None);
            }
            Ok(Some(EncodedVideo {
                data: conv.avcc,
                pts,
                key: conv.keyframe,
            }))
        }
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        // SAFETY: orderly shutdown of the transform.
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}
