//! Keeps an audio track continuous: WASAPI loopback delivers nothing while the PC is silent,
//! but the MP4 needs gap-free samples that line up with the video clock.

/// Output format shared by all tracks: 48 kHz, stereo, 16-bit.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

const TICKS_PER_SEC: i64 = 10_000_000;
/// Gaps shorter than this are left alone (scheduling jitter).
const MIN_GAP_TICKS: i64 = 400_000; // 40 ms

/// Tracks how many audio frames were produced against the wall clock.
#[derive(Debug, Default)]
pub struct Timeline {
    frames: u64,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Time of the next sample in 100 ns ticks since the origin.
    pub fn now_ticks(&self) -> i64 {
        (self.frames as i128 * i128::from(TICKS_PER_SEC) / i128::from(SAMPLE_RATE)) as i64
    }

    /// Number of silent frames to insert so the track catches up with `wall_ticks`.
    pub fn silence_needed(&self, wall_ticks: i64) -> u64 {
        let gap = wall_ticks - self.now_ticks();
        if gap < MIN_GAP_TICKS {
            return 0;
        }
        (gap as i128 * i128::from(SAMPLE_RATE) / i128::from(TICKS_PER_SEC)) as u64
    }

    pub fn advance(&mut self, frames: u64) {
        self.frames += frames;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_gaps_are_ignored() {
        let t = Timeline::new();
        assert_eq!(t.silence_needed(300_000), 0);
    }

    #[test]
    fn large_gap_is_filled() {
        let t = Timeline::new();
        // 1 second behind -> 48000 frames.
        assert_eq!(t.silence_needed(TICKS_PER_SEC), 48_000);
    }

    #[test]
    fn advance_moves_the_clock() {
        let mut t = Timeline::new();
        t.advance(48_000);
        assert_eq!(t.now_ticks(), TICKS_PER_SEC);
        assert_eq!(t.silence_needed(TICKS_PER_SEC), 0);
    }

    #[test]
    fn running_ahead_needs_no_silence() {
        let mut t = Timeline::new();
        t.advance(96_000);
        assert_eq!(t.silence_needed(TICKS_PER_SEC), 0);
    }
}
