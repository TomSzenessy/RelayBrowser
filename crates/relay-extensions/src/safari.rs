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

const MAX_EXPORT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct SafariHistoryJson {
    #[serde(default)]
    history: Vec<SafariHistoryRow>,
}

#[derive(serde::Deserialize)]
struct SafariHistoryRow {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    visit_count: i64,
    #[serde(default)]
    time_usec: i64,
    #[serde(default)]
    last_visit_time_usec: i64,
}

fn parse_history_json(raw: &str) -> Result<Vec<HistoryEntry>, Error> {
    let parsed: SafariHistoryJson =
        serde_json::from_str(raw).map_err(|e| Error::Invalid(format!("unreadable Safari History.json: {e}")))?;
    let mut out = Vec::new();
    for row in parsed.history {
        if !(row.url.starts_with("http://") || row.url.starts_with("https://")) {
            continue;
        }
        let usec = if row.time_usec > 0 { row.time_usec } else { row.last_visit_time_usec };
        let last_visit = if usec > 1_000_000_000_000 { usec / 1_000_000 } else { usec };
        out.push(HistoryEntry {
            url: row.url,
            title: row.title,
            visit_count: row.visit_count.max(1),
            last_visit: last_visit.max(0),
        });
    }
    Ok(out)
}

fn parse_bookmarks_plist_bytes(bytes: &[u8]) -> Result<Vec<Bookmark>, Error> {
    let value = Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| Error::Invalid(format!("unreadable Safari Bookmarks.plist: {e}")))?;
    let mut out = Vec::new();
    let mut folders = Vec::new();
    walk(&value, &mut folders, &mut out);
    Ok(out)
}

/// Import Safari's exported browsing-data file (`.zip` from Safari → File →
/// Export Browsing Data to File…, or a standalone `Bookmarks.html`,
/// `Bookmarks.plist`, or `History.json`). The file is read in place and never
/// copied into the profile; malformed or unrecognized files import nothing.
pub fn import_export_file(store: &crate::store::Store, path: &Path) -> Result<SafariImport, Error> {
    use std::io::Read;

    let meta = std::fs::metadata(path).map_err(|e| Error::Invalid(format!("cannot read export file: {e}")))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(Error::Invalid("Safari export file is empty or missing".into()));
    }
    if meta.len() > MAX_EXPORT_BYTES {
        return Err(Error::Invalid("Safari export file exceeds 64 MB limit".into()));
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut bookmarks: Vec<Bookmark> = Vec::new();
    let mut history: Vec<HistoryEntry> = Vec::new();
    let mut recognized = false;

    if ext == "zip" {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(|_| Error::Invalid("invalid Safari export ZIP archive".into()))?;
        let limit = archive.len().min(128);
        for i in 0..limit {
            let mut entry = archive.by_index(i)?;
            if entry.is_dir() || entry.size() > MAX_ENTRY_BYTES {
                continue;
            }
            let raw_name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            let name = raw_name.rsplit('/').next().unwrap_or(&raw_name).to_ascii_lowercase();
            if name.ends_with(".html") && name.contains("bookmark") {
                let mut text = String::new();
                entry.by_ref().take(MAX_ENTRY_BYTES).read_to_string(&mut text)?;
                bookmarks.extend(crate::bookmarks_html::parse(&text));
                recognized = true;
            } else if name.ends_with(".plist") && name.contains("bookmark") {
                let mut bytes = Vec::new();
                entry.by_ref().take(MAX_ENTRY_BYTES).read_to_end(&mut bytes)?;
                bookmarks.extend(parse_bookmarks_plist_bytes(&bytes)?);
                recognized = true;
            } else if name.ends_with(".json") && name.contains("history") {
                let mut text = String::new();
                entry.by_ref().take(MAX_ENTRY_BYTES).read_to_string(&mut text)?;
                history.extend(parse_history_json(&text)?);
                recognized = true;
            }
        }
        if !recognized {
            return Err(Error::Invalid("ZIP does not contain Safari Bookmarks or History export files".into()));
        }
    } else if ext == "html" || ext == "htm" {
        let text = std::fs::read_to_string(path).map_err(|_| Error::Invalid("unreadable Safari bookmark HTML".into()))?;
        if !text.to_ascii_lowercase().contains("<dl") {
            return Err(Error::Invalid("file is not a Safari bookmark HTML export".into()));
        }
        bookmarks = crate::bookmarks_html::parse(&text);
    } else if ext == "plist" {
        let bytes = std::fs::read(path)?;
        bookmarks = parse_bookmarks_plist_bytes(&bytes)?;
    } else if ext == "json" {
        let text = std::fs::read_to_string(path).map_err(|_| Error::Invalid("unreadable Safari History.json".into()))?;
        history = parse_history_json(&text)?;
    } else {
        return Err(Error::Invalid(
            "unsupported Safari export format (choose Safari's .zip export, Bookmarks.html, Bookmarks.plist, or History.json)".into(),
        ));
    }

    let mut counts = SafariImport::default();
    for b in bookmarks {
        if store.add_bookmark(&b)? {
            counts.bookmarks += 1;
        } else {
            counts.skipped += 1;
        }
    }
    for h in history {
        store.record_visit(&h.url, &h.title, h.last_visit)?;
        counts.history += 1;
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

    #[test]
    fn imports_safari_export_zip_deduplicates_and_rejects_malformed_files() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let profile_dir = dir.path().join("profile");
        let store = crate::store::Store::open(&profile_dir.join("profile.sqlite")).unwrap();

        // Build a Safari exported browsing-data ZIP with Bookmarks.html + History.json.
        let zip_path = dir.path().join("Safari Export.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("Safari Export/Bookmarks.html", opts).unwrap();
            z.write_all(
                br#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
<DT><H3>Reading List</H3>
<DL><p>
<DT><A HREF="https://read.test/article">Deep Dive</A>
</DL><p>
<DT><A HREF="https://home.test/">Home</A>
</DL><p>"#,
            )
            .unwrap();
            z.start_file("Safari Export/History.json", opts).unwrap();
            z.write_all(
                br#"{"metadata":{"browser_name":"Safari"},"history":[
                    {"url":"https://hist.test/page","title":"Hist","time_usec":1700000000000000,"visit_count":2},
                    {"url":"javascript:alert(1)","title":"Bad","time_usec":1700000000000000,"visit_count":1}
                ]}"#,
            )
            .unwrap();
            z.finish().unwrap();
        }

        let first = import_export_file(&store, &zip_path).unwrap();
        assert_eq!(first.bookmarks, 2);
        assert_eq!(first.history, 1);
        assert_eq!(first.skipped, 0);

        // Round-trip deduplication: re-importing skips duplicate bookmarks.
        let second = import_export_file(&store, &zip_path).unwrap();
        assert_eq!(second.bookmarks, 0);
        assert_eq!(second.skipped, 2);

        // Filesystem check: export file is never copied into the profile directory.
        let profile_files: Vec<String> = std::fs::read_dir(&profile_dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(!profile_files.iter().any(|n| n.ends_with(".zip")), "export must not be copied into profile: {profile_files:?}");

        // Negative: malformed/unsupported files import nothing and return a bounded error.
        let bad_zip = dir.path().join("corrupt.zip");
        std::fs::write(&bad_zip, b"not a zip at all").unwrap();
        assert!(import_export_file(&store, &bad_zip).is_err());

        let bad_ext = dir.path().join("notes.txt");
        std::fs::write(&bad_ext, b"plain text").unwrap();
        assert!(import_export_file(&store, &bad_ext).is_err());
    }
}
