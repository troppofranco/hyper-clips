//! SubRip (.srt) subtitles: import into text overlays and export as sidecar.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    let mut parts = s.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3_600_000 + m * 60_000 + (sec * 1000.0).round() as u64)
}

fn fmt_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn parse(src: &str) -> Vec<Cue> {
    let normalized = src.replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in normalized.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let mut first = lines.next();
        // The numeric index line is optional in practice.
        if first.is_some_and(|l| !l.contains("-->")) {
            first = lines.next();
        }
        let Some(timing) = first else { continue };
        let Some((a, b)) = timing.split_once("-->") else {
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (
            parse_time(a),
            parse_time(b.split_whitespace().next().unwrap_or("")),
        ) else {
            continue;
        };
        let text: Vec<&str> = lines.collect();
        if !text.is_empty() {
            cues.push(Cue {
                start_ms,
                end_ms,
                text: text.join("\n"),
            });
        }
    }
    cues
}

pub fn write(cues: &[Cue]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                fmt_time(c.start_ms),
                fmt_time(c.end_ms),
                c.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let src = "1\r\n00:00:01,500 --> 00:00:03,000\r\nHello\r\nworld\r\n\r\n2\r\n00:01:00,000 --> 00:01:02,250\r\nGG\r\n";
        let cues = parse(src);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_ms, 1500);
        assert_eq!(cues[0].text, "Hello\nworld");
        assert_eq!(cues[1].end_ms, 62_250);
        assert_eq!(parse(&write(&cues)), cues);
    }
}
