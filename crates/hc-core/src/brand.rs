//! Procedurally drawn app icon, so no binary assets are needed for the tray
//! and window icons. (The .exe resource icon is generated from this in Phase 7.)

/// RGBA8 pixels of a `size`×`size` icon: gradient rounded square with a
/// white "play" triangle.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4; // supersampling per axis
    let s = size as f32;
    let radius = s * 0.22;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut bg, mut fg) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    if in_rounded_rect(px, py, s, radius) {
                        bg += 1.0;
                        if in_triangle(px / s, py / s) {
                            fg += 1.0;
                        }
                    }
                }
            }
            let n = (SS * SS) as f32;
            let (bg, fg) = (bg / n, fg / n);
            // Diagonal gradient violet (#7C3AED) -> cyan (#06B6D4).
            let t = ((x + y) as f32) / (2.0 * s);
            let base = [
                lerp(124.0, 6.0, t),
                lerp(58.0, 182.0, t),
                lerp(237.0, 212.0, t),
            ];
            let mix = if bg > 0.0 { fg / bg } else { 0.0 };
            for c in base {
                out.push((lerp(c, 255.0, mix)).round() as u8);
            }
            out.push((bg * 255.0).round() as u8);
        }
    }
    out
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn in_rounded_rect(x: f32, y: f32, s: f32, r: f32) -> bool {
    let cx = x.clamp(r, s - r);
    let cy = y.clamp(r, s - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn in_triangle(u: f32, v: f32) -> bool {
    // Triangle pointing right, in normalised coordinates.
    let (ax, ay, bx, by, cx, cy) = (0.38, 0.28, 0.38, 0.72, 0.74, 0.50);
    let d1 = (u - bx) * (ay - by) - (ax - bx) * (v - by);
    let d2 = (u - cx) * (by - cy) - (bx - cx) * (v - cy);
    let d3 = (u - ax) * (cy - ay) - (cx - ax) * (v - ay);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_shape() {
        let px = icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        // Corner is transparent, centre is opaque white-ish (triangle).
        assert_eq!(px[3], 0);
        let c = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[c + 3], 255);
        assert!(px[c] > 200 && px[c + 1] > 200 && px[c + 2] > 200);
    }
}
