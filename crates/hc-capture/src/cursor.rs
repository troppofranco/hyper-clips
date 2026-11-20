//! Conversion of DXGI pointer shapes to straight-alpha BGRA bitmaps.
//!
//! Desktop Duplication does not draw the mouse pointer into the frame; it
//! reports position and shape separately, so we composite it ourselves.

/// winuser / dxgi1_2 pointer shape types.
pub const SHAPE_MONOCHROME: u32 = 1;
pub const SHAPE_COLOR: u32 = 2;
pub const SHAPE_MASKED_COLOR: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA, straight alpha.
    pub bgra: Vec<u8>,
}

/// Convert a raw shape buffer. `height` is the *reported* height (for
/// monochrome shapes it covers both AND and XOR masks).
pub fn shape_to_bgra(
    kind: u32,
    width: u32,
    height: u32,
    pitch: u32,
    data: &[u8],
) -> Option<CursorImage> {
    let (w, h, p) = (width as usize, height as usize, pitch as usize);
    if w == 0 || h == 0 {
        return None;
    }
    match kind {
        SHAPE_COLOR => {
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                out.extend_from_slice(data.get(y * p..y * p + w * 4)?);
            }
            Some(CursorImage {
                width,
                height,
                bgra: out,
            })
        }
        SHAPE_MASKED_COLOR => {
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = data.get(y * p..y * p + w * 4)?;
                for px in row.as_chunks::<4>().0 {
                    // Mask byte 0 = opaque colour; 0xFF = XOR with screen,
                    // approximated as the colour itself when non-black.
                    let xor = px[3] == 0xFF;
                    let visible = !xor || px[0] | px[1] | px[2] != 0;
                    out.extend_from_slice(&[px[0], px[1], px[2], if visible { 255 } else { 0 }]);
                }
            }
            Some(CursorImage {
                width,
                height,
                bgra: out,
            })
        }
        SHAPE_MONOCHROME => {
            let real_h = h / 2;
            let mut out = Vec::with_capacity(w * real_h * 4);
            for y in 0..real_h {
                for x in 0..w {
                    let bit = |row: usize| {
                        data.get(row * p + x / 8)
                            .map(|b| (b >> (7 - (x % 8))) & 1)
                            .unwrap_or(1)
                    };
                    let and = bit(y);
                    let xor = bit(y + real_h);
                    let px = match (and, xor) {
                        (0, 0) => [0, 0, 0, 255],
                        (0, _) => [255, 255, 255, 255],
                        (_, 0) => [0, 0, 0, 0],
                        // Screen inversion: draw white so it stays visible.
                        _ => [255, 255, 255, 255],
                    };
                    out.extend_from_slice(&px);
                }
            }
            Some(CursorImage {
                width,
                height: real_h as u32,
                bgra: out,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_shape_respects_pitch() {
        let data = [1, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7, 8, 9, 9, 9, 9];
        let img = shape_to_bgra(SHAPE_COLOR, 1, 2, 8, &data).unwrap();
        assert_eq!(img.bgra, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn monochrome_masks() {
        // 2x1 cursor: AND row then XOR row, one byte each.
        // pixel0: and=0 xor=0 -> black; pixel1: and=1 xor=0 -> transparent
        let data = [0b0100_0000, 0b0000_0000];
        let img = shape_to_bgra(SHAPE_MONOCHROME, 2, 2, 1, &data).unwrap();
        assert_eq!(img.height, 1);
        assert_eq!(img.bgra, vec![0, 0, 0, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn masked_color() {
        let data = [10, 20, 30, 0, 0, 0, 0, 0xFF];
        let img = shape_to_bgra(SHAPE_MASKED_COLOR, 2, 1, 8, &data).unwrap();
        assert_eq!(img.bgra, vec![10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(shape_to_bgra(SHAPE_COLOR, 4, 4, 16, &[0; 8]).is_none());
        assert!(shape_to_bgra(99, 1, 1, 4, &[0; 4]).is_none());
    }
}
