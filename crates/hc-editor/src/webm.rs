//! Minimal WebM (Matroska) writer for one AV1 video track. No metadata
//! (muxing/writing app left empty).

use std::io::{Seek, SeekFrom, Write};

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;

fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

fn vint(n: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        if n < (1u64 << (7 * len)) - 1 {
            let mut out = vec![0u8; len as usize];
            let v = n | (1u64 << (7 * len));
            for (i, o) in out.iter_mut().enumerate() {
                *o = (v >> (8 * (len as usize - 1 - i))) as u8;
            }
            return out;
        }
    }
    let mut out = vec![0x01];
    out.extend_from_slice(&n.to_be_bytes()[1..]);
    out
}

fn el(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(vint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn uint(id: u32, v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(7);
    el(id, &b[skip..])
}

pub struct WebmWriter<W: Write + Seek> {
    out: W,
    segment_data_start: u64,
    duration_pos: u64,
    cluster: Vec<u8>,
    cluster_ms: Option<u64>,
    last_ms: u64,
}

impl<W: Write + Seek> WebmWriter<W> {
    /// `codec_private` is the av1C record (header + sequence header OBU).
    pub fn new(mut out: W, width: u32, height: u32, codec_private: &[u8]) -> std::io::Result<Self> {
        let header = [
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            el(0x4282, b"webm"),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ]
        .concat();
        out.write_all(&el(EBML, &header))?;
        // Segment with an 8-byte size patched in finish().
        out.write_all(&id_bytes(SEGMENT))?;
        out.write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?;
        let segment_data_start = out.stream_position()?;
        // Info: timecode scale 1 ms, duration (float64) patched at the end.
        let mut info = uint(0x2AD7B1, 1_000_000);
        info.extend(el(0x4D80, b""));
        info.extend(el(0x5741, b""));
        let dur_offset_in_info = info.len() + 3; // id(2) + size(1) precede the float
        info.extend(el(0x4489, &0f64.to_be_bytes()));
        let info_el = el(INFO, &info);
        let info_start = out.stream_position()?;
        let info_header = info_el.len() - info.len();
        out.write_all(&info_el)?;
        let duration_pos = info_start + info_header as u64 + dur_offset_in_info as u64;
        let video = [uint(0xB0, u64::from(width)), uint(0xBA, u64::from(height))].concat();
        let track = [
            uint(0xD7, 1),
            uint(0x73C5, 1),
            uint(0x83, 1),
            el(0x86, b"V_AV1"),
            el(0x63A2, codec_private),
            el(0xE0, &video),
        ]
        .concat();
        out.write_all(&el(TRACKS, &el(0xAE, &track)))?;
        Ok(Self {
            out,
            segment_data_start,
            duration_pos,
            cluster: Vec::new(),
            cluster_ms: None,
            last_ms: 0,
        })
    }

    fn flush_cluster(&mut self) -> std::io::Result<()> {
        if let Some(ms) = self.cluster_ms.take() {
            let mut body = uint(0xE7, ms);
            body.append(&mut self.cluster);
            self.out.write_all(&el(CLUSTER, &body))?;
        }
        Ok(())
    }

    pub fn write(&mut self, pts_ms: u64, key: bool, data: &[u8]) -> std::io::Result<()> {
        let start_new = match self.cluster_ms {
            None => true,
            Some(c) => key || pts_ms.saturating_sub(c) > 30_000,
        };
        if start_new {
            self.flush_cluster()?;
            self.cluster_ms = Some(pts_ms);
        }
        let rel = (pts_ms - self.cluster_ms.unwrap_or(pts_ms)) as i16;
        let mut block = vec![0x81];
        block.extend_from_slice(&rel.to_be_bytes());
        block.push(if key { 0x80 } else { 0x00 });
        block.extend_from_slice(data);
        self.cluster.extend(el(0xA3, &block));
        self.last_ms = self.last_ms.max(pts_ms);
        Ok(())
    }

    pub fn finish(mut self, frame_ms: u64) -> std::io::Result<W> {
        self.flush_cluster()?;
        let end = self.out.stream_position()?;
        let size = end - self.segment_data_start;
        self.out
            .seek(SeekFrom::Start(self.segment_data_start - 7))?;
        self.out.write_all(&size.to_be_bytes()[1..])?;
        self.out.seek(SeekFrom::Start(self.duration_pos))?;
        self.out
            .write_all(&((self.last_ms + frame_ms) as f64).to_be_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn vint_encoding() {
        assert_eq!(vint(0), vec![0x80]);
        assert_eq!(vint(126), vec![0xFE]);
        assert_eq!(vint(127), vec![0x40, 0x7F]);
        assert_eq!(id_bytes(0xA3), vec![0xA3]);
        assert_eq!(id_bytes(EBML), vec![0x1A, 0x45, 0xDF, 0xA3]);
    }

    #[test]
    fn writes_structure() {
        let mut w = WebmWriter::new(Cursor::new(Vec::new()), 64, 32, &[0x81, 0, 0x0C, 0]).unwrap();
        for f in 0..90u64 {
            w.write(f * 33, f % 30 == 0, &[0x32, 0x01, 0x00]).unwrap();
        }
        let data = w.finish(33).unwrap().into_inner();
        assert_eq!(&data[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        let clusters = data
            .windows(4)
            .filter(|c| *c == [0x1F, 0x43, 0xB6, 0x75])
            .count();
        assert_eq!(clusters, 3);
        // Segment size covers the rest of the file.
        let seg = data
            .windows(4)
            .position(|c| c == [0x18, 0x53, 0x80, 0x67])
            .unwrap();
        let size = u64::from_be_bytes([
            0,
            data[seg + 5],
            data[seg + 6],
            data[seg + 7],
            data[seg + 8],
            data[seg + 9],
            data[seg + 10],
            data[seg + 11],
        ]);
        assert_eq!(seg as u64 + 12 + size, data.len() as u64);
    }
}
