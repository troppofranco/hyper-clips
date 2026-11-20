//! Minimal ISO-BMFF (MP4) writer: one video track (H.264/HEVC) plus any
//! number of AAC tracks. Samples are streamed into `mdat`; the `moov` index
//! is written by [`Mp4Writer::finish`]. No metadata (title, software,
//! dates, user names) is ever written: creation times are zero.

use std::io::{self, Seek, SeekFrom, Write};

use hc_core::media::{AudioInfo, AudioRole, Packet, StreamSet, TIMEBASE, VideoCodec, VideoInfo};

use crate::bitstream::hevc_profile_tier_level;

const VIDEO_TIMESCALE: u32 = 90_000;
const MOVIE_TIMESCALE: u32 = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum Mp4Error {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error("unknown track {0}")]
    BadTrack(u8),
    #[error("missing parameter sets for the video track")]
    NoParamSets,
}

struct Track {
    timescale: u32,
    /// Sample start times in track timescale.
    times: Vec<u64>,
    last_dur: u32,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
    sync: Vec<u32>,
}

impl Track {
    fn new(timescale: u32) -> Self {
        Self {
            timescale,
            times: Vec::new(),
            last_dur: 0,
            sizes: Vec::new(),
            offsets: Vec::new(),
            sync: Vec::new(),
        }
    }

    fn duration(&self) -> u64 {
        match (self.times.first(), self.times.last()) {
            (Some(a), Some(b)) => b - a + u64::from(self.last_dur),
            _ => 0,
        }
    }

    /// stts entries (run-length encoded deltas).
    fn stts(&self) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        for i in 0..self.times.len() {
            let d = if i + 1 < self.times.len() {
                (self.times[i + 1].saturating_sub(self.times[i])) as u32
            } else {
                self.last_dur
            };
            match out.last_mut() {
                Some((n, delta)) if *delta == d => *n += 1,
                _ => out.push((1, d)),
            }
        }
        out
    }
}

pub struct Mp4Writer<W: Write + Seek> {
    out: W,
    streams: StreamSet,
    tracks: Vec<Track>,
    mdat_start: u64,
    pos: u64,
    /// pts of the first sample, subtracted from everything.
    origin: Option<i64>,
}

fn to_ts(ticks: i64, timescale: u32) -> u64 {
    let tb = i128::from(TIMEBASE);
    let t = (ticks.max(0) as i128 * i128::from(timescale) + tb / 2) / tb;
    t as u64
}

impl<W: Write + Seek> Mp4Writer<W> {
    pub fn new(mut out: W, streams: StreamSet) -> Result<Self, Mp4Error> {
        if streams.video.param_sets.is_empty() {
            return Err(Mp4Error::NoParamSets);
        }
        let mut ftyp = Vec::new();
        boxed(&mut ftyp, b"ftyp", |b| {
            b.extend_from_slice(b"isom");
            b.extend_from_slice(&0x200u32.to_be_bytes());
            for brand in [b"isom", b"iso2", b"mp41"] {
                b.extend_from_slice(brand);
            }
            match streams.video.codec {
                VideoCodec::H264 => b.extend_from_slice(b"avc1"),
                VideoCodec::Hevc => b.extend_from_slice(b"hvc1"),
                VideoCodec::Av1 => b.extend_from_slice(b"av01"),
            }
        });
        out.write_all(&ftyp)?;
        let mdat_start = ftyp.len() as u64;
        // 64-bit mdat header, size patched in finish().
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&0u64.to_be_bytes())?;
        let mut tracks = vec![Track::new(VIDEO_TIMESCALE)];
        for a in &streams.audio {
            tracks.push(Track::new(a.sample_rate));
        }
        Ok(Self {
            out,
            tracks,
            streams,
            mdat_start,
            pos: mdat_start + 16,
            origin: None,
        })
    }

    /// Append a packet. Packets of each track must be in pts order.
    pub fn write(&mut self, p: &Packet) -> Result<(), Mp4Error> {
        let origin = *self.origin.get_or_insert(p.pts);
        let track = self
            .tracks
            .get_mut(usize::from(p.track))
            .ok_or(Mp4Error::BadTrack(p.track))?;
        let t = to_ts(p.pts - origin, track.timescale);
        // Guard against non-increasing timestamps (jitter): keep strictly increasing.
        let t = match track.times.last() {
            Some(&last) if t <= last => last + 1,
            _ => t,
        };
        self.out.write_all(&p.data)?;
        track.times.push(t);
        track.last_dur = to_ts(p.dur, track.timescale).max(1) as u32;
        track.sizes.push(p.data.len() as u32);
        track.offsets.push(self.pos);
        if p.key || p.track != 0 {
            track.sync.push(track.sizes.len() as u32);
        }
        self.pos += p.data.len() as u64;
        Ok(())
    }

    /// Duration of the video track in ticks.
    pub fn video_duration_ticks(&self) -> i64 {
        let t = &self.tracks[0];
        (t.duration() as i128 * i128::from(TIMEBASE) / i128::from(t.timescale)) as i64
    }

    pub fn finish(mut self) -> Result<W, Mp4Error> {
        let mdat_size = self.pos - self.mdat_start;
        let moov = self.build_moov();
        self.out.write_all(&moov)?;
        self.out.seek(SeekFrom::Start(self.mdat_start + 8))?;
        self.out.write_all(&mdat_size.to_be_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn build_moov(&self) -> Vec<u8> {
        let movie_dur = self
            .tracks
            .iter()
            .map(|t| t.duration() * u64::from(MOVIE_TIMESCALE) / u64::from(t.timescale))
            .max()
            .unwrap_or(0);
        let mut moov = Vec::new();
        boxed(&mut moov, b"moov", |b| {
            full_box(b, b"mvhd", 1, 0, |b| {
                b.extend_from_slice(&0u64.to_be_bytes()); // creation (privacy: zero)
                b.extend_from_slice(&0u64.to_be_bytes()); // modification
                b.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
                b.extend_from_slice(&movie_dur.to_be_bytes());
                b.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
                b.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
                b.extend_from_slice(&[0; 10]);
                matrix(b);
                b.extend_from_slice(&[0; 24]);
                b.extend_from_slice(&(self.tracks.len() as u32 + 1).to_be_bytes());
            });
            for (i, t) in self.tracks.iter().enumerate() {
                self.trak(b, i, t, movie_dur);
            }
        });
        moov
    }

    fn trak(&self, b: &mut Vec<u8>, index: usize, t: &Track, movie_dur: u64) {
        let is_video = index == 0;
        let audio = if is_video {
            None
        } else {
            self.streams.audio.get(index - 1)
        };
        // Only the mix track is enabled by default; stems stay selectable.
        let enabled = is_video || matches!(audio.map(|a| a.role), Some(AudioRole::Mix));
        boxed(b, b"trak", |b| {
            let flags = if enabled { 0x3 } else { 0x2 };
            full_box(b, b"tkhd", 1, flags, |b| {
                b.extend_from_slice(&0u64.to_be_bytes());
                b.extend_from_slice(&0u64.to_be_bytes());
                b.extend_from_slice(&(index as u32 + 1).to_be_bytes());
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&movie_dur.to_be_bytes());
                b.extend_from_slice(&[0; 8]);
                b.extend_from_slice(&0u16.to_be_bytes()); // layer
                let group: u16 = if is_video { 0 } else { 1 };
                b.extend_from_slice(&group.to_be_bytes());
                let vol: u16 = if is_video { 0 } else { 0x0100 };
                b.extend_from_slice(&vol.to_be_bytes());
                b.extend_from_slice(&0u16.to_be_bytes());
                matrix(b);
                let (w, h) = if is_video {
                    (self.streams.video.width, self.streams.video.height)
                } else {
                    (0, 0)
                };
                b.extend_from_slice(&(w << 16).to_be_bytes());
                b.extend_from_slice(&(h << 16).to_be_bytes());
            });
            boxed(b, b"mdia", |b| {
                full_box(b, b"mdhd", 1, 0, |b| {
                    b.extend_from_slice(&0u64.to_be_bytes());
                    b.extend_from_slice(&0u64.to_be_bytes());
                    b.extend_from_slice(&t.timescale.to_be_bytes());
                    b.extend_from_slice(&t.duration().to_be_bytes());
                    b.extend_from_slice(&0x55C4u16.to_be_bytes()); // language "und"
                    b.extend_from_slice(&0u16.to_be_bytes());
                });
                full_box(b, b"hdlr", 0, 0, |b| {
                    b.extend_from_slice(&0u32.to_be_bytes());
                    b.extend_from_slice(if is_video { b"vide" } else { b"soun" });
                    b.extend_from_slice(&[0; 12]);
                    // Audio handlers carry the track role so players and the
                    // editor can tell Mix / Game / Chat / ... apart.
                    match audio {
                        None => b.extend_from_slice(b"VideoHandler\0"),
                        Some(a) => {
                            b.extend_from_slice(a.role.label().as_bytes());
                            b.push(0);
                        }
                    }
                });
                boxed(b, b"minf", |b| {
                    if is_video {
                        full_box(b, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8]));
                    } else {
                        full_box(b, b"smhd", 0, 0, |b| b.extend_from_slice(&[0; 4]));
                    }
                    boxed(b, b"dinf", |b| {
                        full_box(b, b"dref", 0, 0, |b| {
                            b.extend_from_slice(&1u32.to_be_bytes());
                            full_box(b, b"url ", 0, 1, |_| {});
                        });
                    });
                    boxed(b, b"stbl", |b| {
                        full_box(b, b"stsd", 0, 0, |b| {
                            b.extend_from_slice(&1u32.to_be_bytes());
                            match audio {
                                None => video_entry(b, &self.streams.video),
                                Some(a) => audio_entry(b, a),
                            }
                        });
                        let stts = t.stts();
                        full_box(b, b"stts", 0, 0, |b| {
                            b.extend_from_slice(&(stts.len() as u32).to_be_bytes());
                            for (n, d) in &stts {
                                b.extend_from_slice(&n.to_be_bytes());
                                b.extend_from_slice(&d.to_be_bytes());
                            }
                        });
                        if is_video {
                            full_box(b, b"stss", 0, 0, |b| {
                                b.extend_from_slice(&(t.sync.len() as u32).to_be_bytes());
                                for s in &t.sync {
                                    b.extend_from_slice(&s.to_be_bytes());
                                }
                            });
                        }
                        // One sample per chunk keeps interleaving trivial.
                        full_box(b, b"stsc", 0, 0, |b| {
                            b.extend_from_slice(&1u32.to_be_bytes());
                            b.extend_from_slice(&1u32.to_be_bytes());
                            b.extend_from_slice(&1u32.to_be_bytes());
                            b.extend_from_slice(&1u32.to_be_bytes());
                        });
                        full_box(b, b"stsz", 0, 0, |b| {
                            b.extend_from_slice(&0u32.to_be_bytes());
                            b.extend_from_slice(&(t.sizes.len() as u32).to_be_bytes());
                            for s in &t.sizes {
                                b.extend_from_slice(&s.to_be_bytes());
                            }
                        });
                        full_box(b, b"co64", 0, 0, |b| {
                            b.extend_from_slice(&(t.offsets.len() as u32).to_be_bytes());
                            for o in &t.offsets {
                                b.extend_from_slice(&o.to_be_bytes());
                            }
                        });
                    });
                });
            });
        });
    }
}

fn boxed(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full_box(
    out: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    boxed(out, kind, |b| {
        b.extend_from_slice(&((u32::from(version) << 24) | (flags & 0xFF_FFFF)).to_be_bytes());
        body(b);
    });
}

fn matrix(b: &mut Vec<u8>) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        b.extend_from_slice(&v.to_be_bytes());
    }
}

fn video_entry(b: &mut Vec<u8>, v: &VideoInfo) {
    let kind = match v.codec {
        VideoCodec::H264 => b"avc1",
        VideoCodec::Hevc => b"hvc1",
        VideoCodec::Av1 => b"av01",
    };
    boxed(b, kind, |b| {
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&(v.width as u16).to_be_bytes());
        b.extend_from_slice(&(v.height as u16).to_be_bytes());
        b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes()); // frame count
        b.extend_from_slice(&[0; 32]); // compressor name: empty (privacy)
        b.extend_from_slice(&0x0018u16.to_be_bytes());
        b.extend_from_slice(&0xFFFFu16.to_be_bytes());
        match v.codec {
            VideoCodec::H264 => avcc(b, &v.param_sets),
            VideoCodec::Hevc => hvcc(b, &v.param_sets),
            VideoCodec::Av1 => av1c(b, &v.param_sets),
        }
    });
}

fn avcc(b: &mut Vec<u8>, sets: &[Vec<u8>]) {
    let sps: Vec<&Vec<u8>> = sets
        .iter()
        .filter(|s| s.first().map(|x| x & 0x1F) == Some(7))
        .collect();
    let pps: Vec<&Vec<u8>> = sets
        .iter()
        .filter(|s| s.first().map(|x| x & 0x1F) == Some(8))
        .collect();
    let first = sps
        .first()
        .map(|s| s.as_slice())
        .unwrap_or(&[0, 0x64, 0, 0x28]);
    boxed(b, b"avcC", |b| {
        b.push(1);
        b.push(*first.get(1).unwrap_or(&0x64));
        b.push(*first.get(2).unwrap_or(&0));
        b.push(*first.get(3).unwrap_or(&0x28));
        b.push(0xFF); // 4-byte lengths
        b.push(0xE0 | sps.len() as u8);
        for s in &sps {
            b.extend_from_slice(&(s.len() as u16).to_be_bytes());
            b.extend_from_slice(s);
        }
        b.push(pps.len() as u8);
        for p in &pps {
            b.extend_from_slice(&(p.len() as u16).to_be_bytes());
            b.extend_from_slice(p);
        }
    });
}

fn hvcc(b: &mut Vec<u8>, sets: &[Vec<u8>]) {
    let ty = |s: &Vec<u8>| s.first().map(|x| (x >> 1) & 0x3F).unwrap_or(0);
    let ptl = sets
        .iter()
        .find(|s| ty(s) == 33)
        .and_then(|s| hevc_profile_tier_level(s))
        .unwrap_or([0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5D]);
    boxed(b, b"hvcC", |b| {
        b.push(1);
        b.extend_from_slice(&ptl);
        b.extend_from_slice(&0xF000u16.to_be_bytes()); // min_spatial_segmentation 0
        b.push(0xFC); // parallelism unknown
        b.push(0xFD); // chroma 4:2:0
        b.push(0xF8); // luma 8-bit
        b.push(0xF8); // chroma 8-bit
        b.extend_from_slice(&0u16.to_be_bytes()); // avg frame rate unknown
        b.push((1 << 3) | (1 << 2) | 3); // 1 temporal layer, nested, 4-byte lengths
        let arrays: Vec<u8> = [32u8, 33, 34]
            .into_iter()
            .filter(|t| sets.iter().any(|s| ty(s) == *t))
            .collect();
        b.push(arrays.len() as u8);
        for t in arrays {
            b.push(0x80 | t);
            let nals: Vec<&Vec<u8>> = sets.iter().filter(|s| ty(s) == t).collect();
            b.extend_from_slice(&(nals.len() as u16).to_be_bytes());
            for n in nals {
                b.extend_from_slice(&(n.len() as u16).to_be_bytes());
                b.extend_from_slice(n);
            }
        }
    });
}

fn av1c(b: &mut Vec<u8>, sets: &[Vec<u8>]) {
    let seq = sets.first().map(Vec::as_slice).unwrap_or(&[]);
    boxed(b, b"av1C", |b| {
        b.extend_from_slice(&crate::bitstream::av1c_header(seq));
        b.extend_from_slice(seq);
    });
}

fn audio_entry(b: &mut Vec<u8>, a: &AudioInfo) {
    boxed(b, b"mp4a", |b| {
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&a.channels.to_be_bytes());
        b.extend_from_slice(&16u16.to_be_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(a.sample_rate << 16).to_be_bytes());
        full_box(b, b"esds", 0, 0, |b| {
            let dsi_len = a.asc.len() as u8;
            let dcd_len = 13 + 2 + dsi_len;
            let es_len = 3 + 2 + dcd_len + 3;
            b.extend_from_slice(&[0x03, es_len, 0x00, 0x01, 0x00]);
            b.extend_from_slice(&[0x04, dcd_len, 0x40, 0x15]);
            b.extend_from_slice(&[0, 0x18, 0]); // buffer size 6144
            b.extend_from_slice(&a.bitrate.to_be_bytes()); // max
            b.extend_from_slice(&a.bitrate.to_be_bytes()); // avg
            b.extend_from_slice(&[0x05, dsi_len]);
            b.extend_from_slice(&a.asc);
            b.extend_from_slice(&[0x06, 0x01, 0x02]);
        });
    });
}

/// AudioSpecificConfig for AAC-LC.
pub fn aac_lc_asc(sample_rate: u32, channels: u16) -> Vec<u8> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let idx = RATES.iter().position(|r| *r == sample_rate).unwrap_or(3) as u16;
    let v: u16 = (2 << 11) | (idx << 7) | ((channels & 0xF) << 3);
    v.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Arc;

    fn streams() -> StreamSet {
        StreamSet {
            video: VideoInfo {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                fps: 60,
                param_sets: vec![vec![0x67, 0x64, 0, 0x2A, 0xAC], vec![0x68, 0xEE, 0x3C]],
            },
            audio: vec![
                AudioInfo {
                    role: AudioRole::Mix,
                    sample_rate: 48000,
                    channels: 2,
                    asc: aac_lc_asc(48000, 2),
                    bitrate: 160_000,
                },
                AudioInfo {
                    role: AudioRole::Microphone,
                    sample_rate: 48000,
                    channels: 2,
                    asc: aac_lc_asc(48000, 2),
                    bitrate: 128_000,
                },
            ],
        }
    }

    /// Walk top-level and nested boxes, returning (path, size) pairs.
    fn walk(data: &[u8], prefix: &str, out: &mut Vec<(String, usize)>) {
        let containers: &[&[u8; 4]] = &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"dinf"];
        let mut i = 0;
        while i + 8 <= data.len() {
            let mut size = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
            let mut header = 8;
            if size == 1 {
                size = u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap()) as usize;
                header = 16;
            }
            assert!(size >= header && i + size <= data.len(), "bad box {kind:?}");
            let name = format!("{prefix}/{}", String::from_utf8_lossy(&kind));
            out.push((name.clone(), size));
            if containers.contains(&&kind) {
                walk(&data[i + header..i + size], &name, out);
            }
            i += size;
        }
        assert_eq!(i, data.len(), "trailing bytes under {prefix}");
    }

    #[test]
    fn writes_well_formed_file() {
        let mut w = Mp4Writer::new(Cursor::new(Vec::new()), streams()).unwrap();
        for f in 0..120i64 {
            w.write(&Packet {
                track: 0,
                pts: f * TIMEBASE / 60,
                dur: TIMEBASE / 60,
                key: f % 60 == 0,
                data: Arc::from(vec![0u8, 0, 0, 2, 0x65, f as u8]),
            })
            .unwrap();
            if f % 2 == 0 {
                for track in [1u8, 2] {
                    w.write(&Packet {
                        track,
                        pts: f * TIMEBASE / 60,
                        dur: 1024 * TIMEBASE / 48000,
                        key: true,
                        data: Arc::from(vec![0x21u8; 10]),
                    })
                    .unwrap();
                }
            }
        }
        assert_eq!(w.video_duration_ticks(), 2 * TIMEBASE);
        let data = w.finish().unwrap().into_inner();
        let mut boxes = Vec::new();
        walk(&data, "", &mut boxes);
        let names: Vec<&str> = boxes.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names[0], "/ftyp");
        assert_eq!(names[1], "/mdat");
        assert_eq!(names.iter().filter(|n| **n == "/moov/trak").count(), 3);
        assert!(names.contains(&"/moov/trak/mdia/minf/stbl/stss"));
        assert!(names.contains(&"/moov/trak/mdia/minf/stbl/co64"));
        // mdat holds exactly the payload bytes.
        let mdat = boxes[1].1 - 16;
        assert_eq!(mdat, 120 * 6 + 60 * 2 * 10);
    }

    #[test]
    fn stts_is_run_length_encoded() {
        let mut t = Track::new(90_000);
        t.times = vec![0, 1500, 3000, 4500, 6001];
        t.last_dur = 1500;
        assert_eq!(t.stts(), vec![(3, 1500), (1, 1501), (1, 1500)]);
        assert_eq!(t.duration(), 7501);
    }

    #[test]
    fn asc_values() {
        assert_eq!(aac_lc_asc(48000, 2), vec![0x11, 0x90]);
        assert_eq!(aac_lc_asc(44100, 2), vec![0x12, 0x10]);
    }

    #[test]
    fn rejects_missing_param_sets() {
        let mut s = streams();
        s.video.param_sets.clear();
        assert!(Mp4Writer::new(Cursor::new(Vec::new()), s).is_err());
    }
}
