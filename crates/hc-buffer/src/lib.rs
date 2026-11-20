//! Instant Replay ring buffer.
//!
//! Stores *encoded* packets (never raw frames), evicting whole GOPs from the
//! front so the buffer always starts on a video keyframe. Packet payloads are
//! reference counted, so taking a snapshot for saving is a cheap pointer copy
//! and never blocks capture.

use std::collections::VecDeque;

use hc_core::media::{Packet, TIMEBASE};

pub struct ReplayBuffer {
    packets: VecDeque<Packet>,
    /// (sequence number, pts) of video keyframes still in the buffer.
    keyframes: VecDeque<(u64, i64)>,
    /// Sequence number of `packets[0]`.
    base_seq: u64,
    last_video_pts: Option<i64>,
    max_ticks: i64,
    max_bytes: usize,
    bytes: usize,
}

impl ReplayBuffer {
    /// `seconds` of history; `max_bytes` is a hard cap (oldest GOPs go first).
    pub fn new(seconds: u32, max_bytes: usize) -> Self {
        Self {
            packets: VecDeque::new(),
            keyframes: VecDeque::new(),
            base_seq: 0,
            last_video_pts: None,
            max_ticks: i64::from(seconds) * TIMEBASE,
            max_bytes,
            bytes: 0,
        }
    }

    pub fn set_duration(&mut self, seconds: u32) {
        self.max_ticks = i64::from(seconds) * TIMEBASE;
        self.evict();
    }

    pub fn clear(&mut self) {
        self.base_seq += self.packets.len() as u64;
        self.packets.clear();
        self.keyframes.clear();
        self.last_video_pts = None;
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Seconds of video currently held.
    pub fn span_ticks(&self) -> i64 {
        match (self.keyframes.front(), self.last_video_pts) {
            (Some(&(_, first)), Some(last)) => last - first,
            _ => 0,
        }
    }

    pub fn push(&mut self, p: Packet) {
        // Nothing useful can be decoded before the first keyframe.
        if self.keyframes.is_empty() && !(p.track == 0 && p.key) {
            return;
        }
        let seq = self.base_seq + self.packets.len() as u64;
        if p.track == 0 {
            if p.key {
                self.keyframes.push_back((seq, p.pts));
            }
            self.last_video_pts = Some(p.pts);
        }
        self.bytes += p.data.len();
        self.packets.push_back(p);
        self.evict();
    }

    fn drop_until(&mut self, seq: u64) {
        while self.base_seq < seq {
            match self.packets.pop_front() {
                Some(p) => self.bytes -= p.data.len(),
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

    /// Packets covering the last `seconds` (starting at a keyframe), cheap to clone.
    pub fn snapshot(&self, seconds: Option<u32>) -> Vec<Packet> {
        let Some(last) = self.last_video_pts else {
            return Vec::new();
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
        let Some(&(seq, key_pts)) = start else {
            return Vec::new();
        };
        let from = (seq - self.base_seq) as usize;
        self.packets
            .iter()
            .skip(from)
            .filter(|p| p.track == 0 || p.pts >= key_pts)
            .cloned()
            .collect()
    }

    /// Packets since the most recent keyframe (pre-roll for a new recording).
    pub fn since_last_keyframe(&self) -> Vec<Packet> {
        let Some(&(seq, key_pts)) = self.keyframes.back() else {
            return Vec::new();
        };
        let from = (seq - self.base_seq) as usize;
        self.packets
            .iter()
            .skip(from)
            .filter(|p| p.track == 0 || p.pts >= key_pts)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const FPS: i64 = 10;

    fn video(frame: i64, gop: i64) -> Packet {
        Packet {
            track: 0,
            pts: frame * TIMEBASE / FPS,
            dur: TIMEBASE / FPS,
            key: frame % gop == 0,
            data: Arc::from(vec![0u8; 100]),
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

    #[test]
    fn ignores_packets_before_first_keyframe() {
        let mut b = ReplayBuffer::new(5, usize::MAX);
        b.push(audio(0));
        b.push(video(1, 100)); // not a key
        assert!(b.is_empty());
        b.push(video(0, 10));
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn keeps_window_and_starts_on_keyframe() {
        let mut b = ReplayBuffer::new(5, usize::MAX);
        for f in 0..300 {
            b.push(video(f, FPS));
            b.push(audio(f));
        }
        // 5 s window + at most one GOP of slack.
        let span = b.span_ticks();
        assert!((5 * TIMEBASE..=6 * TIMEBASE).contains(&span), "{span}");
        let snap = b.snapshot(None);
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
            b.push(video(f, FPS));
        }
        let snap = b.snapshot(Some(3));
        let span = snap.last().unwrap().pts - snap[0].pts;
        assert!((3 * TIMEBASE..4 * TIMEBASE).contains(&span), "{span}");
    }

    #[test]
    fn byte_cap_evicts_gops() {
        let mut b = ReplayBuffer::new(1000, 2_500);
        for f in 0..100 {
            b.push(video(f, FPS));
        }
        // 10 frames per GOP * 100 bytes; cap allows ~2 GOPs.
        assert!(b.bytes() <= 2_500 + 1_000);
        assert!(b.snapshot(None)[0].key);
    }

    #[test]
    fn preroll_from_last_keyframe() {
        let mut b = ReplayBuffer::new(10, usize::MAX);
        for f in 0..25 {
            b.push(video(f, FPS));
        }
        let pre = b.since_last_keyframe();
        assert_eq!(pre.len(), 5);
        assert!(pre[0].key);
        assert_eq!(pre[0].pts, 20 * TIMEBASE / FPS);
    }

    #[test]
    fn shrinking_duration_evicts() {
        let mut b = ReplayBuffer::new(20, usize::MAX);
        for f in 0..200 {
            b.push(video(f, FPS));
        }
        b.set_duration(5);
        assert!(b.span_ticks() <= 6 * TIMEBASE);
        b.clear();
        assert!(b.is_empty() && b.bytes() == 0);
    }
}
