//! Thumbnails: small JPEGs in the app cache, generated locally with Media
//! Foundation (video) or WIC (images).

use std::path::{Path, PathBuf};

use hc_capture::screenshot::{ImageFormat, downscale, load_image, save_pixels};
use hc_core::media::TIMEBASE;

use crate::{Kind, LibraryError, Result};

/// Big enough for sharp cards on high-DPI screens (cards are ~240-420 pt).
pub const THUMB_W: u32 = 480;
pub const THUMB_H: u32 = 270;

/// Size and version are part of the name, so thumbnails made by older
/// versions (smaller, or sheared by a stride bug) are regenerated once.
pub fn thumb_path(dir: &Path, id: i64) -> PathBuf {
    dir.join(format!("{id}-{THUMB_W}-v2.jpg"))
}

/// True when `thumb` is the up-to-date thumbnail of item `id`.
pub fn is_current(thumb: Option<&Path>, dir: &Path, id: i64) -> bool {
    thumb.is_some_and(|t| t == thumb_path(dir, id) && t.exists())
}

/// Create the thumbnail for `src` at `dest`. Video thumbnails come from
/// ~1 s in (or the middle of very short clips) to skip black first frames.
pub fn make(src: &Path, kind: Kind, duration_ms: u64, dest: &Path) -> Result<()> {
    hc_encode::mf::init_thread();
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (w, h, stride, px) = match kind {
        Kind::Screenshot => {
            let (w, h, px) = load_image(src).map_err(|e| LibraryError::Other(e.to_string()))?;
            (w, h, w * 4, px)
        }
        Kind::Clip | Kind::Recording => {
            let at_ms = (duration_ms / 2).min(1_000) as i64;
            hc_encode::probe::decode_frame(src, at_ms * TIMEBASE / 1000)
                .map_err(|e| LibraryError::Other(e.to_string()))?
        }
    };
    let (tw, th, small) = downscale(w, h, stride, &px, THUMB_W, THUMB_H);
    save_pixels(tw, th, tw * 4, &small, dest, ImageFormat::Jpeg)
        .map_err(|e| LibraryError::Other(e.to_string()))
}
