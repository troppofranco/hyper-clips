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
}

pub struct Indexer {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Indexer {
    pub fn start(paths: &AppPaths) -> Self {
        let (tx, rx) = unbounded::<Job>();
        let db = paths.library_db();
        let thumbs = paths.thumbs_dir();
        let thread = std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                hc_encode::mf::init_thread();
                let mut lib: Option<Library> = None;
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
                    let Some(l) = lib.as_ref() else { continue };
                    match hc_library::index_file(l, &job.path, job.game, &thumbs) {
                        Ok(id) => tracing::info!(id, "indexed"),
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
