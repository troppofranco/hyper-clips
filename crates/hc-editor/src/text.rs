//! Text overlays rasterised with GDI (no font files shipped, no network
//! fonts): anti-aliased glyph coverage becomes alpha, with optional outline.

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DeleteDC,
    DeleteObject, DrawTextW, FW_BOLD, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::core::w;

/// Rendered text: straight-alpha BGRA.
pub struct TextImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Render `text` (may contain newlines) at `px` pixels line height.
pub fn render(text: &str, px: u32, color: [u8; 4], outline: bool) -> Option<TextImage> {
    if text.trim().is_empty() {
        return None;
    }
    let px = px.clamp(8, 400) as i32;
    let pad = if outline { (px / 12).max(2) } else { 1 };
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: GDI objects are created, selected, and deleted in this scope.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            Default::default(),
            Default::default(),
            Default::default(),
            ANTIALIASED_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font.into());
        let mut rc = RECT::default();
        DrawTextW(
            dc,
            &mut wide,
            &mut rc,
            DT_CALCRECT | DT_CENTER | DT_NOPREFIX,
        );
        let tw = (rc.right - rc.left).max(1) + 2 * pad;
        let th = (rc.bottom - rc.top).max(1) + 2 * pad;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw,
                biHeight: -th, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(dc);
            return None;
        };
        let old_bmp = SelectObject(dc, dib.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let mut draw = RECT {
            left: pad,
            top: pad,
            right: tw - pad,
            bottom: th - pad,
        };
        DrawTextW(dc, &mut wide, &mut draw, DT_CENTER | DT_NOPREFIX);
        let n = (tw * th) as usize;
        let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
        // Coverage = brightness of the white-on-black rendering.
        let cov: Vec<u8> = src
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0].max(p[1]).max(p[2]))
            .collect();
        SelectObject(dc, old_bmp);
        SelectObject(dc, old_font);
        let _ = DeleteObject(dib.into());
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);

        let (w, h) = (tw as usize, th as usize);
        let outline_a: Vec<u8> = if outline {
            dilate(&cov, w, h, pad as usize)
        } else {
            vec![0; n]
        };
        let mut bgra = vec![0u8; n * 4];
        for k in 0..n {
            let fa = f32::from(cov[k]) / 255.0 * f32::from(color[3]) / 255.0;
            let oa = f32::from(outline_a[k]) / 255.0;
            // Text colour over a black outline ("over" compositing).
            let a = fa + oa * (1.0 - fa);
            if a > 0.0 {
                for c in 0..3 {
                    let col = f32::from(color[2 - c]) * fa / a;
                    bgra[k * 4 + c] = col.round() as u8;
                }
                bgra[k * 4 + 3] = (a * 255.0).round() as u8;
            }
        }
        Some(TextImage {
            width: tw as u32,
            height: th as u32,
            bgra,
        })
    }
}

/// Max filter with a square radius (outline thickness).
fn dilate(a: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; a.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = (lo..=hi).map(|i| a[y * w + i]).max().unwrap_or(0);
        }
    }
    let mut out = vec![0u8; a.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            out[y * w + x] = (lo..=hi).map(|j| tmp[j * w + x]).max().unwrap_or(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_with_alpha() {
        let img = render("GG\nwp", 48, [255, 0, 0, 255], true).unwrap();
        assert!(img.width > 40 && img.height > 80);
        let opaque = img
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 200)
            .count();
        assert!(opaque > 100, "{opaque}");
        // Some pixels are pure red (text body), some black (outline).
        assert!(
            img.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[2] > 200 && p[0] < 50 && p[3] > 200)
        );
        assert!(
            img.bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[2] < 30 && p[3] > 200)
        );
        assert!(render("  ", 20, [255; 4], false).is_none());
    }
}
