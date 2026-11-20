//! Background export queue shared by the studio (UI) and its worker process.
//!
//! Each job is `<id>.job.json` (project + output path) in the queue folder;
//! the worker writes `<id>.status.json`; a `<id>.cancel` file cancels it.
//! Exports therefore keep running after the studio window is closed.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::project::Project;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub project: Project,
    pub output: PathBuf,
    /// Library database to index the result into.
    pub library_db: Option<PathBuf>,
    pub thumbs_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub progress: f32,
    pub pass: u32,
    pub message: String,
    pub bytes: u64,
}

pub fn job_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.job.json"))
}
pub fn status_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.status.json"))
}
pub fn cancel_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.cancel"))
}

fn write_status(dir: &Path, id: &str, s: &Status) {
    let p = status_path(dir, id);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_vec(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn read_status(dir: &Path, id: &str) -> Option<Status> {
    serde_json::from_slice(&std::fs::read(status_path(dir, id)).ok()?).ok()
}

pub fn enqueue(dir: &Path, job: &Job) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        job_path(dir, &job.id),
        serde_json::to_vec_pretty(job).map_err(std::io::Error::other)?,
    )?;
    write_status(
        dir,
        &job.id,
        &Status {
            state: State::Queued,
            progress: 0.0,
            pass: 0,
            message: String::new(),
            bytes: 0,
        },
    );
    Ok(())
}

/// All jobs in the folder, oldest first: (job, status).
pub fn list(dir: &Path) -> Vec<(Job, Status)> {
    let mut out: Vec<(Job, Status, std::time::SystemTime)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.to_string_lossy().ends_with(".job.json") {
            continue;
        }
        let Ok(job) = serde_json::from_slice::<Job>(&std::fs::read(&p).unwrap_or_default()) else {
            continue;
        };
        let st = read_status(dir, &job.id).unwrap_or(Status {
            state: State::Queued,
            progress: 0.0,
            pass: 0,
            message: String::new(),
            bytes: 0,
        });
        let t = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        out.push((job, st, t));
    }
    out.sort_by_key(|(_, _, t)| *t);
    out.into_iter().map(|(j, s, _)| (j, s)).collect()
}

pub fn remove(dir: &Path, id: &str) {
    for p in [
        job_path(dir, id),
        status_path(dir, id),
        cancel_path(dir, id),
    ] {
        let _ = std::fs::remove_file(p);
    }
}

/// Worker entry point: run every queued job in `dir`, one at a time.
/// `index` is called with the finished file (library indexing).
pub fn run_worker(dir: &Path, index: &dyn Fn(&Job, &Path)) {
    loop {
        let next = list(dir)
            .into_iter()
            .find(|(_, s)| s.state == State::Queued);
        let Some((job, _)) = next else { break };
        let id = job.id.clone();
        write_status(
            dir,
            &id,
            &Status {
                state: State::Running,
                progress: 0.0,
                pass: 1,
                message: String::new(),
                bytes: 0,
            },
        );
        let cancel = AtomicBool::new(false);
        let cancel_file = cancel_path(dir, &id);
        let mut last_write = std::time::Instant::now();
        let result = crate::render::export(
            &job.project,
            &job.output,
            &mut |f, pass| {
                if cancel_file.exists() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if last_write.elapsed().as_millis() > 250 {
                    write_status(
                        dir,
                        &id,
                        &Status {
                            state: State::Running,
                            progress: f,
                            pass,
                            message: String::new(),
                            bytes: 0,
                        },
                    );
                    last_write = std::time::Instant::now();
                }
            },
            &cancel,
        );
        let status = match result {
            Ok(r) => {
                index(&job, &r.path);
                Status {
                    state: State::Done,
                    progress: 1.0,
                    pass: r.passes,
                    message: format!("{}x{} · {}", r.width, r.height, r.encoder),
                    bytes: r.bytes,
                }
            }
            Err(e) if e == "cancelled" => Status {
                state: State::Cancelled,
                progress: 0.0,
                pass: 0,
                message: e,
                bytes: 0,
            },
            Err(e) => {
                tracing::error!(error = %e, "export failed");
                Status {
                    state: State::Failed,
                    progress: 0.0,
                    pass: 0,
                    message: e,
                    bytes: 0,
                }
            }
        };
        write_status(dir, &id, &status);
        let _ = std::fs::remove_file(&cancel_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_list_remove() {
        let dir = std::env::temp_dir().join(format!("hc-queue-{}", std::process::id()));
        let job = Job {
            id: "a1".into(),
            project: Project::default(),
            output: "x.mp4".into(),
            library_db: None,
            thumbs_dir: None,
        };
        enqueue(&dir, &job).unwrap();
        let l = list(&dir);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].1.state, State::Queued);
        // An empty project fails fast and is reported as failed.
        run_worker(&dir, &|_, _| {});
        assert_eq!(read_status(&dir, "a1").unwrap().state, State::Failed);
        remove(&dir, "a1");
        assert!(list(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
