//! One-click "Import from Chrome": read Chrome's local profile files directly.
//! Read-only: Chrome's files are never modified (History is copied first
//! because Chrome holds a lock on it while running).

use crate::Error;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// Folder path from the root, e.g. ["Bookmarks bar", "Work"].
    pub folder: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Unix seconds.
    pub last_visit: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstalledExtension {
    /// 32-char Chrome Web Store id (a–p).
    pub id: String,
    pub name: String,
    pub version: String,
}

/// A row of Chrome's "Export passwords" CSV. Never logged or persisted by
/// Relay: it goes straight into the macOS Keychain.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Credential {
    #[serde(default)]
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub note: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential").field("url", &self.url).field("username", &self.username).field("password", &"<redacted>").finish()
    }
}

/// `~/Library/Application Support/Google/Chrome/Default` on macOS.
pub fn default_profile() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let candidates = [
        home.join("Library/Application Support/Google/Chrome/Default"),
        home.join(".config/google-chrome/Default"),
        home.join(".config/chromium/Default"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

/// Parse the `Bookmarks` JSON file.
pub fn read_bookmarks(profile: &Path) -> Result<Vec<Bookmark>, Error> {
    let v: Value = serde_json::from_slice(&std::fs::read(profile.join("Bookmarks"))?)?;
    let mut out = vec![];
    if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
        for key in ["bookmark_bar", "other", "synced"] {
            if let Some(root) = roots.get(key) {
                walk_bookmarks(root, &mut vec![], &mut out, true);
            }
        }
    }
    Ok(out)
}

fn walk_bookmarks(node: &Value, path: &mut Vec<String>, out: &mut Vec<Bookmark>, is_root: bool) {
    let name = node.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
    match node.get("type").and_then(|t| t.as_str()) {
        Some("url") => {
            if let Some(url) = node.get("url").and_then(|u| u.as_str()) {
                out.push(Bookmark { title: name, url: url.to_string(), folder: path.clone() });
            }
        }
        Some("folder") => {
            let pushed = !name.is_empty() || is_root;
            if pushed {
                path.push(name);
            }
            for c in node.get("children").and_then(|c| c.as_array()).into_iter().flatten() {
                walk_bookmarks(c, path, out, false);
            }
            if pushed {
                path.pop();
            }
        }
        _ => {}
    }
}

/// Chrome stores time as microseconds since 1601-01-01 UTC.
pub fn chrome_time_to_unix(t: i64) -> i64 {
    const EPOCH_DELTA_SECS: i64 = 11_644_473_600;
    t / 1_000_000 - EPOCH_DELTA_SECS
}

/// Read the most recent `limit` URLs from the `History` SQLite database.
pub fn read_history(profile: &Path, limit: usize) -> Result<Vec<HistoryEntry>, Error> {
    let src = profile.join("History");
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("relay-chrome-history-{}-{nonce}.sqlite", std::process::id()));
    // The copy holds the user's browsing history: create it 0600 in one step.
    // If anything fails, the copy is removed before the error leaves here.
    copy_private(&src, &tmp)?;
    let result = (|| {
        let conn = rusqlite::Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = conn.prepare(
            "SELECT url, title, visit_count, last_visit_time FROM urls WHERE hidden = 0 ORDER BY last_visit_time DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visit_count: r.get(2)?,
                last_visit: chrome_time_to_unix(r.get(3)?),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Copy `src` into a new `dst` that nobody else can read. On any failure the
/// partial destination is removed, so an interrupted import never leaves a
/// readable history database in the temp directory.
fn copy_private(src: &Path, dst: &Path) -> Result<(), Error> {
    let result = (|| -> Result<(), Error> {
        let mut input = std::fs::File::open(src)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(dst)?;
        std::io::copy(&mut input, &mut output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// List installed extensions from `Extensions/<id>/<version>/manifest.json`,
/// resolving `__MSG_name__` through `_locales`.
pub fn read_extensions(profile: &Path) -> Result<Vec<InstalledExtension>, Error> {
    let mut out = vec![];
    let dir = profile.join("Extensions");
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !is_extension_id(&id) {
            continue;
        }
        // Highest version directory wins.
        let mut versions: Vec<_> = std::fs::read_dir(entry.path())?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").is_file()).collect();
        versions.sort();
        let Some(vdir) = versions.pop() else { continue };
        let m = crate::manifest::Manifest::read(&vdir)?;
        out.push(InstalledExtension { id, name: m.display_name(&vdir), version: m.version.clone() });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Parse Chrome's password export CSV (name,url,username,password[,note]).
pub fn read_password_csv(path: &Path) -> Result<Vec<Credential>, Error> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_path(path)?;
    let mut out = vec![];
    for row in rdr.deserialize() {
        let c: Credential = row?;
        if !c.url.is_empty() && !c.password.is_empty() {
            out.push(c);
        }
    }
    Ok(out)
}

/// Everything found in a profile, for the import preview.
#[derive(Debug, Default)]
pub struct ImportPreview {
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryEntry>,
    pub extensions: Vec<InstalledExtension>,
    pub errors: Vec<String>,
}

pub fn preview(profile: &Path, history_limit: usize) -> ImportPreview {
    let mut p = ImportPreview::default();
    match read_bookmarks(profile) {
        Ok(b) => p.bookmarks = b,
        Err(e) => p.errors.push(format!("bookmarks: {e}")),
    }
    match read_history(profile, history_limit) {
        Ok(h) => p.history = h,
        Err(e) => p.errors.push(format!("history: {e}")),
    }
    match read_extensions(profile) {
        Ok(x) => p.extensions = x,
        Err(e) => p.errors.push(format!("extensions: {e}")),
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_copy_is_0600_and_cleans_up_on_failure() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("History");
        let dst = d.path().join("copy.sqlite");
        std::fs::write(&src, b"sqlite").unwrap();
        copy_private(&src, &dst).unwrap();
        assert_eq!(std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_file(&dst).unwrap();
        assert!(copy_private(&d.path().join("missing"), &dst).is_err());
        assert!(!dst.exists(), "a failed import leaves no temp copy");
    }

    fn fake_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(
            p.join("Bookmarks"),
            r#"{"roots":{
                "bookmark_bar":{"type":"folder","name":"Bookmarks bar","children":[
                    {"type":"url","name":"Rust","url":"https://www.rust-lang.org/"},
                    {"type":"folder","name":"Work","children":[{"type":"url","name":"CI","url":"https://ci.test/"}]}]},
                "other":{"type":"folder","name":"Other bookmarks","children":[]},
                "synced":{"type":"folder","name":"Mobile bookmarks","children":[{"type":"url","name":"M","url":"https://m.test/"}]}
            },"version":1}"#,
        )
        .unwrap();
        let conn = rusqlite::Connection::open(p.join("History")).unwrap();
        conn.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
             INSERT INTO urls VALUES(1,'https://old.test/','Old',1,0,13000000000000000,0);
             INSERT INTO urls VALUES(2,'https://new.test/','New',9,1,13390000000000000,0);
             INSERT INTO urls VALUES(3,'https://hidden.test/','H',1,0,13390000000000001,1);",
        )
        .unwrap();
        let ext = p.join("Extensions/cjpalhdlnbpafiamejdnhcphjbkeiagm");
        for v in ["1.0.0_0", "1.2.0_0"] {
            std::fs::create_dir_all(ext.join(v).join("_locales/en")).unwrap();
            std::fs::write(ext.join(v).join("manifest.json"), format!(r#"{{"manifest_version":3,"name":"__MSG_extName__","version":"{}","default_locale":"en"}}"#, &v[..5])).unwrap();
            std::fs::write(ext.join(v).join("_locales/en/messages.json"), r#"{"extName":{"message":"uBlock Origin"}}"#).unwrap();
        }
        std::fs::create_dir_all(p.join("Extensions/Temp")).unwrap();
        dir
    }

    #[test]
    fn bookmarks_keep_folder_paths() {
        let d = fake_profile();
        let b = read_bookmarks(d.path()).unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[1], Bookmark { title: "CI".into(), url: "https://ci.test/".into(), folder: vec!["Bookmarks bar".into(), "Work".into()] });
        assert_eq!(b[2].folder, vec!["Mobile bookmarks".to_string()]);
    }

    #[test]
    fn history_newest_first_skips_hidden_and_converts_time() {
        let d = fake_profile();
        let h = read_history(d.path(), 10).unwrap();
        assert_eq!(h.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), ["https://new.test/", "https://old.test/"]);
        // 13390000000000000 µs since 1601 = 2025-04-24.
        assert_eq!(h[0].last_visit, 1_745_526_400);
        assert!(d.path().join("History").exists(), "Chrome's file is untouched");
    }

    #[test]
    fn extensions_resolve_localized_name_and_latest_version() {
        let d = fake_profile();
        let x = read_extensions(d.path()).unwrap();
        assert_eq!(x, vec![InstalledExtension { id: "cjpalhdlnbpafiamejdnhcphjbkeiagm".into(), name: "uBlock Origin".into(), version: "1.2.0".into() }]);
    }

    #[test]
    fn password_csv_and_redacted_debug() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Chrome Passwords.csv");
        std::fs::write(&f, "name,url,username,password,note\nshop.test,https://shop.test/login,me@x.test,hunter2,\nempty,https://e.test,,,\n").unwrap();
        let c = read_password_csv(&f).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].username, "me@x.test");
        assert!(!format!("{:?}", c[0]).contains("hunter2"));
    }

    #[test]
    fn preview_collects_errors_instead_of_failing() {
        let d = tempfile::tempdir().unwrap();
        let p = preview(d.path(), 10);
        assert_eq!(p.errors.len(), 2, "{:?}", p.errors); // bookmarks + history missing; no Extensions dir is fine
    }
}
