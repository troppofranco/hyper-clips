//! Tiny Annex-B (start-code) H.264 helpers: find SPS/PPS and IDR slices.

/// Positions (start of start code, start of NAL header) of every NAL unit.
fn nal_positions(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            // A 4-byte start code has one extra leading zero.
            let begin = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            out.push((begin, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// NAL unit type of every unit in the access unit.
pub fn nal_types(data: &[u8]) -> Vec<u8> {
    nal_positions(data)
        .into_iter()
        .filter_map(|(_, h)| data.get(h).map(|b| b & 0x1F))
        .collect()
}

/// True when the access unit contains an IDR slice (a real keyframe).
pub fn has_idr(data: &[u8]) -> bool {
    nal_types(data).contains(&5)
}

/// Concatenated SPS (type 7) and PPS (type 8) NAL units, start codes included.
/// This is the `MF_MT_MPEG_SEQUENCE_HEADER` blob an MP4 sink needs.
pub fn sequence_header(data: &[u8]) -> Vec<u8> {
    let pos = nal_positions(data);
    let mut out = Vec::new();
    for (n, &(begin, header)) in pos.iter().enumerate() {
        let end = pos
            .get(n + 1)
            .map_or(data.len(), |&(next_begin, _)| next_begin);
        if let Some(b) = data.get(header) {
            if matches!(b & 0x1F, 7 | 8) {
                out.extend_from_slice(&data[begin..end]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0, 0, 0, 1, 0x67, 1, 2, 3]); // SPS
        v.extend_from_slice(&[0, 0, 0, 1, 0x68, 4, 5]); // PPS
        v.extend_from_slice(&[0, 0, 1, 0x65, 9, 9, 9, 9]); // IDR slice, 3-byte start code
        v
    }

    #[test]
    fn finds_nal_types() {
        assert_eq!(nal_types(&sample()), vec![7, 8, 5]);
        assert!(has_idr(&sample()));
        assert!(!has_idr(&[0, 0, 0, 1, 0x41, 1, 2])); // P slice (type 1)
    }

    #[test]
    fn extracts_sps_and_pps_only() {
        let hdr = sequence_header(&sample());
        assert_eq!(hdr, vec![0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5]);
    }

    #[test]
    fn empty_and_garbage_are_safe() {
        assert!(sequence_header(&[]).is_empty());
        assert!(nal_types(&[1, 2, 3, 4]).is_empty());
        assert!(!has_idr(&[0, 0, 1]));
    }
}
