//! Instant Replay ring buffer.
//!
//! Stores *encoded* packets (never raw frames), evicting whole GOPs from the
//! front so the buffer always starts on a video keyframe.
//!
//! Two storages:
//! - **memory**: payloads are reference counted; a snapshot is a pointer copy;
//! - **disk**: payloads go to a preallocated ring file (deleted automatically
//!   when closed, even after a crash); only ~40 bytes of metadata per packet
//!   stay in RAM. Readers detect data overwritten while they were reading.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hc_core::media::{Packet, TIMEBASE};

const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0000_0100;
/// Fraction of the ring file the live buffer may occupy; the rest protects
/// clips being written from being overwritten mid-read.
const DISK_FILL: f64 = 0.8;

/// Ring file holding packet payloads at monotonically increasing logical offsets.
pub struct DiskRing {
    file: File,
    cap: u64,
    written: AtomicU64,
}

impl DiskRing {
    pub fn create(dir: &Path, capacity: u64) -> io::Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("replay-{}.buf", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .attributes(FILE_ATTRIBUTE_TEMPORARY)
            .open(path)?;
        file.set_len(capacity)?;
        Ok(Arc::new(Self {
            file,
            cap: capacity,
            written: AtomicU64::new(0),
        }))
    }

    fn append(&self, data: &[u8]) -> io::Result<u64> {
        let at = self.written.load(Ordering::Acquire);
        let mut done = 0usize;
        while done < data.len() {
            let phys = (at + done as u64) % self.cap;
            let room = (self.cap - phys) as usize;
            let n = room.min(data.len() - done);
            let mut off = 0;
            while off < n {
                off += self
                    .file
                    .seek_write(&data[done + off..done + n], phys + off as u64)?;
            }
            done += n;
        }
        self.written
            .store(at + data.len() as u64, Ordering::Release);
        Ok(at)
    }

    fn valid(&self, at: u64, len: u32) -> bool {
        let w = self.written.load(Ordering::Acquire);
        at + u64::from(len) <= w && w - at <= self.cap
    }

    pub fn read(&self, at: u64, len: u32) -> io::Result<Vec<u8>> {
        let overwritten = || io::Error::other("replay data was overwritten while saving");
        if !self.valid(at, len) {
            return Err(overwritten());
        }
        let mut out = vec![0u8; len as usize];
        let mut done = 0usize;
        while done < out.len() {
            let phys = (at + done as u64) % self.cap;
            let room = (self.cap - phys) as usize;
            let n = room.min(out.len() - done);
            let mut off = 0;
            while off < n {
                let r = self
                    .file
                    .seek_read(&mut out[done + off..done + n], phys + off as u64)?;
                if r == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                off += r;
            }
            done += n;
        }
        // The writer may have lapped us during the read.
        if !self.valid(at, len) {
            return Err(overwritten());
        }
        Ok(out)
    }

    pub fn capacity(&self) -> u64 {
        self.cap
    }
}

#[derive(Debug, Clone)]
enum Payload {
    Mem(Arc<[u8]>),
    Disk { at: u64, len: u32 },
}

impl Payload {
    fn len(&self) -> usize {
        match self {
            Payload::Mem(d) => d.len(),
            Payload::Disk { len, .. } => *len as usize,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub track: u8,
    pub pts: i64,
    pub dur: i64,
    pub key: bool,
    payload: Payload,
}

/// Packets selected for a clip; payloads are materialised lazily.
#[derive(Clone)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    ring: Option<Arc<DiskRing>>,
}

impl Snapshot {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn first_pts(&self) -> Option<i64> {
        self.entries.first().map(|e| e.pts)
    }

    /// Keep only entries with `pts <= end`.
    pub fn truncate_after(&mut self, end: i64) {
        self.entries.retain(|e| e.pts <= end);
    }

    pub fn packets(&self) -> impl Iterator<Item = io::Result<Packet>> + '_ {
        self.entries.iter().map(|e| {
            let data = match &e.payload {
                Payload::Mem(d) => d.clone(),
                Payload::Disk { at, len } => {
                    let ring = self
                        .ring
                        .as_ref()
                        .ok_or_else(|| io::Error::other("no ring"))?;
                    Arc::from(ring.read(*at, *len)?.into_boxed_slice())
                }
            };
            Ok(Packet {
                track: e.track,
                pts: e.pts,
                dur: e.dur,
                key: e.key,
                data,
            })
        })
    }
}

pub struct ReplayBuffer {
    entries: VecDeque<Entry>,
    /// (sequence number, pts) of video keyframes still in the buffer.
    keyframes: VecDeque<(u64, i64)>,
    /// Sequence number of `entries[0]`.
    base_seq: u64,
    last_video_pts: Option<i64>,
    max_ticks: i64,
    max_bytes: usize,
    bytes: usize,
    ring: Option<Arc<DiskRing>>,
}

impl ReplayBuffer {
    /// In-memory buffer: `seconds` of history, `max_bytes` hard cap.
    pub fn new(seconds: u32, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            keyframes: VecDeque::new(),
            base_seq: 0,
            last_video_pts: None,
            max_ticks: i64::from(seconds) * TIMEBASE,
            max_bytes,
            bytes: 0,
            ring: None,
        }
    }

    /// Disk-backed buffer using a ring file of `capacity` bytes in `dir`.
    pub fn new_disk(seconds: u32, dir: &Path, capacity: u64) -> io::Result<Self> {
        let ring = DiskRing::create(dir, capacity)?;
        let mut b = Self::new(seconds, (capacity as f64 * DISK_FILL) as usize);
        b.ring = Some(ring);
        Ok(b)
    }

    pub fn is_disk(&self) -> bool {
        self.ring.is_some()
    }

    pub fn set_duration(&mut self, seconds: u32) {
        self.max_ticks = i64::from(seconds) * TIMEBASE;
        self.evict();
    }

    pub fn clear(&mut self) {
        self.base_seq += self.entries.len() as u64;
        self.entries.clear();
        self.keyframes.clear();
        self.last_video_pts = None;
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Payload bytes held (in RAM for memory mode, on disk for disk mode).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// RAM used by the buffer itself.
    pub fn ram_bytes(&self) -> usize {
        if self.is_disk() {
            self.entries.len() * size_of::<Entry>()
        } else {
            self.bytes
        }
    }

    pub fn span_ticks(&self) -> i64 {
        match (self.keyframes.front(), self.last_video_pts) {
            (Some(&(_, first)), Some(last)) => last - first,
            _ => 0,
        }
    }

    pub fn push(&mut self, p: Packet) -> io::Result<()> {
        // Nothing useful can be decoded before the first keyframe.
        if self.keyframes.is_empty() && !(p.track == 0 && p.key) {
            return Ok(());
        }
        let payload = match &self.ring {
            Some(ring) => {
                let ring = ring.clone();
                // Make room first so the write never laps retained data.
                while self.bytes + p.data.len() > self.max_bytes && self.keyframes.len() > 1 {
                    let (seq2, _) = self.keyframes[1];
                    self.drop_until(seq2);
                }
                let at = ring.append(&p.data)?;
                Payload::Disk {
                    at,
                    len: p.data.len() as u32,
                }
            }
            None => Payload::Mem(p.data.clone()),
        };
        let seq = self.base_seq + self.entries.len() as u64;
        if p.track == 0 {
            if p.key {
                self.keyframes.push_back((seq, p.pts));
            }
            self.last_video_pts = Some(p.pts);
        }
        self.bytes += payload.len();
        self.entries.push_back(Entry {
            track: p.track,
            pts: p.pts,
            dur: p.dur,
            key: p.key,
            payload,
        });
        self.evict();
        Ok(())
    }

    fn drop_until(&mut self, seq: u64) {
        while self.base_seq < seq {
            match self.entries.pop_front() {
                Some(e) => self.bytes -= e.payload.len(),
                None => break,
            }
            self.base_seq += 1;
        }
        while self
            .keyframes
            .front()
            .is_some_and(|(s, _)| *s < self.base_seq)
        {
            self.keyframes.pop_front();
        }
    }

    fn evict(&mut self) {
        let Some(last) = self.last_video_pts else {
            return;
        };
        // Drop the oldest GOP while the next keyframe still covers the window.
        while let Some(&(seq2, pts2)) = self.keyframes.get(1) {
            if last - pts2 >= self.max_ticks || self.bytes > self.max_bytes {
                self.drop_until(seq2);
            } else {
                break;
            }
        }
    }

    fn collect_from(&self, seq: u64, key_pts: i64) -> Snapshot {
        let from = (seq - self.base_seq) as usize;
        Snapshot {
            entries: self
                .entries
                .iter()
                .skip(from)
                .filter(|e| e.track == 0 || e.pts >= key_pts)
                .cloned()
                .collect(),
            ring: self.ring.clone(),
        }
    }

    /// Entries covering the last `seconds` (starting at a keyframe).
    pub fn snapshot(&self, seconds: Option<u32>) -> Snapshot {
        let empty = Snapshot {
            entries: Vec::new(),
            ring: None,
        };
        let Some(last) = self.last_video_pts else {
            return empty;
        };
        let want = seconds.map_or(self.max_ticks, |s| i64::from(s) * TIMEBASE);
        let target = last - want;
        // Latest keyframe at or before the target; otherwise the oldest one.
        let start = self
            .keyframes
            .iter()
            .rev()
            .find(|(_, pts)| *pts <= target)
            .or(self.keyframes.front());
        match start {
            Some(&(seq, key_pts)) => self.collect_from(seq, key_pts),
            None => empty,
        }
    }

    /// Entries since the most recent keyframe (pre-roll for a new recording).
    pub fn since_last_keyframe(&self) -> Snapshot {
        match self.keyframes.back() {
            Some(&(seq, key_pts)) => self.collect_from(seq, key_pts),
            None => Snapshot {
                entries: Vec::new(),
                ring: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: i64 = 10;

    fn video(frame: i64, gop: i64) -> Packet {
        Packet {
            track: 0,
            pts: frame * TIMEBASE / FPS,
            dur: TIMEBASE / FPS,
            key: frame % gop == 0,
            data: Arc::from(vec![(frame % 251) as u8; 100]),
        }
    }

    fn audio(frame: i64) -> Packet {
        Packet {
            track: 1,
            pts: frame * TIMEBASE / FPS,
            dur: TIMEBASE / FPS,
            key: true,
            data: Arc::from(vec![0u8; 10]),
        }
    }

    fn packets(s: &Snapshot) -> Vec<Packet> {
        s.packets().map(|p| p.unwrap()).collect()
    }

    #[test]
    fn ignores_packets_before_first_keyframe() {
        let mut b = ReplayBuffer::new(5, usize::MAX);
        b.push(audio(0)).unwrap();
        b.push(video(1, 100)).unwrap(); // not a key
        assert!(b.is_empty());
        b.push(video(0, 10)).unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn keeps_window_and_starts_on_keyframe() {
        let mut b = ReplayBuffer::new(5, usize::MAX);
        for f in 0..300 {
            b.push(video(f, FPS)).unwrap();
            b.push(audio(f)).unwrap();
        }
        // 5 s window + at most one GOP of slack.
        let span = b.span_ticks();
        assert!((5 * TIMEBASE..=6 * TIMEBASE).contains(&span), "{span}");
        let snap = packets(&b.snapshot(None));
        assert!(snap[0].key && snap[0].track == 0);
        let first = snap[0].pts;
        let last_v = snap
            .iter()
            .filter(|p| p.track == 0)
            .map(|p| p.pts)
            .max()
            .unwrap();
        assert!(last_v - first >= 5 * TIMEBASE);
        assert!(snap.iter().all(|p| p.pts >= first));
    }

    #[test]
    fn shorter_snapshot() {
        let mut b = ReplayBuffer::new(20, usize::MAX);
        for f in 0..200 {
            b.push(video(f, FPS)).unwrap();
        }
        let snap = packets(&b.snapshot(Some(3)));
        let span = snap.last().unwrap().pts - snap[0].pts;
        assert!((3 * TIMEBASE..4 * TIMEBASE).contains(&span), "{span}");
    }

    #[test]
    fn byte_cap_evicts_gops() {
        let mut b = ReplayBuffer::new(1000, 2_500);
        for f in 0..100 {
            b.push(video(f, FPS)).unwrap();
        }
        // 10 frames per GOP * 100 bytes; cap allows ~2 GOPs.
        assert!(b.bytes() <= 2_500 + 1_000);
        assert!(packets(&b.snapshot(None))[0].key);
    }

    #[test]
    fn preroll_from_last_keyframe() {
        let mut b = ReplayBuffer::new(10, usize::MAX);
        for f in 0..25 {
            b.push(video(f, FPS)).unwrap();
        }
        let pre = packets(&b.since_last_keyframe());
        assert_eq!(pre.len(), 5);
        assert!(pre[0].key);
        assert_eq!(pre[0].pts, 20 * TIMEBASE / FPS);
    }

    #[test]
    fn shrinking_duration_evicts() {
        let mut b = ReplayBuffer::new(20, usize::MAX);
        for f in 0..200 {
            b.push(video(f, FPS)).unwrap();
        }
        b.set_duration(5);
        assert!(b.span_ticks() <= 6 * TIMEBASE);
        b.clear();
        assert!(b.is_empty() && b.bytes() == 0);
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("hc-buf-{tag}-{}", std::process::id()))
    }

    #[test]
    fn disk_buffer_roundtrips_and_wraps() {
        let dir = tmp_dir("wrap");
        // 4 KB ring: forces many wrap-arounds with 100-byte frames.
        let mut b = ReplayBuffer::new_disk(1000, &dir, 4096).unwrap();
        for f in 0..500 {
            b.push(video(f, FPS)).unwrap();
        }
        assert!(b.is_disk());
        assert!(b.bytes() <= 4096);
        assert!(b.ram_bytes() < 64 * 1024);
        let snap = packets(&b.snapshot(None));
        assert!(snap[0].key);
        for p in &snap {
            let frame = p.pts * FPS / TIMEBASE;
            assert!(
                p.data.iter().all(|x| *x == (frame % 251) as u8),
                "frame {frame} corrupted"
            );
        }
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disk_detects_overwritten_data() {
        let dir = tmp_dir("ovw");
        let mut b = ReplayBuffer::new_disk(1000, &dir, 4096).unwrap();
        for f in 0..20 {
            b.push(video(f, FPS)).unwrap();
        }
        let old = b.snapshot(None);
        // Lap the ring several times; the old snapshot must refuse to read.
        for f in 20..400 {
            b.push(video(f, FPS)).unwrap();
        }
        assert!(old.packets().any(|p| p.is_err()));
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disk_file_is_deleted_on_close() {
        let dir = tmp_dir("del");
        let b = ReplayBuffer::new_disk(10, &dir, 1024).unwrap();
        let count = || std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        assert_eq!(count(), 1);
        drop(b);
        assert_eq!(count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
