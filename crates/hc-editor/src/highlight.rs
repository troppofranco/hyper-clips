//! Local highlight detection.
//!
//! Finds moments that are much louder than their surroundings (shots,
//! explosions, shouting in voice chat, crowd noise). Everything runs on the
//! local machine from the clip's own audio; no model or network is used.
//!
//! The analysis is deliberately simple and explainable:
//! 1. short-term loudness in 100 ms hops (RMS, dBFS) of the first audio track;
//! 2. a rolling background level (median over ±6 s);
//! 3. moments whose 0.5 s loudness exceeds the background by a margin and an
//!    absolute floor are candidates; the strongest are kept with a minimum
//!    spacing so markers do not cluster.

use std::path::Path;

/// Analysis hop in milliseconds.
pub const HOP_MS: u32 = 100;
/// Audio decoded per pass, so long recordings never sit in memory at once.
const CHUNK_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Highlight {
    /// Position in seconds from the start of the file.
    pub at_secs: f64,
    /// How far above the background it is, in dB.
    pub score_db: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Required margin over the background level (dB).
    pub margin_db: f32,
    /// Ignore anything quieter than this (dBFS).
    pub floor_dbfs: f32,
    /// Minimum distance between two highlights (s).
    pub min_spacing_secs: f64,
    /// Upper bound on the number of highlights.
    pub max_count: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            margin_db: 9.0,
            floor_dbfs: -35.0,
            min_spacing_secs: 8.0,
            max_count: 20,
        }
    }
}

/// Loudness envelope (dBFS per [`HOP_MS`]) of interleaved stereo i16 at 48 kHz.
pub fn envelope(pcm: &[i16]) -> Vec<f32> {
    let hop = (48_000 * HOP_MS as usize / 1000) * 2;
    pcm.chunks(hop)
        .map(|c| {
            let sum: f64 = c.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
            let rms = (sum / c.len().max(1) as f64).sqrt() / 32768.0;
            (20.0 * rms.max(1e-6).log10()) as f32
        })
        .collect()
}

/// Detect highlights from a loudness envelope sampled every [`HOP_MS`].
pub fn detect(env: &[f32], p: &Params) -> Vec<Highlight> {
    if env.len() < 10 {
        return Vec::new();
    }
    // 0.5 s smoothed loudness (power average, then back to dB).
    let smooth = 5usize;
    let pow: Vec<f64> = env
        .iter()
        .map(|&d| 10f64.powf(f64::from(d) / 10.0))
        .collect();
    let short: Vec<f32> = (0..env.len())
        .map(|i| {
            let a = i.saturating_sub(smooth / 2);
            let b = (i + smooth / 2 + 1).min(env.len());
            let m = pow[a..b].iter().sum::<f64>() / (b - a) as f64;
            (10.0 * m.max(1e-12).log10()) as f32
        })
        .collect();
    // Rolling median background over ±6 s (sampled every 0.5 s for speed).
    let half = (6000 / HOP_MS) as usize;
    let mut background = vec![0f32; env.len()];
    let mut last = f32::MIN;
    for (i, slot) in background.iter_mut().enumerate() {
        if i % 5 == 0 || last == f32::MIN {
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(env.len());
            let mut w: Vec<f32> = env[a..b].to_vec();
            w.sort_by(f32::total_cmp);
            last = w[w.len() / 2];
        }
        *slot = last;
    }
    let mut cands: Vec<Highlight> = (0..env.len())
        .filter_map(|i| {
            let score = short[i] - background[i];
            let peak = (i == 0 || short[i] >= short[i - 1])
                && (i + 1 == env.len() || short[i] >= short[i + 1]);
            (peak && score >= p.margin_db && short[i] >= p.floor_dbfs).then_some(Highlight {
                at_secs: i as f64 * f64::from(HOP_MS) / 1000.0,
                score_db: score,
            })
        })
        .collect();
    // Strongest first, then non-maximum suppression by spacing.
    cands.sort_by(|a, b| b.score_db.total_cmp(&a.score_db));
    let mut keep: Vec<Highlight> = Vec::new();
    for c in cands {
        if keep.len() >= p.max_count {
            break;
        }
        if keep
            .iter()
            .all(|k| (k.at_secs - c.at_secs).abs() >= p.min_spacing_secs)
        {
            keep.push(c);
        }
    }
    keep.sort_by(|a, b| a.at_secs.total_cmp(&b.at_secs));
    keep
}

/// Analyse the first audio track of a media file of `duration_ms`.
pub fn detect_file(
    path: &Path,
    duration_ms: u64,
    p: &Params,
) -> windows::core::Result<Vec<Highlight>> {
    if crate::decode::audio_stream_count(path) == 0 {
        return Ok(Vec::new());
    }
    let mut env = Vec::new();
    let mut at = 0u64;
    while at < duration_ms {
        let end = (at + CHUNK_MS).min(duration_ms);
        let pcm = crate::decode::decode_audio(path, 0, at, end)?;
        env.extend(envelope(&pcm));
        at = end;
    }
    Ok(detect(&env, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_isolated_loud_moments() {
        // 60 s of quiet background with two loud bursts at 12 s and 40 s.
        let mut env = vec![-45.0f32; 600];
        env[120..125].fill(-12.0);
        env[400..403].fill(-15.0);
        let h = detect(&env, &Params::default());
        assert_eq!(h.len(), 2, "{h:?}");
        assert!((h[0].at_secs - 12.2).abs() < 0.5);
        assert!((h[1].at_secs - 40.1).abs() < 0.5);
        assert!(h[0].score_db > h[1].score_db);
    }

    #[test]
    fn constant_loudness_has_no_highlights() {
        let env = vec![-10.0f32; 600];
        assert!(detect(&env, &Params::default()).is_empty());
    }

    #[test]
    fn quiet_bumps_below_floor_are_ignored() {
        let mut env = vec![-80.0f32; 300];
        env[150] = -50.0;
        assert!(detect(&env, &Params::default()).is_empty());
    }

    #[test]
    fn spacing_and_cap_are_respected() {
        let mut env = vec![-45.0f32; 3000];
        for k in 0..30 {
            env[50 + k * 95] = -10.0;
        }
        let p = Params {
            max_count: 5,
            ..Params::default()
        };
        let h = detect(&env, &p);
        assert_eq!(h.len(), 5);
        for w in h.windows(2) {
            assert!(w[1].at_secs - w[0].at_secs >= p.min_spacing_secs);
        }
    }

    #[test]
    fn envelope_levels() {
        let silence = vec![0i16; 9600 * 3];
        assert!(envelope(&silence).iter().all(|&d| d < -100.0));
        let full: Vec<i16> = (0..9600)
            .map(|i| if i % 2 == 0 { 32767 } else { -32767 })
            .collect();
        let e = envelope(&full);
        assert_eq!(e.len(), 1);
        assert!(e[0] > -0.1);
    }
}
