//! Local profile store (SQLite): history, bookmarks, settings, installed
//! extensions. No cloud: the file lives in `relay_home()` and can sit in
//! iCloud Drive or a Git repo if the user wants sync.

use crate::chrome_import::{Bookmark, HistoryEntry};
use crate::Error;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;
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

/// One agent tool call. Deliberately no typed text, cookie values, or page
/// content: the audit answers "what did the agent do", not "what was on the
/// page".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentAuditRow {
    pub tool: String,
    pub host: String,
    pub tab: Option<i64>,
    pub outcome: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientRow {
    pub name: String,
    pub last_seen: i64,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportCounts {
    pub bookmarks: usize,
    pub history: usize,
}

/// Maximum tabs kept in the saved session.
pub const MAX_SESSION_TABS: usize = 200;

/// One tab of the saved session (only http/https URLs are persisted).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub scroll_y: f64,
    pub active: bool,
    pub position: i64,
}

/// A web app installed from a page: where its `.app` bundle lives and which
/// site it starts. The bundle itself carries a marker so Relay never deletes
/// a folder it did not create.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WebAppRecord {
    pub id: String,
    pub name: String,
    pub start_url: String,
    pub bundle_path: String,
    pub installed_at: i64,
}

/// Per-site preferences; the default value is never stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SitePrefs {
    pub zoom: f64,
    pub compat_mode: bool,
    pub blocking_off: bool,
    pub hidden_selectors: Vec<String>,
}

impl Default for SitePrefs {
    fn default() -> Self {
        Self { zoom: 1.0, compat_mode: false, blocking_off: false, hidden_selectors: vec![] }
    }
}

impl SitePrefs {
    fn normalized(&self) -> Self {
        let zoom = if self.zoom.is_finite() { self.zoom.clamp(0.3, 5.0) } else { 1.0 };
        Self { zoom, ..self.clone() }
    }
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Lowercase and strip a leading `www.`.
fn norm_host(host: &str) -> String {
    let h = host.trim().to_lowercase();
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            private_dir(dir)?;
        }
        // Create the database 0600 before SQLite opens it; an existing file is
        // hardened after open together with its sidecars.
        if !path.exists() {
            create_private_file(path)?;
        }
        let store = Self::init(Connection::open(path)?)?;
        harden(path);
        Ok(store)
    }

    /// Test-only profile. Crates outside this one enable the `test-support`
    /// feature in their dev-dependencies; production builds cannot call it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, Error> {
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
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
                 enabled INTEGER NOT NULL DEFAULT 1, path TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS session_tabs(
                 position INTEGER PRIMARY KEY, url TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
                 pinned INTEGER NOT NULL DEFAULT 0, scroll_y REAL NOT NULL DEFAULT 0,
                 active INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS site_prefs(
                 host TEXT PRIMARY KEY, zoom REAL NOT NULL DEFAULT 1.0,
                 compat_mode INTEGER NOT NULL DEFAULT 0, blocking_off INTEGER NOT NULL DEFAULT 0,
                 hidden_selectors TEXT NOT NULL DEFAULT '[]');
             CREATE TABLE IF NOT EXISTS web_apps(
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, start_url TEXT NOT NULL UNIQUE,
                 bundle_path TEXT NOT NULL, installed_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS agent_policies(
                 host TEXT PRIMARY KEY, policy TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS agent_audit(
                 id INTEGER PRIMARY KEY, tool TEXT NOT NULL, host TEXT NOT NULL DEFAULT '',
                 tab INTEGER, outcome TEXT NOT NULL, at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS mcp_clients(
                 name TEXT PRIMARY KEY, last_seen INTEGER NOT NULL, granted INTEGER NOT NULL DEFAULT 0);",
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

    /// The latest pages, for the new-tab "Recent" section. This is a pure
    /// recency order, unlike the palette's frecency ranking.
    pub fn recent_sites(&self, limit: usize) -> Result<Vec<Visit>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT url, title, visit_count, last_visit FROM history
             ORDER BY last_visit DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(Visit { url: r.get(0)?, title: r.get(1)?, visit_count: r.get(2)?, last_visit: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The most-visited pages, for the new-tab "Frequent" section. Ties go
    /// to the more recent page so a fresh favourite appears promptly.
    pub fn frequent_sites(&self, limit: usize) -> Result<Vec<Visit>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT url, title, visit_count, last_visit FROM history
             WHERE visit_count > 0
             ORDER BY visit_count DESC, last_visit DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
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

    pub fn is_bookmarked(&self, url: &str) -> Result<bool, Error> {
        Ok(self.conn.query_row("SELECT EXISTS(SELECT 1 FROM bookmarks WHERE url=?1)", [url], |r| r.get::<_, i64>(0))? != 0)
    }

    /// Remove the url from every folder; true if anything was removed.
    pub fn remove_bookmark(&self, url: &str) -> Result<bool, Error> {
        Ok(self.conn.execute("DELETE FROM bookmarks WHERE url=?1", [url])? > 0)
    }

    /// Move every bookmark for `url` to `folder` (empty = top level),
    /// preserving the first title. Returns false when the URL is not saved.
    pub fn move_bookmark(&self, url: &str, folder: &[String]) -> Result<bool, Error> {
        let Some(title) = self
            .conn
            .query_row("SELECT title FROM bookmarks WHERE url=?1 LIMIT 1", [url], |r| r.get::<_, String>(0))
            .optional()?
        else {
            return Ok(false);
        };
        self.conn.execute("DELETE FROM bookmarks WHERE url=?1", [url])?;
        self.add_bookmark(&Bookmark { title, url: url.into(), folder: folder.to_vec() })?;
        Ok(true)
    }

    /// Add or remove; returns the new bookmarked state.
    pub fn toggle_bookmark(&self, url: &str, title: &str) -> Result<bool, Error> {
        if self.remove_bookmark(url)? {
            return Ok(false);
        }
        self.add_bookmark(&Bookmark { url: url.into(), title: title.into(), folder: vec![] })?;
        Ok(true)
    }

    /// Atomically replace the saved session. Non-http(s) URLs are dropped and
    /// at most [`MAX_SESSION_TABS`] tabs are kept; positions are renumbered.
    pub fn save_session(&self, tabs: &[SessionTab]) -> Result<(), Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM session_tabs", [])?;
        let kept = tabs.iter().filter(|t| t.url.starts_with("http://") || t.url.starts_with("https://"));
        for (i, t) in kept.take(MAX_SESSION_TABS).enumerate() {
            let scroll = if t.scroll_y.is_finite() { t.scroll_y.max(0.0) } else { 0.0 };
            tx.execute(
                "INSERT INTO session_tabs(position, url, title, pinned, scroll_y, active) VALUES(?1,?2,?3,?4,?5,?6)",
                params![i as i64, t.url, t.title, t.pinned as i64, scroll, t.active as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_session(&self) -> Result<Vec<SessionTab>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT url, title, pinned, scroll_y, active, position FROM session_tabs ORDER BY position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SessionTab {
                url: r.get(0)?,
                title: r.get(1)?,
                pinned: r.get::<_, i64>(2)? != 0,
                scroll_y: r.get(3)?,
                active: r.get::<_, i64>(4)? != 0,
                position: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns the number of saved tabs removed. Call from `relay clear-data`.
    pub fn clear_session(&self) -> Result<usize, Error> {
        let n = self.conn.execute("DELETE FROM session_tabs", [])?;
        self.reclaim_deleted();
        Ok(n)
    }

    /// Prefs for a host; defaults when none are stored.
    pub fn site_pref(&self, host: &str) -> Result<SitePrefs, Error> {
        let row = self
            .conn
            .query_row(
                "SELECT zoom, compat_mode, blocking_off, hidden_selectors FROM site_prefs WHERE host=?1",
                [norm_host(host)],
                |r| Ok(read_prefs(r)),
            )
            .optional()?;
        Ok(row.unwrap_or_default())
    }

    /// Store prefs (zoom clamped to 0.3..=5.0); default prefs delete the row.
    pub fn set_site_pref(&self, host: &str, prefs: SitePrefs) -> Result<(), Error> {
        let host = norm_host(host);
        if host.is_empty() {
            return Err(Error::Invalid("empty host".into()));
        }
        let p = prefs.normalized();
        if p.is_default() {
            self.conn.execute("DELETE FROM site_prefs WHERE host=?1", [&host])?;
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO site_prefs(host, zoom, compat_mode, blocking_off, hidden_selectors) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(host) DO UPDATE SET zoom=?2, compat_mode=?3, blocking_off=?4, hidden_selectors=?5",
            params![host, p.zoom, p.compat_mode as i64, p.blocking_off as i64, serde_json::to_string(&p.hidden_selectors)?],
        )?;
        Ok(())
    }

    pub fn reset_site_pref(&self, host: &str) -> Result<bool, Error> {
        Ok(self.conn.execute("DELETE FROM site_prefs WHERE host=?1", [norm_host(host)])? > 0)
    }

    pub fn all_site_prefs(&self) -> Result<Vec<(String, SitePrefs)>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT zoom, compat_mode, blocking_off, hidden_selectors, host FROM site_prefs ORDER BY host",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(4)?, read_prefs(r))))?;
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

    pub fn settings_all(&self) -> Result<Vec<(String, String)>, Error> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Remove one URL from history; true if it existed.
    pub fn delete_history(&self, url: &str) -> Result<bool, Error> {
        Ok(self.conn.execute("DELETE FROM history WHERE url=?1", [url])? > 0)
    }

    pub fn clear_history(&self) -> Result<usize, Error> {
        let n = self.conn.execute("DELETE FROM history", [])?;
        self.reclaim_deleted();
        Ok(n)
    }

    pub fn clear_bookmarks(&self) -> Result<usize, Error> {
        let n = self.conn.execute("DELETE FROM bookmarks", [])?;
        self.reclaim_deleted();
        Ok(n)
    }

    fn reclaim_deleted(&self) {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;");
    }

    /// Pretty-printed JSON backup of bookmarks, history, settings, and
    /// installed extension metadata. Passwords are never stored or exported.
    pub fn export_json(&self) -> Result<String, Error> {
        let bookmarks = self
            .bookmarks()?
            .into_iter()
            .map(|b| ExportBookmark { title: b.title, url: b.url, folder: b.folder })
            .collect();
        let mut hist_stmt = self.conn.prepare(
            "SELECT url, title, visit_count, last_visit FROM history ORDER BY last_visit DESC, url ASC",
        )?;
        let history = hist_stmt
            .query_map([], |r| {
                Ok(ExportVisit {
                    url: r.get(0)?,
                    title: r.get(1)?,
                    visit_count: r.get(2)?,
                    last_visit: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let settings: BTreeMap<String, String> = self.settings_all()?.into_iter().collect();
        let mut ext_stmt = self.conn.prepare(
            "SELECT id, name, version, enabled FROM extensions ORDER BY name, id",
        )?;
        let extensions = ext_stmt
            .query_map([], |r| {
                Ok(ExportExtension {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    version: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let site_prefs: BTreeMap<String, SitePrefs> = self.all_site_prefs()?.into_iter().collect();
        let export = ProfileExport {
            schema_version: SCHEMA_VERSION,
            bookmarks,
            history,
            settings,
            extensions,
            session_tab_count: self.load_session()?.len(),
            site_prefs,
            web_apps: self.web_apps()?,
            agent_audit: self.agent_audit(5000)?,
        };
        Ok(serde_json::to_string_pretty(&export)?)
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

    pub fn remove_extension(&self, id: &str) -> Result<bool, Error> {
        let changed = self.conn.execute("DELETE FROM extensions WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// Append one audit row, keeping the newest 5,000 only.
    pub fn record_agent_call(&self, tool: &str, host: &str, tab: Option<i64>, outcome: &str, at: i64) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO agent_audit(tool, host, tab, outcome, at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![tool, host, tab, outcome, at],
        )?;
        self.conn.execute(
            "DELETE FROM agent_audit WHERE id NOT IN (SELECT id FROM agent_audit ORDER BY id DESC LIMIT 5000)",
            [],
        )?;
        Ok(())
    }

    /// Remember a connected MCP client without changing its grant.
    pub fn touch_mcp_client(&self, name: &str, at: i64) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO mcp_clients(name, last_seen, granted) VALUES(?1, ?2, 0)
             ON CONFLICT(name) DO UPDATE SET last_seen=excluded.last_seen",
            params![name, at],
        )?;
        Ok(())
    }

    pub fn set_mcp_client_granted(&self, name: &str, granted: bool, at: i64) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO mcp_clients(name, last_seen, granted) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET granted=excluded.granted",
            params![name, at, i64::from(granted)],
        )?;
        Ok(())
    }

    pub fn mcp_clients(&self) -> Result<Vec<McpClientRow>, Error> {
        let mut stmt = self.conn.prepare("SELECT name, last_seen, granted FROM mcp_clients ORDER BY last_seen DESC LIMIT 50")?;
        let rows = stmt.query_map([], |r| {
            Ok(McpClientRow { name: r.get(0)?, last_seen: r.get(1)?, granted: r.get::<_, i64>(2)? != 0 })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn mcp_client_granted(&self, name: &str) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row("SELECT granted FROM mcp_clients WHERE name=?1", [name], |r| r.get::<_, i64>(0))
            .optional()?
            .is_some_and(|v| v != 0))
    }

    pub fn agent_audit(&self, limit: usize) -> Result<Vec<AgentAuditRow>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT tool, host, tab, outcome, at FROM agent_audit ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.min(5000) as i64], |r| {
            Ok(AgentAuditRow {
                tool: r.get(0)?,
                host: r.get(1)?,
                tab: r.get(2)?,
                outcome: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn clear_agent_audit(&self) -> Result<usize, Error> {
        Ok(self.conn.execute("DELETE FROM agent_audit", [])?)
    }

    /// Per-site agent policy. Allow removes the row (the default).
    pub fn set_agent_policy(&self, host: &str, policy: relay_bridge::AgentPolicy) -> Result<(), Error> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
        if host.is_empty() {
            return Ok(());
        }
        if policy == relay_bridge::AgentPolicy::Allow {
            self.conn.execute("DELETE FROM agent_policies WHERE host=?1", [&host])?;
        } else {
            self.conn.execute("INSERT INTO agent_policies(host, policy) VALUES(?1, ?2) ON CONFLICT(host) DO UPDATE SET policy=?2", params![host, policy.as_str()])?;
        }
        Ok(())
    }

    pub fn agent_policy(&self, host: &str) -> Result<relay_bridge::AgentPolicy, Error> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
        Ok(self
            .conn
            .query_row("SELECT policy FROM agent_policies WHERE host=?1", [&host], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|p| relay_bridge::AgentPolicy::parse(&p))
            .unwrap_or_default())
    }

    pub fn all_agent_policies(&self) -> Result<Vec<(String, relay_bridge::AgentPolicy)>, Error> {
        let mut stmt = self.conn.prepare("SELECT host, policy FROM agent_policies ORDER BY host")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = vec![];
        for row in rows {
            let (host, policy) = row?;
            if let Some(policy) = relay_bridge::AgentPolicy::parse(&policy) {
                out.push((host, policy));
            }
        }
        Ok(out)
    }

    /// Record a web app installed from a page. `start_url` is unique, so
    /// reinstalling replaces the previous bundle path instead of duplicating.
    pub fn upsert_web_app(&self, app: &WebAppRecord) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO web_apps(id, name, start_url, bundle_path, installed_at) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(start_url) DO UPDATE SET id=?1, name=?2, bundle_path=?4, installed_at=?5",
            params![app.id, app.name, app.start_url, app.bundle_path, app.installed_at],
        )?;
        Ok(())
    }

    pub fn web_apps(&self) -> Result<Vec<WebAppRecord>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, start_url, bundle_path, installed_at FROM web_apps ORDER BY name, start_url",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WebAppRecord { id: r.get(0)?, name: r.get(1)?, start_url: r.get(2)?, bundle_path: r.get(3)?, installed_at: r.get(4)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn web_app_for_url(&self, start_url: &str) -> Result<Option<WebAppRecord>, Error> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, start_url, bundle_path, installed_at FROM web_apps WHERE start_url=?1",
                [start_url],
                |r| Ok(WebAppRecord { id: r.get(0)?, name: r.get(1)?, start_url: r.get(2)?, bundle_path: r.get(3)?, installed_at: r.get(4)? }),
            )
            .optional()?)
    }

    /// Forget an installed app; the caller removes the bundle first.
    pub fn remove_web_app(&self, start_url: &str) -> Result<bool, Error> {
        Ok(self.conn.execute("DELETE FROM web_apps WHERE start_url=?1", [start_url])? > 0)
    }
}

#[derive(Serialize)]
struct ProfileExport {
    schema_version: i64,
    bookmarks: Vec<ExportBookmark>,
    history: Vec<ExportVisit>,
    settings: BTreeMap<String, String>,
    extensions: Vec<ExportExtension>,
    session_tab_count: usize,
    site_prefs: BTreeMap<String, SitePrefs>,
    web_apps: Vec<WebAppRecord>,
    agent_audit: Vec<AgentAuditRow>,
}

fn read_prefs(r: &rusqlite::Row<'_>) -> SitePrefs {
    let hidden: String = r.get(3).unwrap_or_default();
    SitePrefs {
        zoom: r.get(0).unwrap_or(1.0),
        compat_mode: r.get::<_, i64>(1).unwrap_or(0) != 0,
        blocking_off: r.get::<_, i64>(2).unwrap_or(0) != 0,
        hidden_selectors: serde_json::from_str(&hidden).unwrap_or_default(),
    }
}

#[derive(Serialize)]
struct ExportBookmark {
    title: String,
    url: String,
    folder: Vec<String>,
}

#[derive(Serialize)]
struct ExportVisit {
    url: String,
    title: String,
    visit_count: i64,
    last_visit: i64,
}

#[derive(Serialize)]
struct ExportExtension {
    id: String,
    name: String,
    version: String,
    enabled: bool,
}

/// History and bookmarks are private: only the user may read the profile,
/// including SQLite's WAL sidecars.
#[cfg(unix)]
fn harden(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    for suffix in ["", "-wal", "-shm"] {
        let p = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if p.exists() {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn harden(_path: &Path) {}

#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir)?;
    // Tighten a directory that already existed with a wider mode.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map(|_| ())
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_clients_track_last_seen_and_grants() {
        let s = Store::in_memory().unwrap();
        s.touch_mcp_client("Claude Desktop", 10).unwrap();
        assert!(!s.mcp_client_granted("Claude Desktop").unwrap());
        s.set_mcp_client_granted("Claude Desktop", true, 11).unwrap();
        assert!(s.mcp_client_granted("Claude Desktop").unwrap());
        s.touch_mcp_client("Claude Desktop", 12).unwrap();
        assert!(s.mcp_client_granted("Claude Desktop").unwrap(), "touch does not revoke");
        s.set_mcp_client_granted("Claude Desktop", false, 13).unwrap();
        assert!(!s.mcp_client_granted("Claude Desktop").unwrap(), "revoke is immediate for the next MCP call");
        let c = s.mcp_clients().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].last_seen, 12);
        assert!(!c[0].granted);
    }

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
    fn frequent_sites_rank_by_visits_then_recency() {
        let s = Store::in_memory().unwrap();
        let now = 1_800_000_000;
        for _ in 0..3 {
            s.record_visit("https://often.test/", "Often", now).unwrap();
        }
        for _ in 0..2 {
            s.record_visit("https://newer.test/", "Newer", now + 10).unwrap();
        }
        s.record_visit("https://once.test/", "Once", now).unwrap();
        let f = s.frequent_sites(10).unwrap();
        assert_eq!(f.iter().map(|v| v.url.as_str()).collect::<Vec<_>>(), vec!["https://often.test/", "https://newer.test/", "https://once.test/"]);
        assert_eq!(f[0].visit_count, 3);
        assert_eq!(s.frequent_sites(2).unwrap().len(), 2);
        let recent = s.recent_sites(2).unwrap();
        assert_eq!(recent.iter().map(|v| v.url.as_str()).collect::<Vec<_>>(), vec!["https://newer.test/", "https://often.test/"]);
        assert_eq!(recent[0].visit_count, 2);
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

    #[cfg(unix)]
    #[test]
    fn profile_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("Relay");
        let p = home.join("profile.sqlite");
        let store = Store::open(&p).unwrap();
        store.record_visit("https://a.test", "A", 1).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
        for suffix in ["-wal", "-shm"] {
            let sidecar = std::path::PathBuf::from(format!("{}{suffix}", p.display()));
            if sidecar.exists() {
                assert_eq!(std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777, 0o600, "{}", sidecar.display());
            }
        }
    }

    #[test]
    fn settings_round_trip() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.setting("search").unwrap(), None);
        s.set_setting("search", "https://duckduckgo.com/?q=%s").unwrap();
        s.set_setting("adblock", "true").unwrap();
        assert_eq!(s.setting("search").unwrap().as_deref(), Some("https://duckduckgo.com/?q=%s"));
        assert_eq!(
            s.settings_all().unwrap(),
            vec![
                ("adblock".into(), "true".into()),
                ("search".into(), "https://duckduckgo.com/?q=%s".into()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_backed_clear_history_reclaims_the_marker() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Relay/profile.sqlite");
        let store = Store::open(&p).unwrap();
        store.record_visit("https://deleted-marker.test/", "UNIQUE-CLEAR-MARKER", 1).unwrap();
        assert_eq!(store.clear_history().unwrap(), 1);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let path = std::path::PathBuf::from(format!("{}{suffix}", p.display()));
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("UNIQUE-CLEAR-MARKER"),
                "{} still contains deleted history text",
                path.display()
            );
        }
    }

    #[test]
    fn clear_history_and_bookmarks() {
        let s = Store::in_memory().unwrap();
        s.record_visit("https://a.test/", "A", 100).unwrap();
        s.record_visit("https://b.test/", "B", 200).unwrap();
        s.add_bookmark(&Bookmark { title: "A".into(), url: "https://a.test/".into(), folder: vec!["Bar".into()] }).unwrap();
        assert!(s.delete_history("https://a.test/").unwrap());
        assert!(!s.delete_history("https://a.test/").unwrap());
        assert_eq!(s.clear_history().unwrap(), 1);
        assert!(s.search_history("", 10).unwrap().is_empty());
        assert_eq!(s.clear_history().unwrap(), 0);
        assert_eq!(s.clear_bookmarks().unwrap(), 1);
        assert!(s.bookmarks().unwrap().is_empty());
        assert_eq!(s.clear_bookmarks().unwrap(), 0);
    }

    #[test]
    fn export_json_contains_profile_without_passwords() {
        let s = Store::in_memory().unwrap();
        s.add_bookmark(&Bookmark { title: "Docs".into(), url: "https://docs.rs/".into(), folder: vec!["Dev".into()] }).unwrap();
        s.record_visit("https://older.test/", "Older", 100).unwrap();
        s.record_visit("https://newer.test/", "Newer", 200).unwrap();
        s.record_visit("https://newer.test/", "Newer", 250).unwrap();
        s.set_setting("search", "https://kagi.com/search?q=%s").unwrap();
        s.set_setting("sleep_minutes", "15").unwrap();
        s.record_extension("ext1", "uBlock", "1.0.0", Path::new("/tmp/ext1")).unwrap();

        let json = s.export_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["bookmarks"][0]["title"], "Docs");
        assert_eq!(v["bookmarks"][0]["url"], "https://docs.rs/");
        assert_eq!(v["bookmarks"][0]["folder"], serde_json::json!(["Dev"]));
        assert_eq!(v["history"].as_array().unwrap().len(), 2);
        assert_eq!(v["history"][0]["url"], "https://newer.test/");
        assert_eq!(v["history"][0]["visit_count"], 2);
        assert_eq!(v["history"][0]["last_visit"], 250);
        assert_eq!(v["history"][1]["url"], "https://older.test/");
        assert_eq!(v["settings"]["search"], "https://kagi.com/search?q=%s");
        assert_eq!(v["settings"]["sleep_minutes"], "15");
        assert_eq!(v["extensions"][0]["id"], "ext1");
        assert_eq!(v["extensions"][0]["name"], "uBlock");
        assert_eq!(v["extensions"][0]["version"], "1.0.0");
        assert_eq!(v["extensions"][0]["enabled"], true);
        assert!(v.get("passwords").is_none());
        assert!(!json.contains("password"));
        assert_eq!(v["session_tab_count"], 0);
        assert_eq!(v["site_prefs"], serde_json::json!({}));
        assert!(v["agent_audit"].is_array());
    }

    fn tab(url: &str, active: bool) -> SessionTab {
        SessionTab { url: url.into(), title: "T".into(), pinned: false, scroll_y: 12.5, active, position: 0 }
    }

    #[test]
    fn session_round_trip_filters_caps_and_replaces() {
        let s = Store::in_memory().unwrap();
        assert!(s.load_session().unwrap().is_empty());
        let mut pinned = tab("https://b.test/", false);
        pinned.pinned = true;
        s.save_session(&[tab("https://a.test/", true), tab("about:blank", false), tab("file:///etc/passwd", false), pinned]).unwrap();
        let got = s.load_session().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].url.as_str(), got[0].active, got[0].position, got[0].scroll_y), ("https://a.test/", true, 0, 12.5));
        assert_eq!((got[1].url.as_str(), got[1].pinned, got[1].position), ("https://b.test/", true, 1));
        s.save_session(&[tab("http://c.test/", true)]).unwrap();
        assert_eq!(s.load_session().unwrap().len(), 1);
        let many: Vec<_> = (0..300).map(|i| tab(&format!("https://x.test/{i}"), false)).collect();
        s.save_session(&many).unwrap();
        assert_eq!(s.load_session().unwrap().len(), MAX_SESSION_TABS);
        // clear_history leaves the session alone; clear_session is separate.
        s.clear_history().unwrap();
        assert_eq!(s.load_session().unwrap().len(), MAX_SESSION_TABS);
        assert_eq!(s.clear_session().unwrap(), MAX_SESSION_TABS);
        assert!(s.load_session().unwrap().is_empty());
        s.save_session(&[]).unwrap();
    }

    #[test]
    fn site_prefs_defaults_normalize_clamp_reset() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.site_pref("a.test").unwrap(), SitePrefs::default());
        s.set_site_pref("WWW.Example.com", SitePrefs { zoom: 9.0, compat_mode: true, blocking_off: false, hidden_selectors: vec![".ad".into()] }).unwrap();
        let p = s.site_pref("example.com").unwrap();
        assert_eq!((p.zoom, p.compat_mode, p.hidden_selectors.clone()), (5.0, true, vec![".ad".to_string()]));
        s.set_site_pref("example.com", SitePrefs { zoom: 0.01, ..p }).unwrap();
        assert_eq!(s.site_pref("www.example.com").unwrap().zoom, 0.3);
        assert_eq!(s.all_site_prefs().unwrap().len(), 1);
        assert_eq!(s.all_site_prefs().unwrap()[0].0, "example.com");
        let v: serde_json::Value = serde_json::from_str(&s.export_json().unwrap()).unwrap();
        assert_eq!(v["site_prefs"]["example.com"]["compat_mode"], true);
        // Writing defaults stores no row.
        s.set_site_pref("example.com", SitePrefs::default()).unwrap();
        assert!(s.all_site_prefs().unwrap().is_empty());
        s.set_site_pref("b.test", SitePrefs { blocking_off: true, ..Default::default() }).unwrap();
        assert!(s.reset_site_pref("b.test").unwrap());
        assert!(!s.reset_site_pref("b.test").unwrap());
    }

    #[test]
    fn bookmark_toggle() {
        let s = Store::in_memory().unwrap();
        assert!(!s.is_bookmarked("https://a.test/").unwrap());
        assert!(s.toggle_bookmark("https://a.test/", "A").unwrap());
        assert!(s.is_bookmarked("https://a.test/").unwrap());
        assert!(!s.toggle_bookmark("https://a.test/", "A").unwrap());
        assert!(!s.remove_bookmark("https://a.test/").unwrap());
        s.toggle_bookmark("https://a.test/", "A").unwrap();
        assert!(s.remove_bookmark("https://a.test/").unwrap());
        s.add_bookmark(&Bookmark { title: "Move".into(), url: "https://m.test/".into(), folder: vec!["Old".into()] }).unwrap();
        assert!(s.move_bookmark("https://m.test/", &["New".into(), "Sub".into()]).unwrap());
        assert_eq!(s.bookmarks().unwrap()[0].folder, vec!["New".to_string(), "Sub".to_string()]);
        assert!(!s.move_bookmark("https://missing.test/", &[]).unwrap());
    }

    #[test]
    fn agent_audit_round_trips_and_clears() {
        let s = Store::in_memory().unwrap();
        s.record_agent_call("click_element", "shop.test", Some(2), "ok", 10).unwrap();
        s.record_agent_call("type_text", "shop.test", Some(2), "error", 11).unwrap();
        let rows = s.agent_audit(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], AgentAuditRow { tool: "type_text".into(), host: "shop.test".into(), tab: Some(2), outcome: "error".into(), at: 11 });
        assert_eq!(s.clear_agent_audit().unwrap(), 2);
        assert!(s.agent_audit(10).unwrap().is_empty());
    }

    #[test]
    fn agent_policies_round_trip_and_allow_clears() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.agent_policy("shop.test").unwrap(), relay_bridge::AgentPolicy::Allow);
        s.set_agent_policy("WWW.Shop.Test", relay_bridge::AgentPolicy::ReadOnly).unwrap();
        s.set_agent_policy("blocked.test", relay_bridge::AgentPolicy::Block).unwrap();
        assert_eq!(s.agent_policy("shop.test").unwrap(), relay_bridge::AgentPolicy::ReadOnly);
        assert_eq!(s.all_agent_policies().unwrap(), vec![("blocked.test".into(), relay_bridge::AgentPolicy::Block), ("shop.test".into(), relay_bridge::AgentPolicy::ReadOnly)]);
        s.set_agent_policy("shop.test", relay_bridge::AgentPolicy::Allow).unwrap();
        assert!(s.all_agent_policies().unwrap().len() == 1);
    }

    #[test]
    fn installed_web_apps_replace_by_start_url() {
        let s = Store::in_memory().unwrap();
        let rec = WebAppRecord {
            id: "app.relay.webapp.shop.1".into(),
            name: "Shop".into(),
            start_url: "https://shop.test/app".into(),
            bundle_path: "/Users/me/Applications/Shop.app".into(),
            installed_at: 10,
        };
        assert_eq!(s.web_app_for_url(&rec.start_url).unwrap(), None);
        s.upsert_web_app(&rec).unwrap();
        assert_eq!(s.web_app_for_url(&rec.start_url).unwrap(), Some(rec.clone()));
        assert_eq!(s.web_apps().unwrap(), vec![rec.clone()]);

        let moved = WebAppRecord { name: "New Shop".into(), bundle_path: "/tmp/New Shop.app".into(), installed_at: 20, ..rec.clone() };
        s.upsert_web_app(&moved).unwrap();
        assert_eq!(s.web_apps().unwrap(), vec![moved.clone()], "one row per start URL");
        assert_eq!(s.web_apps().unwrap()[0].installed_at, 20);

        let v: serde_json::Value = serde_json::from_str(&s.export_json().unwrap()).unwrap();
        assert_eq!(v["web_apps"][0]["name"], "New Shop");

        assert!(s.remove_web_app(&rec.start_url).unwrap());
        assert!(!s.remove_web_app(&rec.start_url).unwrap());
        assert!(s.web_apps().unwrap().is_empty());
    }
}
