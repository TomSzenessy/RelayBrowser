//! Local profile store (SQLite): history, bookmarks, settings, installed
//! extensions. No cloud: the file lives in `relay_home()` and can sit in
//! iCloud Drive or a Git repo if the user wants sync.

use crate::chrome_import::{Bookmark, HistoryEntry};
use crate::Error;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

const SCHEMA_VERSION: i64 = 1;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    pub last_visit: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportCounts {
    pub bookmarks: usize,
    pub history: usize,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, Error> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS history(
                 url TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
                 visit_count INTEGER NOT NULL DEFAULT 1, last_visit INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS history_recent ON history(last_visit DESC);
             CREATE TABLE IF NOT EXISTS bookmarks(
                 id INTEGER PRIMARY KEY, url TEXT NOT NULL, title TEXT NOT NULL,
                 folder TEXT NOT NULL DEFAULT '', UNIQUE(url, folder));
             CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS extensions(
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL,
                 enabled INTEGER NOT NULL DEFAULT 1, path TEXT NOT NULL);",
        )?;
        let v: Option<i64> = conn
            .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|s| s.parse().ok());
        match v {
            None => {
                conn.execute("INSERT INTO meta VALUES('schema', ?1)", [SCHEMA_VERSION.to_string()])?;
            }
            Some(v) if v > SCHEMA_VERSION => {
                return Err(Error::Invalid(format!("profile schema {v} is newer than this Relay ({SCHEMA_VERSION}); update Relay")));
            }
            Some(_) => {}
        }
        Ok(Self { conn })
    }

    pub fn record_visit(&self, url: &str, title: &str, now: i64) -> Result<(), Error> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO history(url, title, visit_count, last_visit) VALUES(?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET visit_count = visit_count + 1, last_visit = ?3,
                 title = CASE WHEN ?2 = '' THEN title ELSE ?2 END",
            params![url, title, now],
        )?;
        Ok(())
    }

    /// Frecency-ish search for the ⌘K palette: substring match on URL or
    /// title, ranked by visits and recency.
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<Visit>, Error> {
        let like = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = self.conn.prepare(
            "SELECT url, title, visit_count, last_visit FROM history
             WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
             ORDER BY visit_count * 1.0 / (1 + max(0, strftime('%s','now') - last_visit) / 86400.0) DESC, last_visit DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![like, limit as i64], |r| {
            Ok(Visit { url: r.get(0)?, title: r.get(1)?, visit_count: r.get(2)?, last_visit: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_bookmark(&self, b: &Bookmark) -> Result<bool, Error> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO bookmarks(url, title, folder) VALUES(?1, ?2, ?3)",
            params![b.url, b.title, b.folder.join("/")],
        )?;
        Ok(n > 0)
    }

    pub fn bookmarks(&self) -> Result<Vec<Bookmark>, Error> {
        let mut stmt = self.conn.prepare("SELECT url, title, folder FROM bookmarks ORDER BY folder, id")?;
        let rows = stmt.query_map([], |r| {
            let folder: String = r.get(2)?;
            Ok(Bookmark {
                url: r.get(0)?,
                title: r.get(1)?,
                folder: if folder.is_empty() { vec![] } else { folder.split('/').map(str::to_string).collect() },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Idempotent Chrome import: re-running adds nothing twice.
    pub fn import(&mut self, bookmarks: &[Bookmark], history: &[HistoryEntry]) -> Result<ImportCounts, Error> {
        let tx = self.conn.transaction()?;
        let mut counts = ImportCounts { bookmarks: 0, history: 0 };
        for b in bookmarks {
            counts.bookmarks += tx.execute(
                "INSERT OR IGNORE INTO bookmarks(url, title, folder) VALUES(?1, ?2, ?3)",
                params![b.url, b.title, b.folder.join("/")],
            )?;
        }
        for h in history {
            counts.history += tx.execute(
                "INSERT INTO history(url, title, visit_count, last_visit) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(url) DO UPDATE SET visit_count = max(visit_count, ?3), last_visit = max(last_visit, ?4)
                 WHERE ?4 > last_visit OR ?3 > visit_count",
                params![h.url, h.title, h.visit_count, h.last_visit],
            )?;
        }
        tx.commit()?;
        Ok(counts)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), Error> {
        self.conn.execute("INSERT INTO settings VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2", [key, value])?;
        Ok(())
    }

    pub fn record_extension(&self, id: &str, name: &str, version: &str, path: &Path) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO extensions(id, name, version, path) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name=?2, version=?3, path=?4",
            params![id, name, version, path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// (id, name, path) of enabled extensions, loaded at startup.
    pub fn enabled_extensions(&self) -> Result<Vec<(String, String, String)>, Error> {
        let mut stmt = self.conn.prepare("SELECT id, name, path FROM extensions WHERE enabled = 1 ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_accumulate_and_search_ranks() {
        let s = Store::in_memory().unwrap();
        let now = 1_800_000_000;
        s.record_visit("https://docs.rs/gpui", "gpui - Rust", now).unwrap();
        s.record_visit("https://docs.rs/gpui", "", now).unwrap();
        s.record_visit("https://docs.rs/tokio", "tokio", now).unwrap();
        s.record_visit("about:blank", "", now).unwrap();
        let r = s.search_history("docs", 10).unwrap();
        assert_eq!(r[0].url, "https://docs.rs/gpui");
        assert_eq!((r[0].visit_count, r[0].title.as_str()), (2, "gpui - Rust"));
        assert_eq!(r.len(), 2);
        assert!(s.search_history("100%", 10).unwrap().is_empty(), "LIKE wildcards are escaped");
        // Future timestamps (clock skew, imported data) must not invert ranking.
        s.record_visit("https://docs.rs/serde", "serde", now + 10_000_000).unwrap();
        assert_eq!(s.search_history("docs", 10).unwrap()[0].url, "https://docs.rs/gpui");
    }

    #[test]
    fn import_is_idempotent() {
        let mut s = Store::in_memory().unwrap();
        let b = vec![Bookmark { title: "R".into(), url: "https://r.test/".into(), folder: vec!["Bar".into(), "Dev".into()] }];
        let h = vec![HistoryEntry { url: "https://r.test/".into(), title: "R".into(), visit_count: 3, last_visit: 10 }];
        assert_eq!(s.import(&b, &h).unwrap(), ImportCounts { bookmarks: 1, history: 1 });
        assert_eq!(s.import(&b, &h).unwrap(), ImportCounts { bookmarks: 0, history: 0 });
        assert_eq!(s.bookmarks().unwrap(), b);
    }

    #[test]
    fn refuses_newer_schema() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("profile.sqlite");
        drop(Store::open(&p).unwrap());
        Connection::open(&p).unwrap().execute("UPDATE meta SET value='99' WHERE key='schema'", []).unwrap();
        assert!(Store::open(&p).err().unwrap().to_string().contains("newer"));
    }

    #[test]
    fn settings_round_trip() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.setting("search").unwrap(), None);
        s.set_setting("search", "https://duckduckgo.com/?q=%s").unwrap();
        assert_eq!(s.setting("search").unwrap().as_deref(), Some("https://duckduckgo.com/?q=%s"));
    }
}
