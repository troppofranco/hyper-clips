//! Indexing files and reconciling the database with the clip folders.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::{Kind, Library, NewItem, Result, fsops, thumbs};

const VIDEO_EXT: &[&str] = &["mp4", "mkv", "mov", "webm"];
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];
const MAX_DEPTH: usize = 4;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub added: usize,
    pub removed: usize,
    pub thumbnails: usize,
    pub errors: usize,
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_media(p: &Path) -> bool {
    let e = ext_of(p);
    VIDEO_EXT.contains(&e.as_str()) || IMAGE_EXT.contains(&e.as_str())
}

fn kind_of(p: &Path) -> Kind {
    if IMAGE_EXT.contains(&ext_of(p).as_str()) {
        Kind::Screenshot
    } else if p
        .file_stem()
        .is_some_and(|s| s.to_string_lossy().ends_with(" REC"))
    {
        Kind::Recording
    } else {
        Kind::Clip
    }
}

/// Game name from the folder layout `<root>/<Game>/[Screenshots/]file`.
fn game_from_layout(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let first = rel.components().next()?;
    let name = first.as_os_str().to_string_lossy().into_owned();
    // A file directly in the root has no game folder.
    (rel.components().count() > 1).then_some(name)
}

/// Parse `{"version":1,"markers":[1.0,2.5]}` written by the recorder.
pub fn read_markers(clip: &Path) -> Vec<f64> {
    let Ok(text) = std::fs::read_to_string(fsops::sidecar(clip)) else {
        return Vec::new();
    };
    let Some(start) = text
        .find("\"markers\"")
        .and_then(|i| text[i..].find('[').map(|j| i + j + 1))
    else {
        return Vec::new();
    };
    let Some(len) = text[start..].find(']') else {
        return Vec::new();
    };
    text[start..start + len]
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect()
}

/// Probe a file and add/refresh it in the library; creates its thumbnail.
pub fn index_file(
    lib: &Library,
    path: &Path,
    game: Option<String>,
    thumbs_dir: &Path,
) -> Result<i64> {
    hc_encode::mf::init_thread();
    let meta = std::fs::metadata(path)?;
    let created = meta
        .created()
        .or_else(|_| meta.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let kind = kind_of(path);
    let mut item = NewItem {
        path: path.to_path_buf(),
        kind: Some(kind),
        game,
        created_at: created,
        size_bytes: meta.len(),
        markers: read_markers(path),
        ..Default::default()
    };
    if kind != Kind::Screenshot {
        match hc_encode::probe::probe(path) {
            Ok(r) => {
                item.duration_ms = (r.duration_ticks / 10_000).max(0) as u64;
                if let Some(v) = r.streams.iter().find(|s| s.major == "video") {
                    item.width = v.width;
                    item.height = v.height;
                    item.codec = Some(v.subtype.clone());
                }
            }
            Err(e) => tracing::debug!(error = %e, "probe failed"),
        }
    } else if let Ok((w, h, _)) = hc_capture::screenshot::load_image(path) {
        item.width = w;
        item.height = h;
    }
    let id = lib.upsert(&item)?;
    let needs_thumb = lib
        .get(id)?
        .is_some_and(|i| i.thumb.as_ref().is_none_or(|t| !t.exists()));
    if needs_thumb {
        let dest = thumbs::thumb_path(thumbs_dir, id);
        match thumbs::make(path, kind, item.duration_ms, &dest) {
            Ok(()) => lib.set_thumb(id, &dest)?,
            Err(e) => tracing::debug!(error = %e, "thumbnail failed"),
        }
    }
    Ok(id)
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth < MAX_DEPTH => walk(&p, depth + 1, out),
            Ok(t) if t.is_file() && is_media(&p) => out.push(p),
            _ => {}
        }
    }
}

/// Add new files from `roots`, drop entries whose file disappeared, and fill
/// in missing thumbnails. Safe to run while the core indexes new clips.
pub fn scan_folders(lib: &Library, roots: &[PathBuf], thumbs_dir: &Path) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    for (id, path) in lib.all_paths()? {
        if !path.exists() {
            lib.forget(id)?;
            report.removed += 1;
        }
    }
    for root in roots {
        let mut files = Vec::new();
        walk(root, 0, &mut files);
        // Never index our own thumbnails (the cache may sit under a clips root).
        files.retain(|f| !f.starts_with(thumbs_dir));
        for f in files {
            let known = lib.id_for_path(&f)?;
            let thumb_ok = match known {
                Some(id) => lib
                    .get(id)?
                    .is_some_and(|i| i.thumb.as_ref().is_some_and(|t| t.exists())),
                None => false,
            };
            if known.is_some() && thumb_ok {
                continue;
            }
            match index_file(lib, &f, game_from_layout(root, &f), thumbs_dir) {
                Ok(_) if known.is_none() => report.added += 1,
                Ok(_) => report.thumbnails += 1,
                Err(e) => {
                    tracing::warn!(error = %e, path = %hc_core::paths::redact(&f), "cannot index");
                    report.errors += 1;
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_layout() {
        assert_eq!(kind_of(Path::new("a/G 2026 REC.mp4")), Kind::Recording);
        assert_eq!(kind_of(Path::new("a/G 2026.mp4")), Kind::Clip);
        assert_eq!(kind_of(Path::new("a/x.WEBP")), Kind::Screenshot);
        let root = Path::new(r"C:\Clips");
        assert_eq!(
            game_from_layout(root, Path::new(r"C:\Clips\CS2\Screenshots\a.png")).as_deref(),
            Some("CS2")
        );
        assert_eq!(game_from_layout(root, Path::new(r"C:\Clips\a.mp4")), None);
    }

    #[test]
    fn scan_adds_images_and_removes_missing() {
        let dir = std::env::temp_dir().join(format!("hc-scan-{}", std::process::id()));
        let game = dir.join("MyGame").join("Screenshots");
        std::fs::create_dir_all(&game).unwrap();
        let img = game.join("shot.png");
        let px: Vec<u8> = (0..64 * 36)
            .flat_map(|i| [(i % 255) as u8, 80, 160, 255])
            .collect();
        hc_encode::mf::init_thread();
        hc_capture::screenshot::save_pixels(
            64,
            36,
            64 * 4,
            &px,
            &img,
            hc_capture::screenshot::ImageFormat::Png,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        let lib = Library::open_in_memory().unwrap();
        let thumbs = dir.join("thumbs");
        let r = scan_folders(&lib, std::slice::from_ref(&dir), &thumbs).unwrap();
        assert_eq!(r.added, 1);
        let items = lib
            .list(&crate::Filter::default(), crate::Sort::Newest, 10, 0)
            .unwrap();
        assert_eq!(items[0].game.as_deref(), Some("MyGame"));
        assert_eq!(items[0].kind, Kind::Screenshot);
        assert_eq!((items[0].width, items[0].height), (64, 36));
        assert!(items[0].thumb.as_ref().is_some_and(|t| t.exists()));
        // Second scan is a no-op.
        assert_eq!(
            scan_folders(&lib, std::slice::from_ref(&dir), &thumbs).unwrap(),
            ScanReport::default()
        );
        std::fs::remove_file(&img).unwrap();
        assert_eq!(
            scan_folders(&lib, std::slice::from_ref(&dir), &thumbs).unwrap().removed,
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn markers_sidecar() {
        let p = std::env::temp_dir().join(format!("hc-mk-{}.mp4", std::process::id()));
        std::fs::write(
            fsops::sidecar(&p),
            "{\"version\":1,\"markers\":[1.500,12.000]}\n",
        )
        .unwrap();
        assert_eq!(read_markers(&p), vec![1.5, 12.0]);
        std::fs::remove_file(fsops::sidecar(&p)).unwrap();
    }
}
