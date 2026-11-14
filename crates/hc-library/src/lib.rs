//! Clip library stored in a local SQLite file. No network, no telemetry.

use hc_core::error::{Error, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

fn db<E: std::fmt::Display>(e: E) -> Error {
    Error::Database(e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClip {
    pub path: String,
    /// Unix seconds.
    pub created_at: i64,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub game: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub id: i64,
    pub path: String,
    pub created_at: i64,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub game: String,
    pub favorite: bool,
    pub tags: Vec<String>,
    /// Bookmarked moments, in milliseconds from the start of the clip.
    pub bookmarks: Vec<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Exact game name; empty = any.
    pub game: String,
    pub favorites_only: bool,
    /// Clip must carry this tag; empty = any.
    pub tag: String,
    /// Substring of the file name; empty = any.
    pub text: String,
    /// 0 = no limit.
    pub limit: u32,
}

/// Rules for automatic clean-up. A zero value disables that rule.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoDelete {
    pub max_age_days: u32,
    pub max_total_bytes: u64,
    pub keep_favorites: bool,
}

pub struct Library {
    conn: Connection,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path).map_err(db)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS clips (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 duration_ms INTEGER NOT NULL DEFAULT 0,
                 size_bytes INTEGER NOT NULL DEFAULT 0,
                 game TEXT NOT NULL DEFAULT '',
                 favorite INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS clip_tags (
                 clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                 tag TEXT NOT NULL,
                 PRIMARY KEY (clip_id, tag)
             );
             CREATE TABLE IF NOT EXISTS clip_bookmarks (
                 clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                 ms INTEGER NOT NULL,
                 PRIMARY KEY (clip_id, ms)
             );
             CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_at);",
        )
        .map_err(db)?;
        Ok(Self { conn })
    }

    /// Adds a clip, or returns the id of the one already stored for that path.
    pub fn add(&self, clip: &NewClip) -> Result<i64> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO clips (path, created_at, duration_ms, size_bytes, game)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    clip.path,
                    clip.created_at,
                    clip.duration_ms,
                    clip.size_bytes,
                    clip.game
                ],
            )
            .map_err(db)?;
        self.conn
            .query_row("SELECT id FROM clips WHERE path = ?1", [&clip.path], |r| {
                r.get(0)
            })
            .map_err(db)
    }

    pub fn get(&self, id: i64) -> Result<Option<Clip>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, path, created_at, duration_ms, size_bytes, game, favorite
                 FROM clips WHERE id = ?1",
                [id],
                row_to_clip,
            )
            .optional()
            .map_err(db)?;
        match row {
            Some(mut clip) => {
                clip.tags = self.tags_of(clip.id)?;
                clip.bookmarks = self.bookmarks_of(clip.id)?;
                Ok(Some(clip))
            }
            None => Ok(None),
        }
    }

    /// Newest first.
    pub fn list(&self, filter: &Filter) -> Result<Vec<Clip>> {
        let mut sql = String::from(
            "SELECT id, path, created_at, duration_ms, size_bytes, game, favorite FROM clips WHERE 1=1",
        );
        let mut args: Vec<String> = Vec::new();
        if !filter.game.is_empty() {
            sql.push_str(" AND game = ?");
            args.push(filter.game.clone());
        }
        if filter.favorites_only {
            sql.push_str(" AND favorite = 1");
        }
        if !filter.tag.is_empty() {
            sql.push_str(" AND id IN (SELECT clip_id FROM clip_tags WHERE tag = ?)");
            args.push(filter.tag.clone());
        }
        if !filter.text.is_empty() {
            // Escape LIKE wildcards in user input.
            let escaped = filter
                .text
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            sql.push_str(" AND path LIKE ? ESCAPE '\\'");
            args.push(format!("%{escaped}%"));
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC");
        if filter.limit > 0 {
            sql.push_str(&format!(" LIMIT {}", filter.limit));
        }
        let mut stmt = self.conn.prepare(&sql).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), row_to_clip)
            .map_err(db)?;
        let mut clips = Vec::new();
        for row in rows {
            let mut clip = row.map_err(db)?;
            clip.tags = self.tags_of(clip.id)?;
            clip.bookmarks = self.bookmarks_of(clip.id)?;
            clips.push(clip);
        }
        Ok(clips)
    }

    fn tags_of(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag FROM clip_tags WHERE clip_id = ?1 ORDER BY tag")
            .map_err(db)?;
        let rows = stmt
            .query_map([id], |r| r.get::<_, String>(0))
            .map_err(db)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db)
    }

    pub fn add_bookmark(&self, id: i64, ms: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO clip_bookmarks (clip_id, ms) VALUES (?1, ?2)",
                params![id, ms.max(0)],
            )
            .map_err(db)?;
        Ok(())
    }

    fn bookmarks_of(&self, id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ms FROM clip_bookmarks WHERE clip_id = ?1 ORDER BY ms")
            .map_err(db)?;
        let rows = stmt.query_map([id], |r| r.get::<_, i64>(0)).map_err(db)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db)
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE clips SET favorite = ?2 WHERE id = ?1",
                params![id, favorite],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Tags are trimmed and lower-cased; empty tags are rejected.
    pub fn add_tag(&self, id: i64, tag: &str) -> Result<()> {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() {
            return Err(Error::Invalid("empty tag".into()));
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO clip_tags (clip_id, tag) VALUES (?1, ?2)",
                params![id, tag],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn remove_tag(&self, id: i64, tag: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM clip_tags WHERE clip_id = ?1 AND tag = ?2",
                params![id, tag.trim().to_lowercase()],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Forgets a clip (the file is not touched).
    pub fn remove(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM clips WHERE id = ?1", [id])
            .map_err(db)?;
        Ok(())
    }

    pub fn games(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT game FROM clips WHERE game <> '' ORDER BY game")
            .map_err(db)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db)
    }

    /// Imports every `.mp4` in `dir` that is not stored yet; returns how many were added.
    pub fn scan_folder(&self, dir: &Path, now_secs: i64) -> Result<usize> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(0);
        };
        let mut added = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let is_mp4 = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
            if !is_mp4 {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let created = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(now_secs, |d| d.as_secs() as i64);
            let key = path.to_string_lossy().into_owned();
            let known: bool = self
                .conn
                .query_row("SELECT 1 FROM clips WHERE path = ?1", [&key], |_| Ok(true))
                .optional()
                .map_err(db)?
                .unwrap_or(false);
            if !known {
                self.add(&NewClip {
                    path: key,
                    created_at: created,
                    duration_ms: 0,
                    size_bytes: meta.len() as i64,
                    game: String::new(),
                })?;
                added += 1;
            }
        }
        Ok(added)
    }

    /// Drops rows whose file no longer exists; returns how many.
    pub fn prune_missing(&self) -> Result<usize> {
        let all = self.list(&Filter::default())?;
        let mut removed = 0;
        for clip in all {
            if !PathBuf::from(&clip.path).exists() {
                self.remove(clip.id)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Chooses the clips to delete under `rules` (oldest first). Returns `(id, path)` pairs;
    /// the caller deletes the files and then calls [`Library::remove`].
    pub fn auto_delete_candidates(
        &self,
        rules: AutoDelete,
        now_secs: i64,
    ) -> Result<Vec<(i64, String)>> {
        let mut clips = self.list(&Filter::default())?;
        clips.retain(|c| !(rules.keep_favorites && c.favorite));
        clips.reverse(); // oldest first
        let mut out: Vec<(i64, String)> = Vec::new();
        let mut taken = std::collections::HashSet::new();

        if rules.max_age_days > 0 {
            let cutoff = now_secs - i64::from(rules.max_age_days) * 86_400;
            for c in clips.iter().filter(|c| c.created_at < cutoff) {
                taken.insert(c.id);
                out.push((c.id, c.path.clone()));
            }
        }
        if rules.max_total_bytes > 0 {
            let all = self.list(&Filter::default())?;
            let mut total: u64 = all
                .iter()
                .filter(|c| !taken.contains(&c.id))
                .map(|c| c.size_bytes.max(0) as u64)
                .sum();
            for c in clips.iter().filter(|c| !taken.contains(&c.id)) {
                if total <= rules.max_total_bytes {
                    break;
                }
                total = total.saturating_sub(c.size_bytes.max(0) as u64);
                out.push((c.id, c.path.clone()));
            }
        }
        Ok(out)
    }
}

fn row_to_clip(r: &rusqlite::Row<'_>) -> rusqlite::Result<Clip> {
    Ok(Clip {
        id: r.get(0)?,
        path: r.get(1)?,
        created_at: r.get(2)?,
        duration_ms: r.get(3)?,
        size_bytes: r.get(4)?,
        game: r.get(5)?,
        favorite: r.get::<_, i64>(6)? != 0,
        tags: Vec::new(),
        bookmarks: Vec::new(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn clip(path: &str, at: i64, size: i64, game: &str) -> NewClip {
        NewClip {
            path: path.into(),
            created_at: at,
            duration_ms: 1000,
            size_bytes: size,
            game: game.into(),
        }
    }

    #[test]
    fn add_is_idempotent_per_path() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.add(&clip("a.mp4", 1, 10, "")).unwrap();
        let b = lib.add(&clip("a.mp4", 2, 99, "x")).unwrap();
        assert_eq!(a, b);
        assert_eq!(lib.list(&Filter::default()).unwrap().len(), 1);
    }

    #[test]
    fn filters_by_game_favorite_tag_and_text() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.add(&clip("C:/v/ace.mp4", 1, 10, "Valorant")).unwrap();
        let b = lib.add(&clip("C:/v/fail.mp4", 2, 10, "CS2")).unwrap();
        lib.set_favorite(a, true).unwrap();
        lib.add_tag(b, "  Funny ").unwrap();

        let fav = lib
            .list(&Filter {
                favorites_only: true,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(fav.len(), 1);
        assert_eq!(fav[0].id, a);
        let game = lib
            .list(&Filter {
                game: "CS2".into(),
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(game[0].id, b);
        let tag = lib
            .list(&Filter {
                tag: "funny".into(),
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(tag[0].tags, vec!["funny".to_string()]);
        let text = lib
            .list(&Filter {
                text: "ace".into(),
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(text[0].id, a);
        assert_eq!(
            lib.games().unwrap(),
            vec!["CS2".to_string(), "Valorant".to_string()]
        );
    }

    #[test]
    fn like_wildcards_in_text_are_literal() {
        let lib = Library::open_in_memory().unwrap();
        lib.add(&clip("a_b.mp4", 1, 1, "")).unwrap();
        lib.add(&clip("axb.mp4", 2, 1, "")).unwrap();
        let r = lib
            .list(&Filter {
                text: "a_b".into(),
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn empty_tag_is_rejected_and_removal_works() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.add(&clip("a.mp4", 1, 1, "")).unwrap();
        assert!(lib.add_tag(id, "   ").is_err());
        lib.add_tag(id, "x").unwrap();
        lib.remove_tag(id, "X").unwrap();
        assert!(lib.get(id).unwrap().unwrap().tags.is_empty());
    }

    #[test]
    fn newest_first_with_limit() {
        let lib = Library::open_in_memory().unwrap();
        lib.add(&clip("old.mp4", 1, 1, "")).unwrap();
        lib.add(&clip("new.mp4", 9, 1, "")).unwrap();
        let r = lib
            .list(&Filter {
                limit: 1,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(r[0].path, "new.mp4");
    }

    #[test]
    fn auto_delete_by_age_keeps_favorites() {
        let lib = Library::open_in_memory().unwrap();
        let old = lib.add(&clip("old.mp4", 0, 1, "")).unwrap();
        let fav = lib.add(&clip("fav.mp4", 0, 1, "")).unwrap();
        lib.add(&clip("new.mp4", 100 * 86_400, 1, "")).unwrap();
        lib.set_favorite(fav, true).unwrap();
        let rules = AutoDelete {
            max_age_days: 30,
            max_total_bytes: 0,
            keep_favorites: true,
        };
        let c = lib.auto_delete_candidates(rules, 100 * 86_400).unwrap();
        assert_eq!(c.iter().map(|x| x.0).collect::<Vec<_>>(), vec![old]);
    }

    #[test]
    fn auto_delete_by_size_removes_oldest_first() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.add(&clip("a.mp4", 1, 60, "")).unwrap();
        lib.add(&clip("b.mp4", 2, 60, "")).unwrap();
        lib.add(&clip("c.mp4", 3, 60, "")).unwrap();
        let rules = AutoDelete {
            max_age_days: 0,
            max_total_bytes: 130,
            keep_favorites: false,
        };
        let c = lib.auto_delete_candidates(rules, 10).unwrap();
        assert_eq!(c.iter().map(|x| x.0).collect::<Vec<_>>(), vec![a]);
    }

    #[test]
    fn bookmarks_are_stored_sorted_and_deduplicated() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.add(&clip("a.mp4", 1, 1, "")).unwrap();
        lib.add_bookmark(id, 5000).unwrap();
        lib.add_bookmark(id, 1000).unwrap();
        lib.add_bookmark(id, 5000).unwrap();
        assert_eq!(lib.get(id).unwrap().unwrap().bookmarks, vec![1000, 5000]);
    }

    #[test]
    fn remove_cascades_tags() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.add(&clip("a.mp4", 1, 1, "")).unwrap();
        lib.add_tag(id, "t").unwrap();
        lib.remove(id).unwrap();
        assert!(lib.get(id).unwrap().is_none());
    }
}
