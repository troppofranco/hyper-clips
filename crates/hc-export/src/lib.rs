//! Clip editing through an external `ffmpeg` executable (not bundled: it is a large, separately
//! licensed program). This crate only builds argument lists (no shell, so no injection) and runs them.

use hc_core::error::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Webm,
    Gif,
    Webp,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mp4" => Some(Self::Mp4),
            "webm" => Some(Self::Webm),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crop {
    None,
    /// Centre crop to 9:16 (vertical video).
    Vertical9x16,
    /// Centre crop to 1:1.
    Square1x1,
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: Format,
    /// Keep only `(start, end)` seconds.
    pub range: Option<(f64, f64)>,
    /// 1.0 = normal. Allowed 0.25..=4.0.
    pub speed: f64,
    pub crop: Crop,
    /// Output width in pixels (height follows); None = keep.
    pub width: Option<u32>,
    /// Frame rate for GIF / WebP.
    pub fps: u32,
    /// Drop container metadata (privacy).
    pub strip_metadata: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: Format::Mp4,
            range: None,
            speed: 1.0,
            crop: Crop::None,
            width: None,
            fps: 15,
            strip_metadata: true,
        }
    }
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// `atempo` only accepts 0.5..=2.0 per filter, so larger changes are chained.
fn atempo_chain(speed: f64) -> String {
    let mut parts = Vec::new();
    let mut s = speed;
    while s > 2.0 {
        parts.push("atempo=2.0".to_owned());
        s /= 2.0;
    }
    while s < 0.5 {
        parts.push("atempo=0.5".to_owned());
        s /= 0.5;
    }
    parts.push(format!("atempo={s:.4}"));
    parts.join(",")
}

/// Builds the ffmpeg arguments for one export. Fails on invalid options.
pub fn build_args(input: &Path, output: &Path, o: &ExportOptions) -> Result<Vec<OsString>> {
    if !(0.25..=4.0).contains(&o.speed) {
        return Err(Error::Invalid("speed must be between 0.25 and 4".into()));
    }
    if let Some((a, b)) = o.range {
        if a < 0.0 || b <= a {
            return Err(Error::Invalid(
                "trim range must satisfy 0 <= start < end".into(),
            ));
        }
    }
    let audio = matches!(o.format, Format::Mp4 | Format::Webm);
    let mut video_filters: Vec<String> = Vec::new();
    match o.crop {
        Crop::None => {}
        Crop::Vertical9x16 => video_filters.push("crop=trunc(ih*9/16/2)*2:ih".into()),
        Crop::Square1x1 => video_filters.push("crop=min(iw\\,ih):min(iw\\,ih)".into()),
    }
    if (o.speed - 1.0).abs() > f64::EPSILON {
        video_filters.push(format!("setpts=PTS/{:.4}", o.speed));
    }
    if matches!(o.format, Format::Gif | Format::Webp) {
        video_filters.push(format!("fps={}", o.fps.clamp(1, 60)));
    }
    if let Some(w) = o.width {
        video_filters.push(format!("scale={}:-2:flags=lanczos", w.max(16) & !1));
    }
    if o.format == Format::Gif {
        video_filters.push("split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse".into());
    }
    let audio_filters = if audio && (o.speed - 1.0).abs() > f64::EPSILON {
        Some(atempo_chain(o.speed))
    } else {
        None
    };

    let mut a: Vec<OsString> = vec![os("-hide_banner"), os("-y"), os("-nostdin")];
    if let Some((start, _)) = o.range {
        a.push(os("-ss"));
        a.push(format!("{start:.3}").into());
    }
    a.push(os("-i"));
    a.push(input.as_os_str().to_owned());
    if let Some((start, end)) = o.range {
        a.push(os("-t"));
        a.push(format!("{:.3}", end - start).into());
    }

    // A plain trim of an MP4 needs no re-encoding.
    let copy_only = o.format == Format::Mp4 && video_filters.is_empty() && audio_filters.is_none();
    if copy_only {
        a.extend([os("-c"), os("copy")]);
    } else {
        if !video_filters.is_empty() {
            a.push(os("-vf"));
            a.push(video_filters.join(",").into());
        }
        if let Some(af) = &audio_filters {
            a.push(os("-af"));
            a.push(af.clone().into());
        }
        match o.format {
            Format::Mp4 => a.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "23",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-movflags",
                    "+faststart",
                ]
                .map(os),
            ),
            Format::Webm => a.extend(
                [
                    "-c:v",
                    "libvpx-vp9",
                    "-crf",
                    "32",
                    "-b:v",
                    "0",
                    "-c:a",
                    "libopus",
                ]
                .map(os),
            ),
            Format::Gif => a.extend(["-an", "-loop", "0"].map(os)),
            Format::Webp => {
                a.extend(["-an", "-c:v", "libwebp", "-loop", "0", "-q:v", "70"].map(os))
            }
        }
    }
    if o.strip_metadata {
        a.extend(["-map_metadata", "-1", "-fflags", "+bitexact"].map(os));
    }
    a.push(output.as_os_str().to_owned());
    Ok(a)
}

/// Contents of the list file for ffmpeg's concat demuxer.
pub fn concat_list(inputs: &[PathBuf]) -> String {
    inputs
        .iter()
        .map(|p| {
            let path = p
                .to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''");
            format!("file '{path}'\n")
        })
        .collect()
}

pub fn concat_args(list_file: &Path, output: &Path) -> Vec<OsString> {
    vec![
        os("-hide_banner"),
        os("-y"),
        os("-nostdin"),
        os("-f"),
        os("concat"),
        os("-safe"),
        os("0"),
        os("-i"),
        list_file.as_os_str().to_owned(),
        os("-c"),
        os("copy"),
        output.as_os_str().to_owned(),
    ]
}

/// Video bitrate (kbps) that makes a clip of `duration_s` fit in `target_mb`, leaving room for audio.
pub fn target_video_kbps(target_mb: f64, duration_s: f64, audio_kbps: u32) -> Option<u32> {
    if target_mb <= 0.0 || duration_s <= 0.0 {
        return None;
    }
    let total_kbps = target_mb * 8192.0 / duration_s * 0.97; // 3 % container overhead
    let video = total_kbps - f64::from(audio_kbps);
    (video >= 100.0).then_some(video as u32)
}

/// Two-pass H.264 arguments aiming at `video_kbps`. Run pass 1 then pass 2 (`pass_log` is the
/// shared statistics prefix; pass 1 writes to the null device).
pub fn two_pass_args(
    input: &Path,
    output: &Path,
    video_kbps: u32,
    pass: u8,
    pass_log: &Path,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        os("-hide_banner"),
        os("-y"),
        os("-nostdin"),
        os("-i"),
        input.as_os_str().to_owned(),
        os("-c:v"),
        os("libx264"),
        os("-preset"),
        os("medium"),
        os("-b:v"),
        format!("{video_kbps}k").into(),
        os("-pass"),
        pass.to_string().into(),
        os("-passlogfile"),
        pass_log.as_os_str().to_owned(),
    ];
    if pass == 1 {
        a.extend(["-an", "-f", "null"].map(os));
        a.push(os(if cfg!(windows) { "NUL" } else { "/dev/null" }));
    } else {
        a.extend(["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"].map(os));
        a.push(output.as_os_str().to_owned());
    }
    a
}

/// Looks for `ffmpeg(.exe)` in `dir` (if given), next to the program, then on PATH.
pub fn find_ffmpeg(dir: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
        candidates.push(d.join(name));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        candidates.push(exe_dir.join(name));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|p| p.join(name)));
    }
    candidates.into_iter().find(|c| c.is_file())
}

/// Runs ffmpeg and waits for it. The error carries the tail of ffmpeg's stderr.
pub fn run(ffmpeg: &Path, args: &[OsString]) -> Result<()> {
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output()?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stderr);
    let tail: String = text
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(Error::System(format!(
        "ffmpeg failed ({}): {tail}",
        out.status
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn strs(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn plain_mp4_trim_copies_streams() {
        let o = ExportOptions {
            range: Some((5.0, 15.5)),
            ..ExportOptions::default()
        };
        let a = strs(&build_args(Path::new("in.mp4"), Path::new("out.mp4"), &o).unwrap());
        assert!(a.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(a.windows(2).any(|w| w == ["-ss", "5.000"]));
        assert!(a.windows(2).any(|w| w == ["-t", "10.500"]));
        assert_eq!(a.last().unwrap(), "out.mp4");
    }

    #[test]
    fn speed_and_crop_reencode_with_filters() {
        let o = ExportOptions {
            speed: 2.0,
            crop: Crop::Vertical9x16,
            ..ExportOptions::default()
        };
        let a = strs(&build_args(Path::new("i.mp4"), Path::new("o.mp4"), &o).unwrap());
        let vf = &a[a.iter().position(|x| x == "-vf").unwrap() + 1];
        assert!(vf.contains("crop=") && vf.contains("setpts=PTS/2.0000"));
        let af = &a[a.iter().position(|x| x == "-af").unwrap() + 1];
        assert_eq!(af, "atempo=2.0000");
        assert!(a.contains(&"libx264".to_owned()));
    }

    #[test]
    fn gif_has_palette_and_no_audio() {
        let o = ExportOptions {
            format: Format::Gif,
            width: Some(480),
            ..ExportOptions::default()
        };
        let a = strs(&build_args(Path::new("i.mp4"), Path::new("o.gif"), &o).unwrap());
        assert!(a.contains(&"-an".to_owned()));
        let vf = &a[a.iter().position(|x| x == "-vf").unwrap() + 1];
        assert!(vf.contains("palettegen") && vf.contains("scale=480:-2"));
    }

    #[test]
    fn invalid_options_are_rejected() {
        let bad_speed = ExportOptions {
            speed: 10.0,
            ..ExportOptions::default()
        };
        assert!(build_args(Path::new("a"), Path::new("b"), &bad_speed).is_err());
        let bad_range = ExportOptions {
            range: Some((5.0, 5.0)),
            ..ExportOptions::default()
        };
        assert!(build_args(Path::new("a"), Path::new("b"), &bad_range).is_err());
    }

    #[test]
    fn atempo_is_chained_beyond_limits() {
        assert_eq!(atempo_chain(4.0), "atempo=2.0,atempo=2.0000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5000");
    }

    #[test]
    fn concat_list_escapes_quotes_and_backslashes() {
        let l = concat_list(&[PathBuf::from("C:\\v\\it's.mp4")]);
        assert_eq!(l, "file 'C:/v/it'\\''s.mp4'\n");
    }

    #[test]
    fn target_bitrate_accounts_for_audio() {
        // 8 MB over 60 s: ~1061 kbps total after overhead, minus 128 audio.
        let k = target_video_kbps(8.0, 60.0, 128).unwrap();
        assert!((900..=950).contains(&k), "{k}");
        assert!(target_video_kbps(0.1, 600.0, 128).is_none());
    }

    #[test]
    fn two_pass_first_pass_discards_output() {
        let a = strs(&two_pass_args(
            Path::new("i"),
            Path::new("o.mp4"),
            1000,
            1,
            Path::new("log"),
        ));
        assert!(a.contains(&"null".to_owned()));
        assert!(!a.contains(&"o.mp4".to_owned()));
    }
}
