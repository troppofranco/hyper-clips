//! Local clip library: a SQLite index of clips, recordings and screenshots
//! with tags, favourites, notes, markers, thumbnails and a storage quota.
//!
//! The database lives in the app's data folder; nothing is synced anywhere.
//! Both the core (indexing new clips) and the studio (browsing/editing)
//! open it; WAL mode makes concurrent access safe.

pub mod fsops;
pub mod scan;
pub mod thumbs;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

pub use scan::{ScanReport, index_file, scan_folders};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("file system: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, LibraryError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Clip,
    Recording,
    Screenshot,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Clip => "clip",
            Kind::Recording => "recording",
            Kind::Screenshot => "screenshot",
        }
    }

    pub fn parse(s: &str) -> Kind {
        match s {
            "recording" => Kind::Recording,
            "screenshot" => Kind::Screenshot,
            _ => Kind::Clip,
        }
    }
}

/// Metadata for indexing a file.
#[derive(Debug, Clone, Default)]
pub struct NewItem {
    pub path: PathBuf,
    pub kind: Option<Kind>,
    pub game: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub codec: Option<String>,
    /// Marker positions in seconds.
    pub markers: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub path: PathBuf,
    pub kind: Kind,
    pub game: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub codec: Option<String>,
    pub favorite: bool,
    pub notes: String,
    pub thumb: Option<PathBuf>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Longest,
    Shortest,
    Largest,
    Title,
}

impl Sort {
    fn sql(self) -> &'static str {
        match self {
            Sort::Newest => "c.created_at DESC, c.id DESC",
            Sort::Oldest => "c.created_at ASC, c.id ASC",
            Sort::Longest => "c.duration_ms DESC",
            Sort::Shortest => "c.duration_ms ASC",
            Sort::Largest => "c.size_bytes DESC",
            Sort::Title => "c.title COLLATE NOCASE ASC",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Matches title, game, notes and tags (case-insensitive).
    pub text: String,
    pub game: Option<String>,
    pub tag: Option<String>,
    pub favorites_only: bool,
    pub kind: Option<Kind>,
    pub min_duration_ms: Option<u64>,
    pub max_duration_ms: Option<u64>,
    /// Unix seconds, inclusive.
    pub from: Option<i64>,
    pub to: Option<i64>,
}

const SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS clips (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL DEFAULT 'clip',
    game        TEXT,
    title       TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    width       INTEGER NOT NULL DEFAULT 0,
    height      INTEGER NOT NULL DEFAULT 0,
    codec       TEXT,
    favorite    INTEGER NOT NULL DEFAULT 0,
    notes       TEXT NOT NULL DEFAULT '',
    thumb       TEXT
);
CREATE INDEX IF NOT EXISTS clips_created ON clips(created_at);
CREATE INDEX IF NOT EXISTS clips_game ON clips(game);
CREATE TABLE IF NOT EXISTS tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE IF NOT EXISTS clip_tags (
    clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (clip_id, tag_id)
);
CREATE TABLE IF NOT EXISTS markers (
    clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
    t_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS markers_clip ON markers(clip_id);
";

pub struct Library {
    conn: Connection,
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn title_from_path(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Escape `%`, `_` and `\` for a LIKE pattern with ESCAPE '\'.
fn like_pattern(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

impl Library {
    pub fn open(db: &Path) -> Result<Self> {
        if let Some(dir) = db.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(db)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self { conn })
    }

    /// Insert or refresh a file's technical metadata (user edits are kept).
    pub fn upsert(&self, it: &NewItem) -> Result<i64> {
        let kind = it.kind.unwrap_or(Kind::Clip).as_str();
        self.conn.execute(
            "INSERT INTO clips (path, kind, game, title, created_at, duration_ms, size_bytes, width, height, codec)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(path) DO UPDATE SET
                 duration_ms = excluded.duration_ms,
                 size_bytes  = excluded.size_bytes,
                 width       = excluded.width,
                 height      = excluded.height,
                 codec       = excluded.codec,
                 game        = COALESCE(clips.game, excluded.game)",
            params![
                path_str(&it.path),
                kind,
                it.game,
                title_from_path(&it.path),
                it.created_at,
                it.duration_ms as i64,
                it.size_bytes as i64,
                it.width,
                it.height,
                it.codec,
            ],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM clips WHERE path = ?1",
            [path_str(&it.path)],
            |r| r.get(0),
        )?;
        if !it.markers.is_empty() && self.markers(id)?.is_empty() {
            self.set_markers(id, &it.markers)?;
        }
        Ok(id)
    }

    pub fn id_for_path(&self, path: &Path) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM clips WHERE path = ?1",
                [path_str(path)],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn tags_of(&self, id: i64) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached(
            "SELECT t.name FROM tags t JOIN clip_tags ct ON ct.tag_id = t.id
             WHERE ct.clip_id = ?1 ORDER BY t.name COLLATE NOCASE",
        )?;
        let rows = st.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn row_to_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
        Ok(Item {
            id: r.get(0)?,
            path: PathBuf::from(r.get::<_, String>(1)?),
            kind: Kind::parse(&r.get::<_, String>(2)?),
            game: r.get(3)?,
            title: r.get(4)?,
            created_at: r.get(5)?,
            duration_ms: r.get::<_, i64>(6)? as u64,
            size_bytes: r.get::<_, i64>(7)? as u64,
            width: r.get(8)?,
            height: r.get(9)?,
            codec: r.get(10)?,
            favorite: r.get::<_, i64>(11)? != 0,
            notes: r.get(12)?,
            thumb: r.get::<_, Option<String>>(13)?.map(PathBuf::from),
            tags: Vec::new(),
        })
    }

    const COLUMNS: &str = "c.id, c.path, c.kind, c.game, c.title, c.created_at, c.duration_ms,
        c.size_bytes, c.width, c.height, c.codec, c.favorite, c.notes, c.thumb";

    pub fn get(&self, id: i64) -> Result<Option<Item>> {
        let sql = format!("SELECT {} FROM clips c WHERE c.id = ?1", Self::COLUMNS);
        let item = self
            .conn
            .query_row(&sql, [id], Self::row_to_item)
            .optional()?;
        match item {
            Some(mut it) => {
                it.tags = self.tags_of(id)?;
                Ok(Some(it))
            }
            None => Ok(None),
        }
    }

    fn where_clause(f: &Filter) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut conds = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        if !f.text.trim().is_empty() {
            args.push(Value::Text(like_pattern(f.text.trim())));
            let n = args.len();
            conds.push(format!(
                "(c.title LIKE ?{n} ESCAPE '\\' OR c.game LIKE ?{n} ESCAPE '\\' OR c.notes LIKE ?{n} ESCAPE '\\'
                  OR EXISTS (SELECT 1 FROM clip_tags ct JOIN tags t ON t.id = ct.tag_id
                             WHERE ct.clip_id = c.id AND t.name LIKE ?{n} ESCAPE '\\'))"
            ));
        }
        if let Some(g) = &f.game {
            args.push(Value::Text(g.clone()));
            conds.push(format!("c.game = ?{}", args.len()));
        }
        if let Some(t) = &f.tag {
            args.push(Value::Text(t.clone()));
            conds.push(format!(
                "EXISTS (SELECT 1 FROM clip_tags ct JOIN tags t ON t.id = ct.tag_id
                         WHERE ct.clip_id = c.id AND t.name = ?{} COLLATE NOCASE)",
                args.len()
            ));
        }
        if f.favorites_only {
            conds.push("c.favorite = 1".into());
        }
        if let Some(k) = f.kind {
            args.push(Value::Text(k.as_str().into()));
            conds.push(format!("c.kind = ?{}", args.len()));
        }
        if let Some(v) = f.min_duration_ms {
            args.push(Value::Integer(v as i64));
            conds.push(format!("c.duration_ms >= ?{}", args.len()));
        }
        if let Some(v) = f.max_duration_ms {
            args.push(Value::Integer(v as i64));
            conds.push(format!("c.duration_ms <= ?{}", args.len()));
        }
        if let Some(v) = f.from {
            args.push(Value::Integer(v));
            conds.push(format!("c.created_at >= ?{}", args.len()));
        }
        if let Some(v) = f.to {
            args.push(Value::Integer(v));
            conds.push(format!("c.created_at <= ?{}", args.len()));
        }
        let clause = if conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conds.join(" AND "))
        };
        (clause, args)
    }

    pub fn list(&self, f: &Filter, sort: Sort, limit: usize, offset: usize) -> Result<Vec<Item>> {
        let (clause, args) = Self::where_clause(f);
        let sql = format!(
            "SELECT {} FROM clips c {clause} ORDER BY {} LIMIT {limit} OFFSET {offset}",
            Self::COLUMNS,
            sort.sql()
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args.iter()), Self::row_to_item)?;
        let mut items: Vec<Item> = rows.collect::<rusqlite::Result<_>>()?;
        for it in &mut items {
            it.tags = self.tags_of(it.id)?;
        }
        Ok(items)
    }

    pub fn count(&self, f: &Filter) -> Result<usize> {
        let (clause, args) = Self::where_clause(f);
        let sql = format!("SELECT COUNT(*) FROM clips c {clause}");
        Ok(self
            .conn
            .query_row(&sql, params_from_iter(args.iter()), |r| r.get::<_, i64>(0))?
            as usize)
    }

    /// (game, number of items), most used first.
    pub fn games(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self.conn.prepare(
            "SELECT game, COUNT(*) FROM clips WHERE game IS NOT NULL GROUP BY game ORDER BY COUNT(*) DESC, game",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self.conn.prepare(
            "SELECT t.name, COUNT(ct.clip_id) FROM tags t LEFT JOIN clip_tags ct ON ct.tag_id = t.id
             GROUP BY t.id HAVING COUNT(ct.clip_id) > 0 ORDER BY t.name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_favorite(&self, id: i64, fav: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET favorite = ?2 WHERE id = ?1",
            params![id, fav as i64],
        )?;
        Ok(())
    }

    pub fn set_notes(&self, id: i64, notes: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET notes = ?2 WHERE id = ?1",
            params![id, notes],
        )?;
        Ok(())
    }

    pub fn set_thumb(&self, id: i64, thumb: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET thumb = ?2 WHERE id = ?1",
            params![id, path_str(thumb)],
        )?;
        Ok(())
    }

    /// Replace the tag set of an item (tags are trimmed, empty ones ignored).
    pub fn set_tags(&mut self, id: i64, tags: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM clip_tags WHERE clip_id = ?1", [id])?;
        for t in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [t])?;
            tx.execute(
                "INSERT OR IGNORE INTO clip_tags (clip_id, tag_id)
                 SELECT ?1, id FROM tags WHERE name = ?2 COLLATE NOCASE",
                params![id, t],
            )?;
        }
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT DISTINCT tag_id FROM clip_tags)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn markers(&self, id: i64) -> Result<Vec<f64>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT t_ms FROM markers WHERE clip_id = ?1 ORDER BY t_ms")?;
        let rows = st.query_map([id], |r| r.get::<_, i64>(0))?;
        Ok(rows
            .map(|r| r.map(|ms| ms as f64 / 1000.0))
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_markers(&self, id: i64, secs: &[f64]) -> Result<()> {
        self.conn
            .execute("DELETE FROM markers WHERE clip_id = ?1", [id])?;
        let mut st = self
            .conn
            .prepare_cached("INSERT INTO markers (clip_id, t_ms) VALUES (?1, ?2)")?;
        for s in secs {
            st.execute(params![id, (s * 1000.0).round() as i64])?;
        }
        Ok(())
    }

    /// Rename: changes the title and the file name on disk (keeping the extension).
    pub fn rename(&self, id: i64, new_title: &str) -> Result<PathBuf> {
        let item = self
            .get(id)?
            .ok_or_else(|| LibraryError::Other("unknown item".into()))?;
        let stem = sanitize(new_title);
        if stem.is_empty() {
            return Err(LibraryError::Other("empty name".into()));
        }
        let dir = item.path.parent().unwrap_or(Path::new("."));
        let ext = item
            .path
            .extension()
            .map(|e| e.to_string_lossy().into_owned());
        let mut target = dir.join(match &ext {
            Some(e) => format!("{stem}.{e}"),
            None => stem.clone(),
        });
        let mut n = 2;
        while target.exists() && target != item.path {
            target = dir.join(match &ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            });
            n += 1;
        }
        if target != item.path {
            std::fs::rename(&item.path, &target)?;
            // Keep a markers sidecar next to its clip.
            let side = fsops::sidecar(&item.path);
            if side.exists() {
                let _ = std::fs::rename(&side, fsops::sidecar(&target));
            }
        }
        self.conn.execute(
            "UPDATE clips SET path = ?2, title = ?3 WHERE id = ?1",
            params![id, path_str(&target), new_title.trim()],
        )?;
        Ok(target)
    }

    /// Remove from the index only (file untouched).
    pub fn forget(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete the file (to the Recycle Bin unless `permanent`) and the entry.
    pub fn delete(&self, id: i64, permanent: bool) -> Result<()> {
        if let Some(item) = self.get(id)? {
            if item.path.exists() {
                fsops::delete_file(&item.path, permanent)?;
            }
            let side = fsops::sidecar(&item.path);
            if side.exists() {
                let _ = fsops::delete_file(&side, permanent);
            }
            if let Some(t) = &item.thumb {
                let _ = std::fs::remove_file(t);
            }
        }
        self.forget(id)
    }

    pub fn total_size(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM clips", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
    }

    /// Delete the oldest non-favourite items until the library fits in
    /// `limit_bytes` (to the Recycle Bin unless `permanent`). Returns what was removed.
    pub fn enforce_quota(&self, limit_bytes: u64, permanent: bool) -> Result<Vec<Item>> {
        let mut removed = Vec::new();
        let mut total = self.total_size()?;
        if total <= limit_bytes {
            return Ok(removed);
        }
        let sql = format!(
            "SELECT {} FROM clips c WHERE c.favorite = 0 ORDER BY c.created_at ASC, c.id ASC",
            Self::COLUMNS
        );
        let candidates: Vec<Item> = {
            let mut st = self.conn.prepare(&sql)?;
            let rows = st.query_map([], Self::row_to_item)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for it in candidates {
            if total <= limit_bytes {
                break;
            }
            self.delete(it.id, permanent)?;
            total = total.saturating_sub(it.size_bytes);
            tracing::info!(path = %hc_core::paths::redact(&it.path), "auto-deleted (storage limit)");
            removed.push(it);
        }
        Ok(removed)
    }

    /// All indexed paths (for reconciliation with the file system).
    pub fn all_paths(&self) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self.conn.prepare("SELECT id, path FROM clips")?;
        let rows = st.query_map([], |r| {
            Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Make a user-provided name safe as a file name.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    cleaned
        .trim()
        .trim_end_matches('.')
        .trim()
        .chars()
        .take(120)
        .collect()
}

/// A group of items for the gallery (by game or by play session).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub label: String,
    /// Indices into the input slice, in input order.
    pub items: Vec<usize>,
}

/// Group by game (input order preserved inside groups).
pub fn group_by_game(items: &[Item]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let label = it.game.clone().unwrap_or_else(|| "Desktop".into());
        match groups.iter_mut().find(|g| g.label == label) {
            Some(g) => g.items.push(i),
            None => groups.push(Group {
                label,
                items: vec![i],
            }),
        }
    }
    groups
}

/// Group into play sessions: same game, consecutive items no more than
/// `gap_secs` apart (items should be sorted by date).
pub fn group_by_session(items: &[Item], gap_secs: i64) -> Vec<Group> {
    let mut groups: Vec<(Option<String>, i64, Group)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let joined = groups
            .iter_mut()
            .rev()
            .find(|(game, last, _)| *game == it.game && (it.created_at - *last).abs() <= gap_secs);
        match joined {
            Some((_, last, g)) => {
                *last = it.created_at;
                g.items.push(i);
            }
            None => groups.push((
                it.game.clone(),
                it.created_at,
                Group {
                    label: it.game.clone().unwrap_or_else(|| "Desktop".into()),
                    items: vec![i],
                },
            )),
        }
    }
    groups.into_iter().map(|(_, _, g)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, game: &str, t: i64, dur: u64, size: u64) -> NewItem {
        NewItem {
            path: PathBuf::from(path),
            kind: Some(Kind::Clip),
            game: Some(game.into()),
            created_at: t,
            duration_ms: dur,
            size_bytes: size,
            width: 1920,
            height: 1080,
            codec: Some("H264".into()),
            markers: vec![],
        }
    }

    fn sample() -> Library {
        let lib = Library::open_in_memory().unwrap();
        lib.upsert(&item(r"C:\c\a.mp4", "CS2", 100, 30_000, 10))
            .unwrap();
        lib.upsert(&item(r"C:\c\b.mp4", "CS2", 200, 60_000, 20))
            .unwrap();
        lib.upsert(&item(r"C:\c\c.mp4", "Rust", 300, 15_000, 30))
            .unwrap();
        lib
    }

    #[test]
    fn upsert_is_idempotent_and_keeps_edits() {
        let lib = sample();
        let id = lib.id_for_path(Path::new(r"C:\c\a.mp4")).unwrap().unwrap();
        lib.set_favorite(id, true).unwrap();
        lib.set_notes(id, "ace").unwrap();
        let again = lib
            .upsert(&item(r"C:\c\a.mp4", "Other", 999, 31_000, 11))
            .unwrap();
        assert_eq!(again, id);
        let it = lib.get(id).unwrap().unwrap();
        assert!(it.favorite);
        assert_eq!(it.notes, "ace");
        assert_eq!(it.duration_ms, 31_000);
        assert_eq!(
            it.game.as_deref(),
            Some("CS2"),
            "game set once, not overwritten"
        );
        assert_eq!(lib.count(&Filter::default()).unwrap(), 3);
    }

    #[test]
    fn filters_and_sorting() {
        let mut lib = sample();
        let c = lib.id_for_path(Path::new(r"C:\c\c.mp4")).unwrap().unwrap();
        lib.set_tags(c, &["Clutch".into(), " funny ".into(), "".into()])
            .unwrap();
        let by_game = Filter {
            game: Some("CS2".into()),
            ..Default::default()
        };
        assert_eq!(lib.list(&by_game, Sort::Newest, 50, 0).unwrap().len(), 2);
        let newest = lib.list(&Filter::default(), Sort::Newest, 50, 0).unwrap();
        assert_eq!(newest[0].path, PathBuf::from(r"C:\c\c.mp4"));
        assert_eq!(
            newest[0].tags,
            vec!["Clutch".to_string(), "funny".to_string()]
        );
        let longest = lib.list(&Filter::default(), Sort::Longest, 1, 0).unwrap();
        assert_eq!(longest[0].duration_ms, 60_000);
        let text = Filter {
            text: "clut".into(),
            ..Default::default()
        };
        assert_eq!(lib.list(&text, Sort::Newest, 50, 0).unwrap().len(), 1);
        let tag = Filter {
            tag: Some("FUNNY".into()),
            ..Default::default()
        };
        assert_eq!(lib.count(&tag).unwrap(), 1);
        let dur = Filter {
            min_duration_ms: Some(20_000),
            max_duration_ms: Some(40_000),
            ..Default::default()
        };
        assert_eq!(lib.count(&dur).unwrap(), 1);
        let dates = Filter {
            from: Some(150),
            to: Some(250),
            ..Default::default()
        };
        assert_eq!(lib.count(&dates).unwrap(), 1);
        // LIKE wildcards in user text are literal.
        let wild = Filter {
            text: "%".into(),
            ..Default::default()
        };
        assert_eq!(lib.count(&wild).unwrap(), 0);
        assert_eq!(lib.games().unwrap()[0], ("CS2".to_string(), 2));
        assert_eq!(lib.tags().unwrap().len(), 2);
        lib.set_tags(c, &[]).unwrap();
        assert!(lib.tags().unwrap().is_empty(), "orphan tags are cleaned up");
    }

    #[test]
    fn markers_roundtrip() {
        let lib = Library::open_in_memory().unwrap();
        let mut it = item(r"C:\c\m.mp4", "CS2", 1, 1, 1);
        it.markers = vec![1.5, 12.25];
        let id = lib.upsert(&it).unwrap();
        assert_eq!(lib.markers(id).unwrap(), vec![1.5, 12.25]);
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hc-lib-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rename_moves_file_and_sidecar() {
        let dir = tmp("rename");
        let f = dir.join("Game 2026.mp4");
        std::fs::write(&f, b"x").unwrap();
        std::fs::write(fsops::sidecar(&f), b"{}").unwrap();
        let lib = Library::open_in_memory().unwrap();
        let id = lib
            .upsert(&item(&f.to_string_lossy(), "G", 1, 1, 1))
            .unwrap();
        let new = lib.rename(id, "My: ace?").unwrap();
        assert_eq!(new.file_name().unwrap(), "My_ ace_.mp4");
        assert!(new.exists() && !f.exists());
        assert!(fsops::sidecar(&new).exists());
        assert_eq!(lib.get(id).unwrap().unwrap().title, "My: ace?");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn quota_deletes_oldest_non_favorites() {
        let dir = tmp("quota");
        let lib = Library::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for (i, t) in [10, 20, 30, 40].iter().enumerate() {
            let p = dir.join(format!("{i}.mp4"));
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            ids.push(
                lib.upsert(&item(&p.to_string_lossy(), "G", *t, 1, 100))
                    .unwrap(),
            );
        }
        lib.set_favorite(ids[0], true).unwrap(); // oldest is protected
        let removed = lib.enforce_quota(250, true).unwrap();
        assert_eq!(
            removed.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![ids[1], ids[2]]
        );
        assert_eq!(lib.total_size().unwrap(), 200);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn grouping() {
        let mk = |game: &str, t: i64| Item {
            id: t,
            path: PathBuf::new(),
            kind: Kind::Clip,
            game: Some(game.into()),
            title: String::new(),
            created_at: t,
            duration_ms: 0,
            size_bytes: 0,
            width: 0,
            height: 0,
            codec: None,
            favorite: false,
            notes: String::new(),
            thumb: None,
            tags: vec![],
        };
        let items = vec![mk("A", 0), mk("A", 600), mk("B", 700), mk("A", 10_000)];
        let s = group_by_session(&items, 3600);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].items, vec![0, 1]);
        let g = group_by_game(&items);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].items, vec![0, 1, 3]);
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("  a/b:c.  "), "a_b_c");
        assert_eq!(sanitize("ok name"), "ok name");
    }
}
