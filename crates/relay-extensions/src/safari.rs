//! Safari bookmark/history import. Safari's files are read-only; history is
//! copied first because Safari keeps the SQLite database locked while running.
//! Reading `~/Library/Safari` may require Full Disk Access on current macOS.

use crate::chrome_import::{Bookmark, HistoryEntry};
use crate::Error;
use plist::Value;
use std::path::{Path, PathBuf};

/// `~/Library/Safari/Bookmarks.plist`.
pub fn default_bookmarks() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join("Library/Safari/Bookmarks.plist")).filter(|p| p.is_file())
}

/// `~/Library/Safari/History.db`.
pub fn default_history() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join("Library/Safari/History.db")).filter(|p| p.is_file())
}

/// Parse Safari's property-list bookmarks, keeping folder titles.
pub fn read_bookmarks(path: &Path) -> Result<Vec<Bookmark>, Error> {
    let value = Value::from_file(path).map_err(|e| Error::Invalid(format!("Safari bookmarks: {e}")))?;
    let mut out = Vec::new();
    let mut folders = Vec::new();
    walk(&value, &mut folders, &mut out);
    Ok(out)
}

fn walk(value: &Value, folders: &mut Vec<String>, out: &mut Vec<Bookmark>) {
    let Some(dict) = value.as_dictionary() else { return };
    let kind = dict.get("WebBookmarkType").and_then(Value::as_string).unwrap_or_default();
    let title = dict
        .get("URIDictionary")
        .and_then(Value::as_dictionary)
        .and_then(|d| d.get("title"))
        .and_then(Value::as_string)
        .or_else(|| dict.get("Title").and_then(Value::as_string))
        .unwrap_or_default()
        .to_string();
    match kind {
        "WebBookmarkTypeList" => {
            let pushed = !title.is_empty() && title != "Bookmarks";
            if pushed {
                folders.push(title);
            }
            if let Some(children) = dict.get("Children").and_then(Value::as_array) {
                for child in children {
                    walk(child, folders, out);
                }
            }
            if pushed {
                folders.pop();
            }
        }
        "WebBookmarkTypeLeaf" => {
            let Some(url) = dict.get("URLString").and_then(Value::as_string).filter(|u| u.starts_with("http")) else {
                return;
            };
            out.push(Bookmark { title: if title.is_empty() { url.to_string() } else { title }, url: url.to_string(), folder: folders.clone() });
        }
        _ => {
            // A bare root dict (or an unknown list type) still has children.
            if let Some(children) = dict.get("Children").and_then(Value::as_array) {
                for child in children {
                    walk(child, folders, out);
                }
            }
        }
    }
}

/// Read Safari history into Relay's history rows. Safely returns an error when
/// the file is missing or Full Disk Access is denied.
pub fn read_history(path: &Path) -> Result<Vec<HistoryEntry>, Error> {
    let copy = std::env::temp_dir().join(format!("relay-safari-history-{}.db", std::process::id()));
    std::fs::copy(path, &copy)?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", path.display()));
        if side.is_file() {
            let _ = std::fs::copy(&side, format!("{}{suffix}", copy.display()));
        }
    }
    let conn = rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT i.url, COALESCE(v.title,''), i.visit_count, v.visit_time
         FROM history_items i
         JOIN history_visits v ON v.history_item = i.id
         WHERE v.id = (SELECT MAX(v2.id) FROM history_visits v2 WHERE v2.history_item = i.id)",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(HistoryEntry {
            url: r.get(0)?,
            title: r.get(1)?,
            visit_count: r.get::<_, i64>(2)?.max(1),
            last_visit: (r.get::<_, f64>(3)? + 978_307_200.0) as i64,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if row.url.starts_with("http") {
            out.push(row);
        }
    }
    drop(stmt);
    drop(conn);
    let _ = std::fs::remove_file(&copy);
    let _ = std::fs::remove_file(format!("{}-wal", copy.display()));
    let _ = std::fs::remove_file(format!("{}-shm", copy.display()));
    Ok(out)
}

/// What an import would/adds did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SafariImport {
    pub bookmarks: usize,
    pub history: usize,
    pub skipped: usize,
}

/// Merge Safari's bookmarks and history into `store`. A dry run only counts.
pub fn import(store: &crate::store::Store, bookmarks: &Path, history: &Path, dry_run: bool) -> Result<SafariImport, Error> {
    let mut counts = SafariImport::default();
    if bookmarks.is_file() {
        for b in read_bookmarks(bookmarks)? {
            if dry_run || store.add_bookmark(&b)? {
                counts.bookmarks += 1;
            } else {
                counts.skipped += 1;
            }
        }
    }
    if history.is_file() {
        for h in read_history(history)? {
            if !dry_run {
                store.record_visit(&h.url, &h.title, h.last_visit)?;
            }
            counts.history += 1;
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn reads_folder_titles_and_leaf_urls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks.plist");
        std::fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Children</key><array>
<dict><key>WebBookmarkType</key><string>WebBookmarkTypeList</string><key>Title</key><string>Bookmarks</string>
  <key>Children</key><array>
    <dict><key>WebBookmarkType</key><string>WebBookmarkTypeList</string><key>Title</key><string>Work</string>
      <key>Children</key><array><dict><key>WebBookmarkType</key><string>WebBookmarkTypeLeaf</string>
        <key>URLString</key><string>https://work.test/</string>
        <key>URIDictionary</key><dict><key>title</key><string>Work site</string></dict>
      </dict></array>
    </dict>
    <dict><key>WebBookmarkType</key><string>WebBookmarkTypeLeaf</string><key>URLString</key><string>https://home.test/</string>
      <key>URIDictionary</key><dict><key>title</key><string>Home</string></dict></dict>
  </array></dict>
</array></dict></plist>"#,
        )
        .unwrap();
        let bookmarks = read_bookmarks(&path).unwrap();
        assert_eq!(bookmarks.len(), 2);
        assert_eq!(bookmarks[0].url, "https://work.test/");
        assert_eq!(bookmarks[0].folder, vec!["Work".to_string()]);
        assert_eq!(bookmarks[1].title, "Home");
    }

    #[test]
    fn reads_history_and_converts_safari_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE history_items(id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
                 CREATE TABLE history_visits(id INTEGER PRIMARY KEY, history_item INTEGER, visit_time REAL, title TEXT);
                 INSERT INTO history_items VALUES(1,'https://a.test/',3);
                 INSERT INTO history_visits VALUES(1,1,100.0,'Old');
                 INSERT INTO history_visits VALUES(2,1,200.0,'New');",
            )
            .unwrap();
        }
        let h = read_history(&path).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].url, "https://a.test/");
        assert_eq!(h[0].title, "New");
        assert_eq!(h[0].visit_count, 3);
        assert_eq!(h[0].last_visit, 978_307_400);
    }
}
