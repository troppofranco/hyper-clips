//! Media diagnostics: encoder listing, MP4 probing, raw video pipeline test.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hc_capture::convert::{Nv12Converter, output_size};
use hc_capture::desktop::{DesktopCapture, MonitorSelect, create_device, select_output};
use hc_core::media::{Packet, StreamSet, TIMEBASE, VideoCodec, now_ticks};
use hc_encode::mp4::Mp4Writer;
use hc_encode::video::{Rate, VideoConfig, VideoEncoder, available_encoders};

pub fn encoders() -> ExitCode {
    let out = match select_output(MonitorSelect::Primary) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("primary monitor on: {}", out.adapter_name);
    for e in available_encoders(Some(out.adapter_luid)) {
        println!(
            "  {:<5} {:<8} {}",
            format!("{:?}", e.codec),
            if e.hardware { "hardware" } else { "software" },
            e.name
        );
    }
    ExitCode::SUCCESS
}

/// Memory cost of each pipeline stage (Phase 2 RAM budget analysis).
pub fn spike_pipeline(codec: Option<&str>) -> ExitCode {
    let mem = |label: &str| {
        let m = hc_core::sys::current_memory();
        println!(
            "{label:<34} working set {:6.1} MB   private {:6.1} MB",
            m.working_set as f64 / 1048576.0,
            m.private_bytes as f64 / 1048576.0
        );
    };
    mem("baseline");
    hc_encode::mf::init_thread();
    mem("+ COM/Media Foundation");
    let Ok(out) = select_output(MonitorSelect::Primary) else {
        return ExitCode::FAILURE;
    };
    let Ok((device, context)) = create_device(out.adapter_index) else {
        return ExitCode::FAILURE;
    };
    mem("+ D3D11 device");
    let Ok(mut cap) = DesktopCapture::new(out.clone(), device.clone(), context.clone()) else {
        return ExitCode::FAILURE;
    };
    let _ = cap.acquire(100);
    mem("+ desktop duplication + 1 frame");
    let (w, h) = output_size(out.width(), out.height(), 1080);
    let Ok(mut conv) =
        Nv12Converter::new(&device, &context, out.width(), out.height(), w, h, 60, 8)
    else {
        return ExitCode::FAILURE;
    };
    mem("+ NV12 converter (8 surfaces)");
    let cfg = VideoConfig {
        codec: if codec == Some("hevc") {
            VideoCodec::Hevc
        } else {
            VideoCodec::H264
        },
        width: w,
        height: h,
        fps: 60,
        rate: Rate::Vbr { kbps: 20_000 },
        gop: 60,
    };
    let Ok(mut enc) = VideoEncoder::new(&device, out.adapter_luid, &cfg) else {
        return ExitCode::FAILURE;
    };
    mem(&format!("+ encoder ({})", enc.desc.name));
    for i in 0..120 {
        let _ = cap.acquire(0);
        if let Some(src) = cap.frame()
            && let Ok(t) = conv.convert(src)
        {
            let _ = enc.encode(&t, i * TIMEBASE / 60, TIMEBASE / 60);
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    mem("+ 2 s of encoding");
    let aac = hc_encode::aac::AacEncoder::new(160_000);
    mem("+ AAC encoder");
    drop(aac);
    ExitCode::SUCCESS
}

/// Print raw Desktop Duplication results for every monitor.
pub fn dd_diag() -> ExitCode {
    let outs = match hc_capture::desktop::list_outputs() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    for out in outs {
        println!("{} on {} {:?}", out.device_name, out.adapter_name, out.rect);
        let res = create_device(out.adapter_index)
            .and_then(|(d, c)| DesktopCapture::new(out.clone(), d, c));
        match res {
            Ok(mut cap) => {
                for i in 0..5 {
                    println!(
                        "  acquire #{i}: {:?}",
                        cap.acquire(200).map_err(|e| e.to_string())
                    );
                }
            }
            Err(e) => println!("  open failed: {e}"),
        }
    }
    ExitCode::SUCCESS
}

pub fn probe(path: &str) -> ExitCode {
    match hc_encode::probe::probe(Path::new(path)) {
        Ok(r) => {
            println!(
                "duration: {:.3} s",
                r.duration_ticks as f64 / TIMEBASE as f64
            );
            for s in r.streams {
                let span = (s.last_pts - s.first_pts) as f64 / TIMEBASE as f64;
                println!(
                    "  #{} {} {} {}x{}  samples {}  pts {:.3}..{:.3} (span {:.3} s)",
                    s.index,
                    s.major,
                    s.subtype,
                    s.width,
                    s.height,
                    s.samples,
                    s.first_pts as f64 / TIMEBASE as f64,
                    s.last_pts as f64 / TIMEBASE as f64,
                    span
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("probe failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Run the full capture service, save a clip after `secs` and record in parallel.
pub fn service_test(secs: u64, out_path: &str, mic: bool) -> ExitCode {
    use hc_recorder::{CaptureService, Event, ServiceConfig, SinkCmd};
    let mut cfg = ServiceConfig {
        capture: hc_core::config::Capture::default(),
        audio: hc_core::config::Audio::default(),
        monitor: MonitorSelect::Primary,
        min_buffer_seconds: 5,
    };
    cfg.capture.replay_seconds = 30;
    cfg.audio.microphone = mic;
    let (tx, rx) = crossbeam_channel::unbounded();
    let service = match CaptureService::start(cfg, tx) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("service failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rec_path = format!("{out_path}.session.mp4");
    std::thread::sleep(Duration::from_secs(1));
    service.command(SinkCmd::StartRecording {
        path: rec_path.clone().into(),
    });
    std::thread::sleep(Duration::from_secs(secs));
    service.command(SinkCmd::AddMarker);
    service.command(SinkCmd::SaveClip {
        path: out_path.into(),
        seconds: None,
    });
    service.screenshot(format!("{out_path}.png").into(), false);
    std::thread::sleep(Duration::from_millis(1500));
    service.command(SinkCmd::StopRecording);
    std::thread::sleep(Duration::from_millis(500));
    drop(service);
    for ev in rx.try_iter() {
        match ev {
            Event::Error(e) => println!("ERROR {e}"),
            other => println!("{other:?}"),
        }
    }
    let _ = probe(out_path);
    probe(&rec_path)
}

pub fn frame(mp4: &str, png: &str, at_secs: f64) -> ExitCode {
    let at = (at_secs * TIMEBASE as f64) as i64;
    let res = hc_encode::probe::decode_frame(Path::new(mp4), at)
        .map_err(|e| e.to_string())
        .and_then(|(w, h, stride, px)| {
            hc_capture::screenshot::save_pixels(
                w,
                h,
                stride,
                &px,
                Path::new(png),
                hc_capture::screenshot::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())
            .map(|()| (w, h))
        });
    match res {
        Ok((w, h)) => {
            println!("decoded {w}x{h} frame at {at_secs:.2}s -> {png}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn video_test(secs: u64, out_path: &str, codec: Option<&str>) -> ExitCode {
    match run_video_test(secs, out_path, codec) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("video test failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_video_test(
    secs: u64,
    out_path: &str,
    codec: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let codec = match codec {
        Some("hevc") => VideoCodec::Hevc,
        _ => VideoCodec::H264,
    };
    let fps = 60u32;
    let out = select_output(MonitorSelect::Primary)?;
    let (device, context) = create_device(out.adapter_index)?;
    let mut cap = DesktopCapture::new(out.clone(), device.clone(), context.clone())?;
    let (w, h) = output_size(out.width(), out.height(), 1080);
    let mut conv = Nv12Converter::new(&device, &context, out.width(), out.height(), w, h, fps, 8)?;
    let cfg = VideoConfig {
        codec,
        width: w,
        height: h,
        fps,
        rate: Rate::Vbr { kbps: 20_000 },
        gop: fps,
    };
    let mut enc = VideoEncoder::new(&device, out.adapter_luid, &cfg)?;
    println!(
        "capturing {} ({}x{}) -> {}x{} with {} ({})",
        out.device_name,
        out.width(),
        out.height(),
        w,
        h,
        enc.desc.name,
        if enc.desc.hardware {
            "hardware"
        } else {
            "software"
        }
    );
    let frame_dur = TIMEBASE / i64::from(fps);
    let mut packets = Vec::new();
    let start = now_ticks();
    let wall = Instant::now();
    let mut frame = 0i64;
    let mut encode_time = Duration::ZERO;
    while wall.elapsed() < Duration::from_secs(secs) {
        let due = start + frame * frame_dur;
        let wait = due - now_ticks();
        if wait > 0 {
            std::thread::sleep(Duration::from_nanos((wait * 100) as u64));
        }
        let _ = cap.acquire(if cap.frame().is_none() { 50 } else { 0 });
        if let Some(src) = cap.frame() {
            let t0 = Instant::now();
            let tex = conv.convert(src)?;
            for p in enc.encode(&tex, due, frame_dur)? {
                packets.push(p);
            }
            encode_time += t0.elapsed();
        }
        frame += 1;
    }
    println!(
        "frames submitted {frame}, packets {}, avg submit {:.2} ms",
        packets.len(),
        encode_time.as_secs_f64() * 1000.0 / frame.max(1) as f64
    );
    let info = enc
        .video_info()
        .ok_or("encoder produced no parameter sets")?;
    let file = std::fs::File::create(out_path)?;
    let mut mux = Mp4Writer::new(
        std::io::BufWriter::new(file),
        StreamSet {
            video: info,
            audio: Vec::new(),
        },
    )?;
    let keys = packets.iter().filter(|p| p.key).count();
    let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
    let first_key = packets.iter().position(|p| p.key).ok_or("no keyframe")?;
    for p in &packets[first_key..] {
        mux.write(&Packet {
            track: 0,
            pts: p.pts,
            dur: frame_dur,
            key: p.key,
            data: Arc::from(p.data.as_slice()),
        })?;
    }
    mux.finish()?;
    println!(
        "keyframes {keys}, {:.1} MB, {:.1} Mbps",
        bytes as f64 / 1e6,
        bytes as f64 * 8.0 / 1e6 / secs as f64
    );
    println!("wrote {out_path}");
    Ok(())
}
