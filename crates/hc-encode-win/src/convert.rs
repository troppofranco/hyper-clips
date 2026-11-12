//! BGRA -> NV12 (BT.709, limited range) on the CPU. Portable and unit-tested.
//!
//! This is the Phase 2 path; it costs real CPU time at 1080p. A GPU conversion
//! (D3D11 video processor) replaces it later.

/// Size in bytes of an NV12 frame (`width` and `height` must be even).
pub fn nv12_size(width: u32, height: u32) -> usize {
    (width as usize) * (height as usize) * 3 / 2
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Converts a tightly packed BGRA image into NV12 inside `out`
/// (cleared and resized as needed). Odd trailing row/column are dropped,
/// so the effective size is `(width & !1, height & !1)`, which is returned.
pub fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> (u32, u32) {
    let w = (width & !1) as usize;
    let h = (height & !1) as usize;
    let stride = width as usize * 4;
    out.clear();
    out.resize(w * h * 3 / 2, 0);
    if w == 0 || h == 0 || bgra.len() < stride * h {
        return (w as u32, h as u32);
    }
    let (y_plane, uv_plane) = out.split_at_mut(w * h);

    for row in 0..h {
        let src = &bgra[row * stride..row * stride + w * 4];
        let dst = &mut y_plane[row * w..(row + 1) * w];
        for (px, y) in src.chunks_exact(4).zip(dst.iter_mut()) {
            let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            *y = clamp8(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16);
        }
    }

    for crow in 0..h / 2 {
        let top = &bgra[(crow * 2) * stride..(crow * 2) * stride + w * 4];
        let bottom = &bgra[(crow * 2 + 1) * stride..(crow * 2 + 1) * stride + w * 4];
        let dst = &mut uv_plane[crow * w..(crow + 1) * w];
        for (cx, uv) in dst.chunks_exact_mut(2).enumerate() {
            let i = cx * 8;
            let mut b = 0;
            let mut g = 0;
            let mut r = 0;
            for p in [
                &top[i..i + 4],
                &top[i + 4..i + 8],
                &bottom[i..i + 4],
                &bottom[i + 4..i + 8],
            ] {
                b += i32::from(p[0]);
                g += i32::from(p[1]);
                r += i32::from(p[2]);
            }
            let (b, g, r) = ((b + 2) / 4, (g + 2) / 4, (r + 2) / 4);
            uv[0] = clamp8(((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128);
            uv[1] = clamp8(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128);
        }
    }
    (w as u32, h as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(b: u8, g: u8, r: u8, w: u32, h: u32) -> Vec<u8> {
        std::iter::repeat_n([b, g, r, 255], (w * h) as usize)
            .flatten()
            .collect()
    }

    fn near(a: u8, b: u8, tol: i32) -> bool {
        (i32::from(a) - i32::from(b)).abs() <= tol
    }

    #[test]
    fn black_and_white_map_to_limited_range() {
        let mut out = Vec::new();
        assert_eq!(bgra_to_nv12(&solid(0, 0, 0, 4, 4), 4, 4, &mut out), (4, 4));
        assert_eq!(out.len(), nv12_size(4, 4));
        assert!(out[..16].iter().all(|&y| y == 16));
        assert!(out[16..].iter().all(|&c| c == 128));

        bgra_to_nv12(&solid(255, 255, 255, 4, 4), 4, 4, &mut out);
        assert!(out[..16].iter().all(|&y| near(y, 235, 1)));
        assert!(out[16..].iter().all(|&c| near(c, 128, 1)));
    }

    #[test]
    fn primary_colours_have_the_expected_chroma_direction() {
        let mut out = Vec::new();
        bgra_to_nv12(&solid(0, 0, 255, 2, 2), 2, 2, &mut out); // pure red
        assert!(near(out[0], 63, 2), "Y of red was {}", out[0]);
        // NV12 chroma plane starts at index 4 for a 2x2 image: U, then V.
        assert!(near(out[4], 102, 3), "U of red was {}", out[4]);
        assert!(out[5] > 230, "V of red was {}", out[5]);

        bgra_to_nv12(&solid(255, 0, 0, 2, 2), 2, 2, &mut out); // pure blue
        assert!(out[4] > 230, "U of blue was {}", out[4]);
        assert!(out[5] < 128, "V of blue was {}", out[5]);
    }

    #[test]
    fn odd_sizes_are_cropped_to_even() {
        let mut out = Vec::new();
        let img = solid(10, 20, 30, 5, 3);
        assert_eq!(bgra_to_nv12(&img, 5, 3, &mut out), (4, 2));
        assert_eq!(out.len(), nv12_size(4, 2));
    }

    #[test]
    fn short_input_does_not_panic() {
        let mut out = Vec::new();
        assert_eq!(bgra_to_nv12(&[0; 8], 4, 4, &mut out), (4, 4));
        assert_eq!(out.len(), nv12_size(4, 4));
    }
}
