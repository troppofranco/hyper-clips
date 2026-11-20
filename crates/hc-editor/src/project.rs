//! Non-destructive edit description, serialised as JSON for the export queue.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A piece of a source clip placed on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub path: PathBuf,
    pub in_ms: u64,
    pub out_ms: u64,
    /// Playback speed (0.25 = slow motion, 2.0 = fast forward).
    pub speed: f32,
}

impl Segment {
    pub fn new(path: PathBuf, duration_ms: u64) -> Self {
        Self {
            path,
            in_ms: 0,
            out_ms: duration_ms,
            speed: 1.0,
        }
    }

    /// Length on the output timeline.
    pub fn output_ms(&self) -> u64 {
        let src = self.out_ms.saturating_sub(self.in_ms) as f64;
        (src / f64::from(self.speed.clamp(0.1, 16.0))) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransitionKind {
    #[default]
    Cut,
    Crossfade,
    FadeBlack,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration_ms: u64,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            kind: TransitionKind::Cut,
            duration_ms: 500,
        }
    }
}

/// Output framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Aspect {
    #[default]
    Source,
    /// 16:9
    Landscape,
    /// 9:16 (Shorts/TikTok/Reels)
    Vertical,
    /// 1:1
    Square,
}

impl Aspect {
    /// width / height, or None to keep the source ratio.
    pub fn ratio(self) -> Option<f64> {
        match self {
            Aspect::Source => None,
            Aspect::Landscape => Some(16.0 / 9.0),
            Aspect::Vertical => Some(9.0 / 16.0),
            Aspect::Square => Some(1.0),
        }
    }
}

/// Zoom/pan applied to every segment, plus the aspect crop.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Framing {
    pub aspect: Aspect,
    /// 1.0 = no zoom.
    pub zoom: f32,
    /// Centre of the crop, normalised 0..1.
    pub center_x: f32,
    pub center_y: f32,
}

impl Default for Framing {
    fn default() -> Self {
        Self {
            aspect: Aspect::Source,
            zoom: 1.0,
            center_x: 0.5,
            center_y: 0.5,
        }
    }
}

impl Framing {
    /// Normalised source rectangle [u0, v0, u1, v1] for a source of `w × h`.
    pub fn source_uv(&self, w: u32, h: u32) -> [f32; 4] {
        let src_ratio = f64::from(w.max(1)) / f64::from(h.max(1));
        let (mut uw, mut vh) = match self.aspect.ratio() {
            Some(r) if r < src_ratio => ((r / src_ratio) as f32, 1.0),
            Some(r) => (1.0, (src_ratio / r) as f32),
            None => (1.0, 1.0),
        };
        let z = self.zoom.clamp(1.0, 8.0);
        uw /= z;
        vh /= z;
        let cx = self.center_x.clamp(uw / 2.0, 1.0 - uw / 2.0);
        let cy = self.center_y.clamp(vh / 2.0, 1.0 - vh / 2.0);
        [cx - uw / 2.0, cy - vh / 2.0, cx + uw / 2.0, cy + vh / 2.0]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayKind {
    Text {
        text: String,
        /// Height as a fraction of the output height.
        size: f32,
        /// RGBA.
        color: [u8; 4],
        outline: bool,
    },
    /// Sticker / logo / any image WIC can read.
    Image {
        path: PathBuf,
        /** width as a fraction of the output width */
        width: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub kind: OverlayKind,
    /// On the output timeline.
    pub start_ms: u64,
    pub end_ms: u64,
    /// Centre position, normalised 0..1.
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackMix {
    pub volume: f32,
    pub muted: bool,
}

impl Default for TrackMix {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Format {
    #[default]
    Mp4,
    /// AV1 video only (no Opus encoder available yet).
    WebM,
    Gif,
    WebP,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::WebM => "webm",
            Format::Gif => "gif",
            Format::WebP => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoCodecChoice {
    #[default]
    H264,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub format: Format,
    pub codec: VideoCodecChoice,
    /// Output height; width follows the framing aspect. 0 = source.
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Aim for a file no larger than this (iterative re-encode).
    pub target_size_mb: Option<f64>,
    /// MP4: also write each source audio track separately.
    pub separate_audio: bool,
    pub audio: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            format: Format::Mp4,
            codec: VideoCodecChoice::H264,
            height: 1080,
            fps: 60,
            bitrate_kbps: 12_000,
            target_size_mb: None,
            separate_audio: false,
            audio: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Discord,
    YouTube,
    Shorts,
    Twitter,
    MaxQuality,
    Gif,
    WebP,
}

impl Preset {
    pub const ALL: [Preset; 7] = [
        Preset::Discord,
        Preset::YouTube,
        Preset::Shorts,
        Preset::Twitter,
        Preset::MaxQuality,
        Preset::Gif,
        Preset::WebP,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Preset::Discord => "discord",
            Preset::YouTube => "youtube",
            Preset::Shorts => "shorts",
            Preset::Twitter => "twitter",
            Preset::MaxQuality => "max",
            Preset::Gif => "gif",
            Preset::WebP => "webp",
        }
    }

    /// Apply the preset to output settings and framing.
    pub fn apply(self, out: &mut Output, framing: &mut Framing) {
        let base = Output::default();
        *out = match self {
            // Discord's free upload limit is 10 MB.
            Preset::Discord => Output {
                height: 720,
                fps: 60,
                bitrate_kbps: 4_000,
                target_size_mb: Some(10.0),
                ..base
            },
            Preset::YouTube => Output {
                height: 1080,
                fps: 60,
                bitrate_kbps: 16_000,
                ..base
            },
            Preset::Shorts => Output {
                height: 1920,
                fps: 60,
                bitrate_kbps: 12_000,
                ..base
            },
            Preset::Twitter => Output {
                height: 720,
                fps: 60,
                bitrate_kbps: 6_000,
                target_size_mb: Some(512.0),
                ..base
            },
            Preset::MaxQuality => Output {
                codec: VideoCodecChoice::Hevc,
                height: 0,
                fps: 60,
                bitrate_kbps: 50_000,
                ..base
            },
            Preset::Gif => Output {
                format: Format::Gif,
                height: 360,
                fps: 15,
                audio: false,
                ..base
            },
            Preset::WebP => Output {
                format: Format::WebP,
                height: 360,
                fps: 15,
                audio: false,
                ..base
            },
        };
        if self == Preset::Shorts {
            framing.aspect = Aspect::Vertical;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Project {
    pub segments: Vec<Segment>,
    pub transition: Transition,
    pub framing: Framing,
    pub overlays: Vec<Overlay>,
    /// Per source audio track (index = track order in the source files).
    pub tracks: Vec<TrackMix>,
    pub output: Output,
}

impl Project {
    /// Total output duration, accounting for overlapping transitions.
    pub fn duration_ms(&self) -> u64 {
        let sum: u64 = self.segments.iter().map(Segment::output_ms).sum();
        let overlap = if self.transition.kind == TransitionKind::Crossfade {
            self.transition.duration_ms * self.segments.len().saturating_sub(1) as u64
        } else {
            0
        };
        sum.saturating_sub(overlap)
    }

    /// Start time of each segment on the output timeline.
    pub fn segment_starts(&self) -> Vec<u64> {
        let overlap = if self.transition.kind == TransitionKind::Crossfade {
            self.transition.duration_ms
        } else {
            0
        };
        let mut t = 0u64;
        self.segments
            .iter()
            .map(|s| {
                let start = t;
                t = (t + s.output_ms()).saturating_sub(overlap);
                start
            })
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_starts() {
        let mut p = Project::default();
        p.segments.push(Segment::new("a.mp4".into(), 10_000));
        p.segments.push(Segment {
            speed: 2.0,
            ..Segment::new("b.mp4".into(), 8_000)
        });
        assert_eq!(p.duration_ms(), 14_000);
        assert_eq!(p.segment_starts(), vec![0, 10_000]);
        p.transition = Transition {
            kind: TransitionKind::Crossfade,
            duration_ms: 1_000,
        };
        assert_eq!(p.duration_ms(), 13_000);
        assert_eq!(p.segment_starts(), vec![0, 9_000]);
    }

    #[test]
    fn vertical_crop_uv() {
        let f = Framing {
            aspect: Aspect::Vertical,
            ..Default::default()
        };
        let uv = f.source_uv(1920, 1080);
        let w = uv[2] - uv[0];
        assert!((w - 0.316).abs() < 0.01, "{uv:?}");
        assert!((uv[0] + w / 2.0 - 0.5).abs() < 1e-5);
        assert_eq!((uv[1], uv[3]), (0.0, 1.0));
        // Pan is clamped inside the frame.
        let g = Framing {
            aspect: Aspect::Square,
            center_x: 0.0,
            zoom: 2.0,
            ..Default::default()
        };
        let uv = g.source_uv(1920, 1080);
        assert!(uv[0] >= 0.0 && (uv[3] - uv[1] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn json_roundtrip_and_presets() {
        let mut p = Project::default();
        Preset::Shorts.apply(&mut p.output, &mut p.framing);
        assert_eq!(p.framing.aspect, Aspect::Vertical);
        p.overlays.push(Overlay {
            kind: OverlayKind::Text {
                text: "GG".into(),
                size: 0.08,
                color: [255, 255, 255, 255],
                outline: true,
            },
            start_ms: 0,
            end_ms: 1000,
            x: 0.5,
            y: 0.9,
        });
        let back = Project::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        let mut o = Output::default();
        let mut f = Framing::default();
        Preset::Discord.apply(&mut o, &mut f);
        assert_eq!(o.target_size_mb, Some(10.0));
    }
}
