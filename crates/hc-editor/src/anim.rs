//! Animated GIF and animated WebP writers (CPU, from BGRA frames).

use std::io::Write;

/// Streams frames into an animated GIF (NeuQuant palette per frame).
pub struct GifWriter<W: Write> {
    enc: gif::Encoder<W>,
    delay_cs: u16,
}

impl<W: Write> GifWriter<W> {
    pub fn new(out: W, width: u16, height: u16, fps: u32) -> Result<Self, String> {
        let mut enc = gif::Encoder::new(out, width, height, &[]).map_err(|e| e.to_string())?;
        enc.set_repeat(gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            enc,
            delay_cs: (100 / fps.max(1)).max(2) as u16,
        })
    }

    pub fn add(&mut self, width: u16, height: u16, bgra: &[u8]) -> Result<(), String> {
        let mut rgba: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, _]| [r, g, b, 255])
            .collect();
        let mut frame = gif::Frame::from_rgba_speed(width, height, &mut rgba, 10);
        frame.delay = self.delay_cs;
        self.enc.write_frame(&frame).map_err(|e| e.to_string())
    }
}

/// Collects lossless WebP frames and writes an animated WebP container.
pub struct WebpAnimWriter {
    width: u32,
    height: u32,
    frame_ms: u32,
    frames: Vec<Vec<u8>>,
}

fn chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

fn u24(v: u32) -> [u8; 3] {
    let b = v.to_le_bytes();
    [b[0], b[1], b[2]]
}

/// Extract the `VP8L` chunk (header included) from a still WebP file.
fn vp8l_chunk(webp: &[u8]) -> Option<&[u8]> {
    let mut i = 12; // "RIFF" size "WEBP"
    while i + 8 <= webp.len() {
        let fourcc = &webp[i..i + 4];
        let size = u32::from_le_bytes(webp[i + 4..i + 8].try_into().ok()?) as usize;
        let end = i + 8 + size + (size & 1);
        if fourcc == b"VP8L" {
            return webp.get(i..(i + 8 + size).min(webp.len()));
        }
        i = end;
    }
    None
}

impl WebpAnimWriter {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            frame_ms: 1000 / fps.max(1),
            frames: Vec::new(),
        }
    }

    pub fn add(&mut self, bgra: &[u8]) -> Result<(), String> {
        let rgb: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, _]| [r, g, b])
            .collect();
        let mut still = Vec::new();
        image_webp::WebPEncoder::new(&mut still)
            .encode(&rgb, self.width, self.height, image_webp::ColorType::Rgb8)
            .map_err(|e| e.to_string())?;
        let vp8l = vp8l_chunk(&still).ok_or("encoder produced no VP8L chunk")?;
        self.frames.push(vp8l.to_vec());
        Ok(())
    }

    pub fn finish<W: Write>(self, mut out: W) -> std::io::Result<()> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WEBP");
        let mut vp8x = vec![0x02, 0, 0, 0]; // animation flag
        vp8x.extend_from_slice(&u24(self.width - 1));
        vp8x.extend_from_slice(&u24(self.height - 1));
        chunk(&mut body, b"VP8X", &vp8x);
        let mut anim = Vec::new();
        anim.extend_from_slice(&0xFF00_0000u32.to_le_bytes()); // opaque black background
        anim.extend_from_slice(&0u16.to_le_bytes()); // loop forever
        chunk(&mut body, b"ANIM", &anim);
        for f in &self.frames {
            let mut anmf = Vec::with_capacity(16 + f.len());
            anmf.extend_from_slice(&u24(0));
            anmf.extend_from_slice(&u24(0));
            anmf.extend_from_slice(&u24(self.width - 1));
            anmf.extend_from_slice(&u24(self.height - 1));
            anmf.extend_from_slice(&u24(self.frame_ms));
            anmf.push(0x02); // no blending, no dispose
            anmf.extend_from_slice(f);
            chunk(&mut body, b"ANMF", &anmf);
        }
        out.write_all(b"RIFF")?;
        out.write_all(&(body.len() as u32).to_le_bytes())?;
        out.write_all(&body)?;
        out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, v: u8) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| [v, (i % 255) as u8, 255 - v, 255])
            .collect()
    }

    #[test]
    fn gif_has_header_and_frames() {
        let mut out = Vec::new();
        {
            let mut g = GifWriter::new(&mut out, 32, 16, 10).unwrap();
            for v in [0u8, 128, 255] {
                g.add(32, 16, &frame(32, 16, v)).unwrap();
            }
        }
        assert_eq!(&out[..6], b"GIF89a");
        assert!(out.iter().filter(|b| **b == 0x2C).count() >= 3);
    }

    #[test]
    fn webp_animation_structure() {
        let mut w = WebpAnimWriter::new(32, 16, 10);
        for v in [0u8, 200] {
            w.add(&frame(32, 16, v)).unwrap();
        }
        let mut out = Vec::new();
        w.finish(&mut out).unwrap();
        assert_eq!(&out[..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        assert_eq!(&out[12..16], b"VP8X");
        let size = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        assert_eq!(size + 8, out.len());
        assert_eq!(out.windows(4).filter(|c| *c == b"ANMF").count(), 2);
    }
}
