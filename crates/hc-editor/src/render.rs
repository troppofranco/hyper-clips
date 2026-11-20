//! Export engine: decode → compose (GPU) → encode → mux.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hc_capture::compose::{Compositor, DrawMode, Overlay, RenderTarget};
use hc_capture::convert::{ConvertParams, Nv12Converter};
use hc_capture::desktop::{adapter_luid, create_device};
use hc_core::media::{AudioInfo, AudioRole, Packet, StreamSet, TIMEBASE, VideoCodec};
use hc_encode::aac::{AacEncoder, CHANNELS, SAMPLE_RATE};
use hc_encode::mp4::Mp4Writer;
use hc_encode::video::{Rate, VideoConfig, VideoEncoder};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};

use crate::anim::{GifWriter, WebpAnimWriter};
use crate::decode::{
    RATE, VideoReader, audio_stream_count, decode_audio, load_overlay_image, resample, video_size,
};
use crate::project::{Format, OverlayKind, Project, TransitionKind, VideoCodecChoice};
use crate::webm::WebmWriter;

const MAX_PASSES: u32 = 3;

#[derive(Debug, Clone)]
pub struct ExportReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub duration_ms: u64,
    pub passes: u32,
    pub width: u32,
    pub height: u32,
    pub encoder: String,
}

/// Output size for the project (even dimensions).
pub fn output_size(p: &Project) -> Result<(u32, u32), String> {
    let first = p.segments.first().ok_or("the project has no clips")?;
    let (sw, sh) = video_size(&first.path).ok_or("cannot read the source video size")?;
    let ratio = p
        .framing
        .aspect
        .ratio()
        .unwrap_or(f64::from(sw) / f64::from(sh.max(1)));
    let mut h = if p.output.height == 0 {
        sh
    } else {
        p.output.height
    };
    if matches!(p.output.format, Format::Gif | Format::WebP) {
        h = h.min(720);
    }
    let w = (f64::from(h) * ratio).round() as u32;
    Ok((w.max(16) & !1, h.max(16) & !1))
}

/// Export `project` to `out`. `progress(fraction 0..1, pass)`.
pub fn export(
    project: &Project,
    out: &Path,
    progress: &mut dyn FnMut(f32, u32),
    cancel: &AtomicBool,
) -> Result<ExportReport, String> {
    hc_encode::mf::init_thread();
    if project.segments.is_empty() || project.duration_ms() == 0 {
        return Err("nothing to export".into());
    }
    let (w, h) = output_size(project)?;
    let audio = build_audio(project)?;
    let dur_s = project.duration_ms() as f64 / 1000.0;
    let mut kbps = project.output.bitrate_kbps.max(300);
    let target = project
        .output
        .target_size_mb
        .filter(|_| matches!(project.output.format, Format::Mp4 | Format::WebM))
        .map(|mb| (mb * 1024.0 * 1024.0) as u64);
    if let Some(t) = target {
        // Leave ~4 % for container overhead.
        let audio_bits = audio
            .as_ref()
            .map_or(0.0, |a| a.tracks.len() as f64 * 160_000.0 * dur_s);
        let video_bits = (t as f64 * 8.0 * 0.96 - audio_bits).max(dur_s * 300_000.0);
        kbps = kbps.min((video_bits / dur_s / 1000.0) as u32).max(300);
    }
    let tmp = out.with_extension(format!("{}.part", project.output.format.extension()));
    let mut passes = 0;
    let mut encoder_name;
    loop {
        passes += 1;
        encoder_name = encode_pass(
            project,
            &tmp,
            (w, h),
            kbps,
            audio.as_ref(),
            &mut |f| progress(f, passes),
            cancel,
        )?;
        let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        match target {
            Some(t) if size > t && passes < MAX_PASSES => {
                let ratio = t as f64 / size as f64;
                kbps = ((f64::from(kbps) * ratio * 0.93) as u32).max(250);
                tracing::info!(
                    size,
                    target = t,
                    new_kbps = kbps,
                    "over target size, re-encoding"
                );
            }
            _ => break,
        }
    }
    if out.exists() {
        let _ = std::fs::remove_file(out);
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if let Some(t) = target
        && bytes > t
    {
        tracing::warn!(bytes, target = t, "could not reach the target size");
    }
    Ok(ExportReport {
        path: out.to_path_buf(),
        bytes,
        duration_ms: project.duration_ms(),
        passes,
        width: w,
        height: h,
        encoder: encoder_name,
    })
}

struct AudioOut {
    infos: Vec<AudioInfo>,
    tracks: Vec<Vec<i16>>,
}

/// Mix (and optionally keep separate) the selected source audio tracks
/// along the output timeline, with speed changes and transitions.
fn build_audio(p: &Project) -> Result<Option<AudioOut>, String> {
    if !p.output.audio || p.output.format != Format::Mp4 {
        return Ok(None);
    }
    let n = p
        .segments
        .iter()
        .map(|s| audio_stream_count(&s.path))
        .max()
        .unwrap_or(0);
    if n == 0 {
        return Ok(None);
    }
    // Default: only track 0 (the recorded mix) is used.
    let mix_of = |k: usize| -> (f32, bool) {
        match p.tracks.get(k) {
            Some(t) => (t.volume, !t.muted),
            None => (1.0, k == 0),
        }
    };
    let total = (p.duration_ms() as usize * RATE / 1000) * 2;
    let selected: Vec<usize> = (0..n).filter(|&k| mix_of(k).1).collect();
    let mut mix = vec![0f32; total];
    let mut stems: Vec<Vec<f32>> = if p.output.separate_audio {
        vec![vec![0f32; total]; selected.len()]
    } else {
        vec![]
    };
    let starts = p.segment_starts();
    let trans = p.transition.duration_ms as f64;
    let count = p.segments.len();
    for (i, seg) in p.segments.iter().enumerate() {
        let len_ms = seg.output_ms() as f64;
        let envelope = |t_ms: f64| -> f32 {
            match p.transition.kind {
                TransitionKind::Cut => 1.0,
                TransitionKind::Crossfade => {
                    let mut g = 1.0;
                    if i > 0 && t_ms < trans {
                        g *= t_ms / trans;
                    }
                    if i + 1 < count && len_ms - t_ms < trans {
                        g *= (len_ms - t_ms) / trans;
                    }
                    g as f32
                }
                TransitionKind::FadeBlack => {
                    let edge = t_ms.min(len_ms - t_ms);
                    ((edge / (trans / 2.0)).clamp(0.0, 1.0)) as f32
                }
            }
        };
        for (si, &k) in selected.iter().enumerate() {
            let (vol, _) = mix_of(k);
            let mut pcm =
                decode_audio(&seg.path, k, seg.in_ms, seg.out_ms).map_err(|e| e.to_string())?;
            if (seg.speed - 1.0).abs() > 0.01 {
                pcm = resample(&pcm, f64::from(seg.speed));
            }
            let offset = (starts[i] as usize * RATE / 1000) * 2;
            for (j, s) in pcm.iter().enumerate() {
                let idx = offset + j;
                if idx >= total {
                    break;
                }
                let t_ms = (j / 2) as f64 * 1000.0 / RATE as f64;
                let v = f32::from(*s) * vol * envelope(t_ms);
                mix[idx] += v;
                if let Some(stem) = stems.get_mut(si) {
                    stem[idx] += v;
                }
            }
        }
    }
    let to_i16 = |b: Vec<f32>| -> Vec<i16> {
        b.into_iter()
            .map(|v| v.clamp(-32768.0, 32767.0) as i16)
            .collect()
    };
    let mut tracks = vec![to_i16(mix)];
    let mut roles = vec![AudioRole::Mix];
    for (si, stem) in stems.into_iter().enumerate() {
        tracks.push(to_i16(stem));
        roles.push(AudioRole::Track(selected[si] as u8));
    }
    let asc = hc_encode::mp4::aac_lc_asc(SAMPLE_RATE, CHANNELS);
    let infos = roles
        .into_iter()
        .map(|role| AudioInfo {
            role,
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            asc: asc.clone(),
            bitrate: 160_000,
        })
        .collect();
    Ok(Some(AudioOut { infos, tracks }))
}

fn encode_aac(pcm: &[i16], track: u8) -> Result<Vec<Packet>, String> {
    let mut enc = AacEncoder::new(160_000).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let block = 1024 * 2;
    for (b, chunk) in pcm.chunks(block).enumerate() {
        let mut buf = chunk.to_vec();
        buf.resize(block, 0);
        let pts = (b as i64) * 1024 * TIMEBASE / i64::from(SAMPLE_RATE);
        for f in enc.encode(&buf, pts).map_err(|e| e.to_string())? {
            out.push(Packet {
                track,
                pts: f.pts,
                dur: f.dur,
                key: true,
                data: Arc::from(f.data.into_boxed_slice()),
            });
        }
    }
    Ok(out)
}

struct Overlays {
    items: Vec<(Overlay, f32, f32, f32, f32, u64, u64)>, // tex, x, y, w, h (px), start, end
}

fn prepare_overlays(
    p: &Project,
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    (w, h): (u32, u32),
) -> Overlays {
    let mut items = Vec::new();
    for o in &p.overlays {
        let (tex, ow, oh) = match &o.kind {
            OverlayKind::Text {
                text,
                size,
                color,
                outline,
            } => {
                let px = (size * h as f32).max(8.0) as u32;
                let Some(img) = crate::text::render(text, px, *color, *outline) else {
                    continue;
                };
                let Ok(ov) = Overlay::new(device, img.width, img.height) else {
                    continue;
                };
                ov.upload(ctx, &img.bgra);
                (ov, img.width as f32, img.height as f32)
            }
            OverlayKind::Image { path, width } => {
                let Some(ov) = load_overlay_image(device, ctx, path) else {
                    continue;
                };
                let ow = width * w as f32;
                let oh = ow * ov.height as f32 / ov.width.max(1) as f32;
                (ov, ow, oh)
            }
        };
        let cx = o.x * w as f32;
        let cy = o.y * h as f32;
        items.push((
            tex,
            cx - ow / 2.0,
            cy - oh / 2.0,
            ow,
            oh,
            o.start_ms,
            o.end_ms,
        ));
    }
    Overlays { items }
}

fn readback(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    staging: &mut Option<ID3D11Texture2D>,
) -> Option<Vec<u8>> {
    // SAFETY: staging copy + map/unmap on our device.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        if staging.is_none() {
            let sd = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut st = None;
            device.CreateTexture2D(&sd, None, Some(&mut st)).ok()?;
            *staging = st;
        }
        let st = staging.as_ref()?;
        ctx.CopyResource(st, tex);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
        let row = desc.Width as usize * 4;
        let mut out = vec![0u8; row * desc.Height as usize];
        for y in 0..desc.Height as usize {
            let src = std::slice::from_raw_parts(
                (m.pData as *const u8).add(y * m.RowPitch as usize),
                row,
            );
            out[y * row..(y + 1) * row].copy_from_slice(src);
        }
        ctx.Unmap(st, 0);
        Some(out)
    }
}

enum Sink {
    Mp4 {
        writer: Option<Mp4Writer<BufWriter<File>>>,
        file: Option<File>,
        pending: Vec<Packet>,
    },
    WebM {
        writer: Option<WebmWriter<BufWriter<File>>>,
        file: Option<File>,
        pending: Vec<Packet>,
    },
    Gif(GifWriter<BufWriter<File>>),
    WebP(WebpAnimWriter, PathBuf),
}

#[allow(clippy::too_many_arguments)]
fn encode_pass(
    p: &Project,
    path: &Path,
    (w, h): (u32, u32),
    kbps: u32,
    audio: Option<&AudioOut>,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<String, String> {
    let (device, ctx) = create_device(0).map_err(|e| e.to_string())?;
    let luid = adapter_luid(0).map_err(|e| e.to_string())?;
    let comp = Compositor::new(&device, &ctx).map_err(|e| e.to_string())?;
    let target = RenderTarget::new(&device, w, h).map_err(|e| e.to_string())?;
    let overlays = prepare_overlays(p, &device, &ctx, (w, h));
    let fps = p.output.fps.clamp(5, 120);
    let frame_ticks = TIMEBASE / i64::from(fps);
    let frames = (p.duration_ms() * u64::from(fps) / 1000).max(1);
    let file = File::create(path).map_err(|e| e.to_string())?;

    let video_codec = match (p.output.format, p.output.codec) {
        (Format::WebM, _) | (_, VideoCodecChoice::Av1) => Some(VideoCodec::Av1),
        (Format::Mp4, VideoCodecChoice::Hevc) => Some(VideoCodec::Hevc),
        (Format::Mp4, VideoCodecChoice::H264) => Some(VideoCodec::H264),
        _ => None,
    };
    let mut encoder = None;
    let mut conv = None;
    let mut sink = match p.output.format {
        Format::Mp4 => Sink::Mp4 {
            writer: None,
            file: Some(file),
            pending: Vec::new(),
        },
        Format::WebM => Sink::WebM {
            writer: None,
            file: Some(file),
            pending: Vec::new(),
        },
        Format::Gif => Sink::Gif(GifWriter::new(
            BufWriter::new(file),
            w as u16,
            h as u16,
            fps,
        )?),
        Format::WebP => {
            drop(file);
            Sink::WebP(WebpAnimWriter::new(w, h, fps), path.to_path_buf())
        }
    };
    if let Some(codec) = video_codec {
        conv = Some(
            Nv12Converter::new(
                &device,
                &ctx,
                &ConvertParams {
                    in_width: w,
                    in_height: h,
                    crop: None,
                    out_width: w,
                    out_height: h,
                    fps,
                    pool_size: 4,
                },
            )
            .map_err(|e| e.to_string())?,
        );
        let cfg = VideoConfig {
            codec,
            width: w,
            height: h,
            fps,
            rate: Rate::Vbr { kbps },
            gop: fps * 2,
        };
        encoder = Some(VideoEncoder::new(&device, luid, &cfg).map_err(|e| {
            if codec == VideoCodec::Av1 {
                format!("no AV1 encoder on this GPU (needed for WebM/AV1): {e}")
            } else {
                e.to_string()
            }
        })?);
    }
    let encoder_name = encoder
        .as_ref()
        .map(|e| e.desc.name.clone())
        .unwrap_or_else(|| "CPU".into());

    let starts = p.segment_starts();
    let mut readers: Vec<Option<VideoReader>> = (0..p.segments.len()).map(|_| None).collect();
    let trans = p.transition.duration_ms as f64;
    let mut staging = None;
    let last_real_pts = (frames as i64 - 1) * frame_ticks;
    let mut got_last = false;
    let mut extra = 0u64;

    let handle_packet = |sink: &mut Sink,
                         enc: &VideoEncoder,
                         pk: hc_encode::video::EncodedVideo|
     -> Result<(), String> {
        if pk.pts > last_real_pts {
            return Ok(());
        }
        let packet = Packet {
            track: 0,
            pts: pk.pts,
            dur: frame_ticks,
            key: pk.key,
            data: Arc::from(pk.data.into_boxed_slice()),
        };
        match sink {
            Sink::Mp4 {
                writer,
                file,
                pending,
            } => {
                if writer.is_none()
                    && let Some(info) = enc.video_info()
                    && let Some(f) = file.take()
                {
                    let streams = StreamSet {
                        video: info,
                        audio: audio.map(|a| a.infos.clone()).unwrap_or_default(),
                    };
                    let mut wr = Mp4Writer::new(BufWriter::with_capacity(1 << 20, f), streams)
                        .map_err(|e| e.to_string())?;
                    for q in pending.drain(..) {
                        wr.write(&q).map_err(|e| e.to_string())?;
                    }
                    *writer = Some(wr);
                }
                match writer {
                    Some(wr) => wr.write(&packet).map_err(|e| e.to_string())?,
                    None => pending.push(packet),
                }
            }
            Sink::WebM {
                writer,
                file,
                pending,
            } => {
                if writer.is_none()
                    && let Some(info) = enc.video_info()
                    && let Some(seq) = info.param_sets.first()
                    && let Some(f) = file.take()
                {
                    let mut cp = hc_encode::bitstream::av1c_header(seq).to_vec();
                    cp.extend_from_slice(seq);
                    let mut wr = WebmWriter::new(BufWriter::with_capacity(1 << 20, f), w, h, &cp)
                        .map_err(|e| e.to_string())?;
                    for q in pending.drain(..) {
                        wr.write((q.pts / 10_000) as u64, q.key, &q.data)
                            .map_err(|e| e.to_string())?;
                    }
                    *writer = Some(wr);
                }
                match writer {
                    Some(wr) => wr
                        .write((packet.pts / 10_000) as u64, packet.key, &packet.data)
                        .map_err(|e| e.to_string())?,
                    None => pending.push(packet),
                }
            }
            _ => {}
        }
        Ok(())
    };

    let mut f = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(sink);
            let _ = std::fs::remove_file(path);
            return Err("cancelled".into());
        }
        let real = f < frames;
        if real {
            let t = f as f64 * 1000.0 / f64::from(fps);
            comp.clear(&target);
            for (i, seg) in p.segments.iter().enumerate() {
                let s0 = starts[i] as f64;
                let len = seg.output_ms() as f64;
                if t < s0 || t >= s0 + len {
                    if t >= s0 + len {
                        readers[i] = None; // free decoder memory once done
                    }
                    continue;
                }
                let local = t - s0;
                let opacity = match p.transition.kind {
                    TransitionKind::Cut => 1.0,
                    TransitionKind::Crossfade if i > 0 && local < trans => (local / trans) as f32,
                    TransitionKind::Crossfade => 1.0,
                    TransitionKind::FadeBlack => {
                        ((local.min(len - local)) / (trans / 2.0)).clamp(0.0, 1.0) as f32
                    }
                };
                if readers[i].is_none() {
                    readers[i] = Some(
                        VideoReader::open(&seg.path, &device, &ctx, seg.in_ms)
                            .map_err(|e| format!("{}: {e}", seg.path.display()))?,
                    );
                }
                let Some(r) = readers[i].as_mut() else {
                    continue;
                };
                let (rw, rh) = (r.width, r.height);
                let src_t = seg.in_ms as f64 + local * f64::from(seg.speed);
                if let Some(srv) = r.frame_at(src_t) {
                    let uv = p.framing.source_uv(rw, rh);
                    comp.draw_uv(
                        &target,
                        srv,
                        [0.0, 0.0, w as f32, h as f32],
                        uv,
                        DrawMode::Video { opacity },
                    );
                }
            }
            for (tex, x, y, ow, oh, s, e) in &overlays.items {
                if (*s as f64) <= t && t < *e as f64 {
                    comp.draw(
                        &target,
                        &tex.srv,
                        [*x, *y, x + ow, y + oh],
                        DrawMode::Blend { opacity: 1.0 },
                    );
                }
            }
        }
        match (&mut sink, encoder.as_mut(), conv.as_mut()) {
            (Sink::Gif(g), _, _) if real => {
                let px = readback(&device, &ctx, &target.texture, &mut staging)
                    .ok_or("readback failed")?;
                g.add(w as u16, h as u16, &px)?;
            }
            (Sink::WebP(wp, _), _, _) if real => {
                let px = readback(&device, &ctx, &target.texture, &mut staging)
                    .ok_or("readback failed")?;
                wp.add(&px)?;
            }
            (Sink::Mp4 { .. } | Sink::WebM { .. }, Some(enc), Some(cv)) => {
                // After the last real frame keep feeding it until the encoder
                // has returned every real frame (hardware encoders lag).
                let nv12 = cv.convert(&target.texture).map_err(|e| e.to_string())?;
                let pts = f as i64 * frame_ticks;
                for pk in enc
                    .encode(&nv12, pts, frame_ticks)
                    .map_err(|e| e.to_string())?
                {
                    if pk.pts >= last_real_pts {
                        got_last = true;
                    }
                    handle_packet(&mut sink, enc, pk)?;
                }
            }
            _ => {}
        }
        if real {
            progress((f + 1) as f32 / frames as f32);
        }
        f += 1;
        if !real {
            extra += 1;
        }
        let video = matches!(sink, Sink::Mp4 { .. } | Sink::WebM { .. });
        if f >= frames && (!video || got_last || extra > 120) {
            break;
        }
    }
    // Finish the container.
    match sink {
        Sink::Mp4 { writer, .. } => {
            let mut wr = writer.ok_or("the encoder produced no video")?;
            if let Some(a) = audio {
                for (k, pcm) in a.tracks.iter().enumerate() {
                    for pk in encode_aac(pcm, (k + 1) as u8)? {
                        wr.write(&pk).map_err(|e| e.to_string())?;
                    }
                }
            }
            wr.finish().map_err(|e| e.to_string())?;
        }
        Sink::WebM { writer, .. } => {
            let wr = writer.ok_or("the encoder produced no video")?;
            wr.finish(1000 / u64::from(fps))
                .map_err(|e| e.to_string())?;
        }
        Sink::Gif(g) => drop(g),
        Sink::WebP(wp, path) => {
            let f = File::create(&path).map_err(|e| e.to_string())?;
            wp.finish(BufWriter::new(f)).map_err(|e| e.to_string())?;
        }
    }
    Ok(encoder_name)
}
