//! `hyperclips-cli`: scriptable access to the clip library and editing tools.
//! Works on files directly; it does not need the background service.

use anyhow::{bail, Context, Result};
use hc_core::config::Config;
use hc_core::paths::AppPaths;
use hc_export::{
    build_args, concat_args, concat_list, find_ffmpeg, run, Crop, ExportOptions, Format,
};
use hc_library::{Filter, Library};
use std::path::{Path, PathBuf};

const HELP: &str = "hyperclips-cli <command>\n\
  list [--favorites] [--tag T] [--search S]\n\
  trim <in> <start-s> <end-s> [out]\n\
  export <in> <mp4|webm|gif|webp> [--vertical|--square] [--speed X] [--width W] [--start S --end E] [out]\n\
  shrink <in> <target-MB> [out]\n\
  merge <out> <in1> <in2> [...]\n\
  share <file> [message]\n";

fn ffmpeg(config: &Config) -> Result<PathBuf> {
    let dir =
        (!config.share.ffmpeg_dir.is_empty()).then(|| PathBuf::from(&config.share.ffmpeg_dir));
    find_ffmpeg(dir.as_deref()).context(
        "ffmpeg not found: put ffmpeg.exe next to hyperclips-cli.exe, on PATH, or set share.ffmpeg_dir in config.toml",
    )
}

fn default_out(input: &Path, suffix: &str, ext: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "clip".into(), |s| s.to_string_lossy().into_owned());
    input.with_file_name(format!("{stem}_{suffix}.{ext}"))
}

fn num(s: &str, what: &str) -> Result<f64> {
    s.parse::<f64>()
        .with_context(|| format!("{what} must be a number"))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(());
    };
    let paths = AppPaths::detect()?;
    let config = Config::load_or_default(&paths.config_file())?;
    let rest = &args[1..];

    match cmd {
        "list" => {
            let mut filter = Filter::default();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--favorites" => filter.favorites_only = true,
                    "--tag" => filter.tag = it.next().cloned().unwrap_or_default(),
                    "--search" => filter.text = it.next().cloned().unwrap_or_default(),
                    other => bail!("unknown option {other}"),
                }
            }
            let lib = Library::open(&paths.data_dir().join("library.db"))?;
            for c in lib.list(&filter)? {
                println!(
                    "{}\t{}{}\t{} MB\t{}\t{}",
                    c.id,
                    if c.favorite { "* " } else { "" },
                    c.path,
                    c.size_bytes / 1_048_576,
                    c.game,
                    c.bookmarks
                        .iter()
                        .map(|ms| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
        }
        "trim" => {
            let [input, start, end, ..] = rest else {
                bail!("usage: trim <in> <start-s> <end-s> [out]")
            };
            let input = PathBuf::from(input);
            let out = rest
                .get(3)
                .map_or_else(|| default_out(&input, "trim", "mp4"), PathBuf::from);
            let o = ExportOptions {
                range: Some((num(start, "start")?, num(end, "end")?)),
                strip_metadata: config.privacy.strip_metadata,
                ..ExportOptions::default()
            };
            run(&ffmpeg(&config)?, &build_args(&input, &out, &o)?)?;
            println!("{}", out.display());
        }
        "export" => {
            let [input, fmt, tail @ ..] = rest else {
                bail!("usage: export <in> <format> [options] [out]")
            };
            let input = PathBuf::from(input);
            let format = Format::parse(fmt).context("format must be mp4, webm, gif or webp")?;
            let mut o = ExportOptions {
                format,
                strip_metadata: config.privacy.strip_metadata,
                ..ExportOptions::default()
            };
            let (mut start, mut end) = (None, None);
            let mut out = None;
            let mut it = tail.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--vertical" => o.crop = Crop::Vertical9x16,
                    "--square" => o.crop = Crop::Square1x1,
                    "--speed" => {
                        o.speed = num(it.next().context("--speed needs a value")?, "speed")?
                    }
                    "--width" => {
                        o.width =
                            Some(num(it.next().context("--width needs a value")?, "width")? as u32)
                    }
                    "--start" => {
                        start = Some(num(it.next().context("--start needs a value")?, "start")?)
                    }
                    "--end" => end = Some(num(it.next().context("--end needs a value")?, "end")?),
                    other if !other.starts_with("--") => out = Some(PathBuf::from(other)),
                    other => bail!("unknown option {other}"),
                }
            }
            if let (Some(s), Some(e)) = (start, end) {
                o.range = Some((s, e));
            }
            let out = out.unwrap_or_else(|| default_out(&input, "export", format.extension()));
            run(&ffmpeg(&config)?, &build_args(&input, &out, &o)?)?;
            println!("{}", out.display());
        }
        "shrink" => {
            let [input, mb, ..] = rest else {
                bail!("usage: shrink <in> <target-MB> [out]")
            };
            let input = PathBuf::from(input);
            let out = rest
                .get(2)
                .map_or_else(|| default_out(&input, "small", "mp4"), PathBuf::from);
            let ff = ffmpeg(&config)?;
            let duration = probe_duration(&ff, &input)?;
            let kbps = hc_export::target_video_kbps(num(mb, "target size")?, duration, 128)
                .context("target size too small for this clip length")?;
            let log = std::env::temp_dir().join("hyperclips-2pass");
            run(&ff, &hc_export::two_pass_args(&input, &out, kbps, 1, &log))?;
            run(&ff, &hc_export::two_pass_args(&input, &out, kbps, 2, &log))?;
            println!("{}", out.display());
        }
        "merge" => {
            let [out, inputs @ ..] = rest else {
                bail!("usage: merge <out> <in1> <in2> [...]")
            };
            if inputs.len() < 2 {
                bail!("merge needs at least two input files");
            }
            let list: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
            let list_file = std::env::temp_dir().join("hyperclips-concat.txt");
            std::fs::write(&list_file, concat_list(&list))?;
            run(&ffmpeg(&config)?, &concat_args(&list_file, Path::new(out)))?;
            let _ = std::fs::remove_file(&list_file);
            println!("{out}");
        }
        "share" => {
            let [file, ..] = rest else {
                bail!("usage: share <file> [message]")
            };
            if config.share.discord_webhook.is_empty() {
                bail!("set share.discord_webhook in config.toml first");
            }
            let message = rest.get(1).map_or("", String::as_str);
            hc_share::send_discord(&config.share.discord_webhook, Path::new(file), message)?;
            println!("sent");
        }
        _ => print!("{HELP}"),
    }
    Ok(())
}

/// Reads the duration from ffmpeg's banner (`Duration: HH:MM:SS.xx`) so no ffprobe is needed.
fn probe_duration(ffmpeg: &Path, input: &Path) -> Result<f64> {
    let out = std::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(input)
        .output()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let line = text
        .lines()
        .find(|l| l.contains("Duration:"))
        .context("cannot read the clip duration")?;
    let value = line
        .split("Duration:")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .context("bad duration line")?
        .trim();
    let mut parts = value.split(':');
    let (h, m, s) = (
        num(parts.next().unwrap_or("0"), "hours")?,
        num(parts.next().unwrap_or("0"), "minutes")?,
        num(parts.next().unwrap_or("0"), "seconds")?,
    );
    Ok(h * 3600.0 + m * 60.0 + s)
}
