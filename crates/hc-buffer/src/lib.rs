//! Replay buffer: keeps the most recent seconds of *encoded* packets in memory.
//!
//! Time unit everywhere is 100 ns ticks (the Media Foundation unit), as `i64`.
//! Track 0 is the video track and drives clip boundaries: a saved clip always
//! starts on one of its keyframes, so the MP4 can be written without re-encoding.
//! Other tracks (audio) are cut at the same start time.

use std::collections::VecDeque;

/// One second in 100 ns ticks.
pub const TICKS_PER_SEC: i64 = 10_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub data: Vec<u8>,
    /// Presentation time in ticks.
    pub pts: i64,
    pub duration: i64,
    /// True for video keyframes; audio packets are all marked true.
    pub keyframe: bool,
}

/// Packets of one track, oldest first. The first packet is always a keyframe.
#[derive(Debug)]
pub struct TrackBuffer {
    packets: VecDeque<Packet>,
    bytes: usize,
}

impl TrackBuffer {
    fn new() -> Self {
        Self {
            packets: VecDeque::new(),
            bytes: 0,
        }
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

    /// End time of the newest packet.
    fn end(&self) -> Option<i64> {
        self.packets.back().map(|p| p.pts + p.duration)
    }

    fn push(&mut self, packet: Packet) {
        // Never start the buffer on a delta frame: it could not be decoded.
        if self.packets.is_empty() && !packet.keyframe {
            return;
        }
        self.bytes += packet.data.len();
        self.packets.push_back(packet);
    }

    /// Index of the first keyframe after the front packet.
    fn second_keyframe(&self) -> Option<usize> {
        self.packets
            .iter()
            .skip(1)
            .position(|p| p.keyframe)
            .map(|i| i + 1)
    }

    fn drop_front(&mut self, count: usize) {
        for _ in 0..count {
            if let Some(p) = self.packets.pop_front() {
                self.bytes -= p.data.len();
            }
        }
    }

    /// Drops whole GOPs from the front while the rest still covers `max_duration`,
    /// then while the byte budget is exceeded (the byte cap wins over duration).
    fn evict(&mut self, max_duration: i64, max_bytes: usize) {
        while let (Some(k), Some(end)) = (self.second_keyframe(), self.end()) {
            if end - self.packets[k].pts >= max_duration {
                self.drop_front(k);
            } else {
                break;
            }
        }
        while self.bytes > max_bytes {
            match self.second_keyframe() {
                Some(k) => self.drop_front(k),
                None => break, // a single GOP is never split
            }
        }
    }

    /// Drops packets that end at or before `pts` (used to keep audio aligned to video).
    fn trim_before(&mut self, pts: i64) {
        while self
            .packets
            .front()
            .is_some_and(|p| p.pts + p.duration <= pts)
        {
            self.drop_front(1);
        }
    }

    /// Time of the front packet.
    fn front_pts(&self) -> Option<i64> {
        self.packets.front().map(|p| p.pts)
    }

    /// Index of the latest keyframe whose pts is <= `target`, else the front.
    fn keyframe_at_or_before(&self, target: i64) -> usize {
        self.packets
            .iter()
            .enumerate()
            .filter(|(_, p)| p.keyframe && p.pts <= target)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0)
    }
}

/// A cut of the buffer, ready to be muxed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Time of the first video packet; subtract it to start the file at zero.
    pub start_pts: i64,
    /// One entry per track; track 0 is video.
    pub tracks: Vec<Vec<Packet>>,
}

impl Snapshot {
    /// Length of the video track in ticks.
    pub fn duration(&self) -> i64 {
        match (
            self.tracks.first().and_then(|t| t.first()),
            self.tracks.first().and_then(|t| t.last()),
        ) {
            (Some(first), Some(last)) => last.pts + last.duration - first.pts,
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub struct ReplayBuffer {
    tracks: Vec<TrackBuffer>,
    max_duration: i64,
    max_bytes: usize,
}

impl ReplayBuffer {
    /// `max_duration` in ticks, `max_bytes` is the total budget split evenly across tracks.
    pub fn new(track_count: usize, max_duration: i64, max_bytes: usize) -> Self {
        let tracks = (0..track_count.max(1))
            .map(|_| TrackBuffer::new())
            .collect();
        Self {
            tracks,
            max_duration,
            max_bytes,
        }
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn total_bytes(&self) -> usize {
        self.tracks.iter().map(TrackBuffer::bytes).sum()
    }

    /// Adds a packet to `track`. Unknown tracks are ignored.
    ///
    /// Video (track 0) is evicted by GOP. Other tracks follow the video front, so
    /// audio is always available from the start of any clip the video can produce.
    pub fn push(&mut self, track: usize, packet: Packet) {
        let per_track = self.max_bytes / self.tracks.len();
        let max_duration = self.max_duration;
        if track == 0 {
            let Some(video) = self.tracks.first_mut() else {
                return;
            };
            video.push(packet);
            video.evict(max_duration, per_track);
            if let Some(front) = video.front_pts() {
                for t in self.tracks.iter_mut().skip(1) {
                    t.trim_before(front);
                }
            }
            return;
        }
        let video_front = self.tracks.first().and_then(TrackBuffer::front_pts);
        if let Some(t) = self.tracks.get_mut(track) {
            t.push(packet);
            match video_front {
                Some(front) => t.trim_before(front),
                // No video yet: keep a bounded amount so audio cannot grow forever.
                None => t.evict(max_duration.saturating_mul(2), per_track),
            }
            // Byte cap only (duration is governed by the video front).
            t.evict(i64::MAX, per_track);
        }
    }

    /// Changes the retention length (e.g. a per-game profile) and trims immediately.
    pub fn set_max_duration(&mut self, max_duration: i64) {
        self.max_duration = max_duration;
        let per_track = self.max_bytes / self.tracks.len();
        for t in &mut self.tracks {
            t.evict(max_duration, per_track);
        }
    }

    pub fn clear(&mut self) {
        for t in &mut self.tracks {
            t.packets.clear();
            t.bytes = 0;
        }
    }

    /// Cuts the last `last` ticks. The start is moved back to the previous video
    /// keyframe, so the clip may be a little longer than requested, never shorter
    /// (unless the buffer holds less). Returns `None` when there is no video yet.
    pub fn snapshot(&self, last: i64) -> Option<Snapshot> {
        let video = self.tracks.first()?;
        let end = video.end()?;
        let idx = video.keyframe_at_or_before(end - last);
        let start_pts = video.packets.get(idx)?.pts;

        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == 0 {
                    t.packets.iter().skip(idx).cloned().collect()
                } else {
                    // Keep audio that still overlaps the clip start.
                    t.packets
                        .iter()
                        .filter(|p| p.pts + p.duration > start_pts)
                        .cloned()
                        .collect()
                }
            })
            .collect();
        Some(Snapshot { start_pts, tracks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: i64 = TICKS_PER_SEC / 30;

    /// Pushes `seconds` of 30 fps video with a keyframe every `gop` frames.
    fn fill_video(buf: &mut ReplayBuffer, start_frame: i64, frames: i64, gop: i64, size: usize) {
        for n in start_frame..start_frame + frames {
            buf.push(
                0,
                Packet {
                    data: vec![0; size],
                    pts: n * FRAME,
                    duration: FRAME,
                    keyframe: n % gop == 0,
                },
            );
        }
    }

    #[test]
    fn buffer_never_starts_on_a_delta_frame() {
        let mut buf = ReplayBuffer::new(1, 10 * TICKS_PER_SEC, usize::MAX);
        buf.push(
            0,
            Packet {
                data: vec![1],
                pts: 0,
                duration: FRAME,
                keyframe: false,
            },
        );
        assert!(buf.snapshot(TICKS_PER_SEC).is_none());
        buf.push(
            0,
            Packet {
                data: vec![1],
                pts: FRAME,
                duration: FRAME,
                keyframe: true,
            },
        );
        assert!(buf.snapshot(TICKS_PER_SEC).is_some());
    }

    #[test]
    fn retention_keeps_at_least_the_requested_duration() {
        let mut buf = ReplayBuffer::new(1, 5 * TICKS_PER_SEC, usize::MAX);
        // 30 s of video, GOP of 1 s.
        fill_video(&mut buf, 0, 30 * 30, 30, 10);
        let snap = buf.snapshot(5 * TICKS_PER_SEC).expect("snapshot");
        assert!(
            snap.duration() >= 5 * TICKS_PER_SEC,
            "got {}",
            snap.duration()
        );
        // ... and not much more than one GOP extra.
        assert!(snap.duration() <= 6 * TICKS_PER_SEC + FRAME);
        assert!(snap.tracks[0][0].keyframe);
        // Old data really was freed.
        assert!(buf.total_bytes() <= 7 * 30 * 10);
    }

    #[test]
    fn snapshot_of_a_short_request_starts_on_a_keyframe() {
        let mut buf = ReplayBuffer::new(1, 20 * TICKS_PER_SEC, usize::MAX);
        fill_video(&mut buf, 0, 10 * 30, 60, 10); // keyframe every 2 s
        let snap = buf.snapshot(3 * TICKS_PER_SEC).expect("snapshot");
        assert!(snap.tracks[0][0].keyframe);
        assert!(snap.duration() >= 3 * TICKS_PER_SEC);
    }

    #[test]
    fn snapshot_longer_than_buffer_returns_everything() {
        let mut buf = ReplayBuffer::new(1, 20 * TICKS_PER_SEC, usize::MAX);
        fill_video(&mut buf, 0, 3 * 30, 30, 10);
        let snap = buf.snapshot(60 * TICKS_PER_SEC).expect("snapshot");
        assert_eq!(snap.tracks[0].len(), 90);
        assert_eq!(snap.start_pts, 0);
    }

    #[test]
    fn byte_cap_drops_old_gops_but_never_splits_one() {
        // 1 s GOPs of 30 * 100 bytes = 3000 bytes; cap of 7000 keeps two GOPs.
        let mut buf = ReplayBuffer::new(1, 600 * TICKS_PER_SEC, 7000);
        fill_video(&mut buf, 0, 10 * 30, 30, 100);
        assert!(
            buf.total_bytes() <= 7000 + 3000,
            "bytes {}",
            buf.total_bytes()
        );
        let snap = buf.snapshot(600 * TICKS_PER_SEC).expect("snapshot");
        assert!(snap.tracks[0][0].keyframe);
        // A single oversized GOP is kept whole instead of being cut mid-way.
        let mut tiny = ReplayBuffer::new(1, 600 * TICKS_PER_SEC, 10);
        fill_video(&mut tiny, 0, 30, 30, 100);
        assert_eq!(
            tiny.snapshot(TICKS_PER_SEC).expect("snapshot").tracks[0].len(),
            30
        );
    }

    #[test]
    fn audio_is_cut_at_the_video_start() {
        let mut buf = ReplayBuffer::new(2, 4 * TICKS_PER_SEC, usize::MAX);
        fill_video(&mut buf, 0, 20 * 30, 30, 10);
        // Audio: 1024-sample AAC frames at 48 kHz, all keyframes.
        let aac = 1024 * TICKS_PER_SEC / 48_000;
        let mut t = 0;
        while t < 20 * TICKS_PER_SEC {
            buf.push(
                1,
                Packet {
                    data: vec![0; 4],
                    pts: t,
                    duration: aac,
                    keyframe: true,
                },
            );
            t += aac;
        }
        let snap = buf.snapshot(4 * TICKS_PER_SEC).expect("snapshot");
        assert_eq!(snap.tracks.len(), 2);
        let first_audio = &snap.tracks[1][0];
        assert!(first_audio.pts + first_audio.duration > snap.start_pts);
        assert!(
            first_audio.pts <= snap.start_pts,
            "audio must not start after video"
        );
    }

    #[test]
    fn shrinking_the_duration_trims_and_clear_empties() {
        let mut buf = ReplayBuffer::new(1, 20 * TICKS_PER_SEC, usize::MAX);
        fill_video(&mut buf, 0, 20 * 30, 30, 10);
        let before = buf.total_bytes();
        buf.set_max_duration(3 * TICKS_PER_SEC);
        assert!(buf.total_bytes() < before);
        buf.clear();
        assert_eq!(buf.total_bytes(), 0);
        assert!(buf.snapshot(TICKS_PER_SEC).is_none());
    }
}
