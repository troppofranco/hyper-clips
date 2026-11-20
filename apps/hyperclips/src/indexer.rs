//! Background indexer: adds saved clips/screenshots to the library
//! (metadata, thumbnail, markers) and applies the storage quota.

use std::path::PathBuf;
use std::thread::JoinHandle;

use crossbeam_channel::{Sender, unbounded};
use hc_core::paths::AppPaths;
use hc_library::Library;

pub struct Job {
    pub path: PathBuf,
    pub game: Option<String>,
    /// Storage limit to enforce afterwards, if any.
    pub limit_bytes: Option<u64>,
    /// Run local highlight detection with this margin (dB).
    pub highlights_margin_db: Option<u32>,
    /// Run enabled plugins (ids listed here are switched off).
    pub plugins: Option<Vec<String>>,
}

pub struct Indexer {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Indexer {
    pub fn start(paths: &AppPaths, notify: Box<dyn Fn(String) + Send>) -> Self {
        let (tx, rx) = unbounded::<Job>();
        let db = paths.library_db();
        let thumbs = paths.thumbs_dir();
        let plugins_dir = paths.data_dir.join("plugins");
        let thread = std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                hc_encode::mf::init_thread();
                let mut lib: Option<Library> = None;
                let mut plugins: Option<(Vec<String>, hc_plugins::PluginHost)> = None;
                for job in rx {
                    if lib.is_none() {
                        match Library::open(&db) {
                            Ok(l) => lib = Some(l),
                            Err(e) => {
                                tracing::error!(error = %e, "library unavailable");
                                continue;
                            }
                        }
                    }
                    let Some(l) = lib.as_mut() else { continue };
                    match hc_library::index_file(l, &job.path, job.game, &thumbs) {
                        Ok(id) => {
                            tracing::info!(id, "indexed");
                            if let Some(margin) = job.highlights_margin_db {
                                find_highlights(l, id, margin);
                            }
                            if let Some(disabled) = &job.plugins {
                                if plugins.as_ref().is_none_or(|(d, _)| d != disabled) {
                                    plugins = Some((
                                        disabled.clone(),
                                        load_plugins(&plugins_dir, disabled),
                                    ));
                                }
                                if let Some((_, host)) = plugins.as_mut() {
                                    run_plugins(host, l, id, &*notify);
                                }
                            } else {
                                plugins = None; // free the interpreters when switched off
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "indexing failed"),
                    }
                    if let Some(limit) = job.limit_bytes {
                        match l.enforce_quota(limit, false) {
                            Ok(removed) if !removed.is_empty() => {
                                tracing::info!(
                                    count = removed.len(),
                                    "storage limit: moved old clips to the Recycle Bin"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!(error = %e, "quota enforcement failed"),
                        }
                    }
                }
            })
            .ok();
        Self {
            tx: Some(tx),
            thread,
        }
    }

    pub fn submit(&self, job: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        // Closing the channel lets the thread finish queued jobs and exit.
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Local highlight detection on a freshly indexed video.
fn find_highlights(lib: &mut Library, id: i64, margin_db: u32) {
    let Ok(Some(item)) = lib.get(id) else { return };
    if item.kind == hc_library::Kind::Screenshot || item.duration_ms == 0 {
        return;
    }
    let params = hc_editor::highlight::Params {
        margin_db: margin_db as f32,
        ..Default::default()
    };
    let t = std::time::Instant::now();
    match hc_editor::highlight::detect_file(&item.path, item.duration_ms, &params) {
        Ok(found) => {
            let secs: Vec<f64> = found.iter().map(|h| h.at_secs).collect();
            match lib.add_highlights(id, &secs) {
                Ok(added) => tracing::info!(
                    id,
                    found = found.len(),
                    added,
                    ms = t.elapsed().as_millis() as u64,
                    "highlights"
                ),
                Err(e) => tracing::warn!(error = %e, "cannot store highlights"),
            }
        }
        Err(e) => tracing::warn!(error = %e, "highlight detection failed"),
    }
}
fn load_plugins(dir: &std::path::Path, disabled: &[String]) -> hc_plugins::PluginHost {
    let mut host = hc_plugins::PluginHost::default();
    for m in hc_plugins::discover(dir).into_iter().flatten() {
        if disabled.contains(&m.id) {
            continue;
        }
        match host.load(&dir.join(&m.id)) {
            Ok(id) => tracing::info!(plugin = %id, "plugin loaded"),
            Err(e) => tracing::warn!(plugin = %m.id, error = %e, "plugin rejected"),
        }
    }
    host
}

fn run_plugins(
    host: &mut hc_plugins::PluginHost,
    lib: &mut Library,
    id: i64,
    notify: &dyn Fn(String),
) {
    let Ok(Some(item)) = lib.get(id) else { return };
    let event = hc_plugins::ClipEvent {
        path: item.path.clone(),
        game: item.game.clone(),
        duration_s: item.duration_ms as f64 / 1000.0,
        kind: match item.kind {
            hc_library::Kind::Clip => hc_plugins::ClipKind::Clip,
            hc_library::Kind::Recording => hc_plugins::ClipKind::Recording,
            hc_library::Kind::Screenshot => hc_plugins::ClipKind::Screenshot,
        },
        markers: lib.markers(id).unwrap_or_default(),
    };
    let mut tags = item.tags.clone();
    for (pid, res) in host.on_clip_saved(&event) {
        match res {
            Ok(out) => {
                for t in out.tags {
                    if !tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                        tags.push(t);
                    }
                }
                for n in out.notifications {
                    notify(n);
                }
            }
            Err(e) => tracing::warn!(plugin = %pid, error = %e, "plugin failed and was disabled"),
        }
    }
    if tags != item.tags
        && let Err(e) = lib.set_tags(id, &tags)
    {
        tracing::warn!(error = %e, "cannot store plugin tags");
    }
}
