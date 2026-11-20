//! H.264 / HEVC elementary-stream helpers: Annex-B parsing, conversion to
//! length-prefixed NAL units, parameter-set extraction.

use hc_core::media::VideoCodec;

/// Split an Annex-B byte stream into NAL units (without start codes).
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        let sc3 = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        if sc3 {
            if let Some(s) = start {
                // Trim trailing zero (part of a 4-byte start code).
                let mut end = i;
                while end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                if end > s {
                    nals.push(&data[s..end]);
                }
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    match start {
        Some(s) if s < data.len() => nals.push(&data[s..]),
        None if !data.is_empty() => nals.push(data),
        _ => {}
    }
    nals
}

/// NAL type for the codec.
pub fn nal_type(codec: VideoCodec, nal: &[u8]) -> u8 {
    match (codec, nal.first()) {
        (VideoCodec::H264, Some(b)) => b & 0x1F,
        (VideoCodec::Hevc, Some(b)) => (b >> 1) & 0x3F,
        _ => 0xFF,
    }
}

fn is_param_set(codec: VideoCodec, t: u8) -> bool {
    match codec {
        VideoCodec::H264 => t == 7 || t == 8,
        VideoCodec::Hevc => (32..=34).contains(&t),
    }
}

fn is_dropped(codec: VideoCodec, t: u8) -> bool {
    match codec {
        // Access unit delimiter, SEI kept? drop AUD only.
        VideoCodec::H264 => t == 9,
        VideoCodec::Hevc => t == 35,
    }
}

/// Whether the access unit contains an IDR / IRAP picture.
pub fn is_keyframe(codec: VideoCodec, nals: &[&[u8]]) -> bool {
    nals.iter().any(|n| {
        let t = nal_type(codec, n);
        match codec {
            VideoCodec::H264 => t == 5,
            VideoCodec::Hevc => (16..=23).contains(&t),
        }
    })
}

/// Result of converting one encoder output buffer.
#[derive(Debug, Default)]
pub struct Converted {
    /// 4-byte length-prefixed NAL units, parameter sets removed.
    pub avcc: Vec<u8>,
    /// Parameter sets found in this access unit, in stream order.
    pub param_sets: Vec<Vec<u8>>,
    pub keyframe: bool,
}

/// Convert an Annex-B access unit into MP4 sample format.
pub fn annexb_to_avcc(codec: VideoCodec, data: &[u8]) -> Converted {
    let nals = split_annexb(data);
    let mut out = Converted {
        keyframe: is_keyframe(codec, &nals),
        ..Default::default()
    };
    out.avcc.reserve(data.len() + 16);
    for nal in nals {
        let t = nal_type(codec, nal);
        if is_param_set(codec, t) {
            out.param_sets.push(nal.to_vec());
        } else if !is_dropped(codec, t) {
            out.avcc
                .extend_from_slice(&(nal.len() as u32).to_be_bytes());
            out.avcc.extend_from_slice(nal);
        }
    }
    out
}

/// Order parameter sets as the sample entry needs them and drop duplicates.
pub fn normalize_param_sets(codec: VideoCodec, sets: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let order: &[u8] = match codec {
        VideoCodec::H264 => &[7, 8],
        VideoCodec::Hevc => &[32, 33, 34],
    };
    let mut out = Vec::new();
    for t in order {
        if let Some(s) = sets.iter().find(|s| nal_type(codec, s) == *t) {
            out.push(s.clone());
        }
    }
    out
}

/// Remove emulation-prevention bytes (00 00 03 -> 00 00).
pub fn unescape_rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// The 12 bytes of HEVC `general_profile_tier_level` that hvcC copies
/// verbatim: profile_space/tier/profile_idc, 32 compat flags, 48 constraint
/// bits, level_idc.
pub fn hevc_profile_tier_level(sps: &[u8]) -> Option<[u8; 12]> {
    let rbsp = unescape_rbsp(sps);
    // 2-byte NAL header + 1 byte (vps id, max_sub_layers, nesting flag).
    let ptl = rbsp.get(3..15)?;
    let mut out = [0u8; 12];
    out.copy_from_slice(ptl);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_mixed_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9,
        ];
        let nals = split_annexb(&data);
        assert_eq!(
            nals,
            vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]
        );
    }

    #[test]
    fn converts_h264_idr() {
        let data = [
            0, 0, 0, 1, 0x09, 0xF0, // AUD (dropped)
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, // SPS
            0, 0, 0, 1, 0x68, 0xEE, // PPS
            0, 0, 1, 0x65, 0xAA, 0xBB, // IDR slice
        ];
        let c = annexb_to_avcc(VideoCodec::H264, &data);
        assert!(c.keyframe);
        assert_eq!(c.param_sets.len(), 2);
        assert_eq!(c.avcc, vec![0, 0, 0, 3, 0x65, 0xAA, 0xBB]);
    }

    #[test]
    fn non_idr_is_not_key() {
        let c = annexb_to_avcc(VideoCodec::H264, &[0, 0, 1, 0x41, 1]);
        assert!(!c.keyframe);
    }

    #[test]
    fn hevc_types() {
        // IDR_W_RADL = 19 -> header byte 19<<1 = 0x26
        let data = [0, 0, 0, 1, 0x40, 0x01, 7, 0, 0, 1, 0x26, 0x01, 5];
        let c = annexb_to_avcc(VideoCodec::Hevc, &data);
        assert!(c.keyframe);
        assert_eq!(c.param_sets, vec![vec![0x40, 0x01, 7]]);
    }

    #[test]
    fn unescape() {
        assert_eq!(unescape_rbsp(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
    }

    #[test]
    fn normalizes_order() {
        let sets = vec![vec![0x68, 1], vec![0x67, 2], vec![0x68, 1]];
        assert_eq!(
            normalize_param_sets(VideoCodec::H264, &sets),
            vec![vec![0x67, 2], vec![0x68, 1]]
        );
    }
}
