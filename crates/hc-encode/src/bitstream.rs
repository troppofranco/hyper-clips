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
        VideoCodec::Av1 => false,
    }
}

fn is_dropped(codec: VideoCodec, t: u8) -> bool {
    match codec {
        // Access unit delimiter, SEI kept? drop AUD only.
        VideoCodec::H264 => t == 9,
        VideoCodec::Hevc => t == 35,
        VideoCodec::Av1 => false,
    }
}

/// Whether the access unit contains an IDR / IRAP picture.
pub fn is_keyframe(codec: VideoCodec, nals: &[&[u8]]) -> bool {
    nals.iter().any(|n| {
        let t = nal_type(codec, n);
        match codec {
            VideoCodec::H264 => t == 5,
            VideoCodec::Hevc => (16..=23).contains(&t),
            VideoCodec::Av1 => false,
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
    if codec == VideoCodec::Av1 {
        return av1_to_mp4(data);
    }
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
        VideoCodec::Av1 => {
            return sets
                .iter()
                .find(|s| obu_type(s) == OBU_SEQUENCE_HEADER)
                .cloned()
                .into_iter()
                .collect();
        }
    };
    let mut out = Vec::new();
    for t in order {
        if let Some(s) = sets.iter().find(|s| nal_type(codec, s) == *t) {
            out.push(s.clone());
        }
    }
    out
}

// ---------------------------------------------------------------- AV1 ----

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;

pub fn obu_type(obu: &[u8]) -> u8 {
    obu.first().map_or(0, |b| (b >> 3) & 0x0F)
}

fn read_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for i in 0..8 {
        let b = *data.get(i)?;
        value |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

fn write_leb128(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

/// Split a low-overhead AV1 bitstream into OBUs, each re-emitted with an
/// explicit size field (required by the MP4 binding).
pub fn split_obus(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let header = data[i];
        let ext = header & 0x04 != 0;
        let has_size = header & 0x02 != 0;
        let hlen = if ext { 2 } else { 1 };
        if i + hlen > data.len() {
            break;
        }
        let (payload_len, size_len) = if has_size {
            match read_leb128(&data[i + hlen..]) {
                Some((v, n)) => (v as usize, n),
                None => break,
            }
        } else {
            (data.len() - i - hlen, 0)
        };
        let start = i + hlen + size_len;
        let end = (start + payload_len).min(data.len());
        let mut obu = Vec::with_capacity(hlen + 4 + payload_len);
        obu.push(header | 0x02);
        if ext {
            obu.push(data[i + 1]);
        }
        write_leb128((end - start) as u64, &mut obu);
        obu.extend_from_slice(&data[start..end]);
        out.push(obu);
        i = end;
    }
    out
}

/// AV1 temporal unit → MP4 sample: drop temporal delimiters, keep the
/// sequence header in-band (allowed, and it marks keyframes from NVENC/AMF).
pub fn av1_to_mp4(data: &[u8]) -> Converted {
    let mut out = Converted::default();
    for obu in split_obus(data) {
        match obu_type(&obu) {
            OBU_TEMPORAL_DELIMITER => {}
            OBU_SEQUENCE_HEADER => {
                out.keyframe = true;
                out.param_sets.push(obu.clone());
                out.avcc.extend_from_slice(&obu);
            }
            _ => out.avcc.extend_from_slice(&obu),
        }
    }
    out
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let byte = self.data.get(self.pos / 8).copied().unwrap_or(0);
        let b = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        u32::from(b)
    }
    fn bits(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |acc, _| (acc << 1) | self.bit())
    }
    fn uvlc(&mut self) {
        let mut zeros = 0;
        while self.bit() == 0 && zeros < 32 {
            zeros += 1;
        }
        let _ = self.bits(zeros);
    }
}

/// The 4 fixed bytes of an v1C box (before configOBUs), from a sequence
/// header OBU. Assumes 8-bit 4:2:0 for the colour fields.
pub fn av1c_header(seq_obu: &[u8]) -> [u8; 4] {
    let hlen = if seq_obu.first().is_some_and(|h| h & 0x04 != 0) {
        2
    } else {
        1
    };
    let payload = read_leb128(seq_obu.get(hlen..).unwrap_or(&[]))
        .and_then(|(_, n)| seq_obu.get(hlen + n..))
        .unwrap_or(&[]);
    let mut r = Bits {
        data: payload,
        pos: 0,
    };
    let profile = r.bits(3);
    let _still = r.bit();
    let reduced = r.bit();
    let (level, tier) = if reduced == 1 {
        (r.bits(5), 0)
    } else {
        let timing = r.bit();
        let mut decoder_model = 0;
        if timing == 1 {
            let _ = r.bits(32);
            let _ = r.bits(32);
            if r.bit() == 1 {
                r.uvlc();
            }
            decoder_model = r.bit();
            if decoder_model == 1 {
                let _ = r.bits(5 + 32 + 5 + 5);
            }
        }
        let initial_delay = r.bit();
        let ops = r.bits(5) + 1;
        let mut first = (0, 0);
        for i in 0..ops {
            let _idc = r.bits(12);
            let lvl = r.bits(5);
            let tier = if lvl > 7 { r.bit() } else { 0 };
            if decoder_model == 1 && r.bit() == 1 {
                let _ = r.bits(10 + 10 + 1); // n = buffer_delay_length (assumed 10)
            }
            if initial_delay == 1 && r.bit() == 1 {
                let _ = r.bits(4);
            }
            if i == 0 {
                first = (lvl, tier);
            }
        }
        first
    };
    [
        0x81,
        ((profile as u8) << 5) | (level as u8 & 0x1F),
        ((tier as u8) << 7) | 0b0000_1100, // 8-bit, not mono, subsampling x=1 y=1
        0,
    ]
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
    fn av1_obus_and_config() {
        // TD (no size), then seq header with size, then frame OBU with size.
        // seq payload: profile 0, still 0, reduced 1, level 8 -> 000 0 1 01000 -> 0x0A 0x00
        let data = [0x12, 0x00, 0x0A, 0x02, 0x0A, 0x00, 0x32, 0x02, 0xAA, 0xBB];
        let c = av1_to_mp4(&data);
        assert!(c.keyframe);
        assert_eq!(c.param_sets, vec![vec![0x0A, 0x02, 0x0A, 0x00]]);
        assert_eq!(c.avcc, vec![0x0A, 0x02, 0x0A, 0x00, 0x32, 0x02, 0xAA, 0xBB]);
        let cfg = av1c_header(&c.param_sets[0]);
        assert_eq!(cfg[0], 0x81);
        assert_eq!(cfg[1], 8); // profile 0, level 8
    }

    #[test]
    fn obu_without_size_gets_one() {
        // frame OBU (type 6) without size field
        let obus = split_obus(&[0x30, 1, 2, 3]);
        assert_eq!(obus, vec![vec![0x32, 3, 1, 2, 3]]);
    }

    #[test]
    fn leb128_roundtrip() {
        for v in [0u64, 1, 127, 128, 300, 1 << 20] {
            let mut b = Vec::new();
            write_leb128(v, &mut b);
            assert_eq!(read_leb128(&b), Some((v, b.len())));
        }
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
