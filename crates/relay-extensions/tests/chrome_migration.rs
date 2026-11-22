//! The 1-click "Import from Chrome" path end to end: a fake Chrome profile is
//! previewed, imported into a fresh on-disk Relay profile, and the imported
//! data is what the ⌘K palette searches.

use relay_extensions::chrome_import;
use relay_extensions::store::Store;

fn chrome_profile(dir: &std::path::Path) {
    std::fs::write(
        dir.join("Bookmarks"),
        r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bookmarks bar","children":[
            {"type":"url","name":"GPUI docs","url":"https://docs.rs/gpui/"},
            {"type":"folder","name":"Work","children":[{"type":"url","name":"CI","url":"https://ci.example.test/"}]}]},
          "other":{"type":"folder","name":"Other bookmarks","children":[]}},"version":1}"#,
    )
    .unwrap();
    let db = rusqlite::Connection::open(dir.join("History")).unwrap();
    db.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
         INSERT INTO urls VALUES(1,'https://docs.rs/gpui/','gpui - Rust',12,3,13390000000000000,0);
         INSERT INTO urls VALUES(2,'https://news.example.test/','News',2,0,13380000000000000,0);",
    )
    .unwrap();
    let ext = dir.join("Extensions/ddkjiahejlhfcafbddmgiahcphecmpfh/2025.1_0");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("manifest.json"), r#"{"manifest_version":3,"name":"uBlock Origin Lite","version":"2025.1","permissions":["declarativeNetRequest"]}"#).unwrap();
}

#[test]
fn import_from_chrome_then_search_it() {
    let chrome = tempfile::tempdir().unwrap();
    chrome_profile(chrome.path());
    let relay = tempfile::tempdir().unwrap();

    let preview = chrome_import::preview(chrome.path(), 10_000);
    assert!(preview.errors.is_empty(), "{:?}", preview.errors);
    assert_eq!((preview.bookmarks.len(), preview.history.len(), preview.extensions.len()), (2, 2, 1));
    assert_eq!(preview.extensions[0].name, "uBlock Origin Lite");

    let path = relay.path().join("profile.sqlite");
    let mut store = Store::open(&path).unwrap();
    let counts = store.import(&preview.bookmarks, &preview.history).unwrap();
    assert_eq!((counts.bookmarks, counts.history), (2, 2));

    // Survives a restart, and a second import adds nothing.
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let again = store.import(&preview.bookmarks, &preview.history).unwrap();
    assert_eq!((again.bookmarks, again.history), (0, 0));

    let hits = store.search_history("gpui", 5).unwrap();
    assert_eq!(hits[0].url, "https://docs.rs/gpui/");
    assert_eq!(hits[0].visit_count, 12);
    let work: Vec<_> = store.bookmarks().unwrap().into_iter().filter(|b| b.folder.last().map(String::as_str) == Some("Work")).collect();
    assert_eq!(work[0].url, "https://ci.example.test/");

    store.set_setting("sleep_minutes", "15").unwrap();
    drop(store);
    assert_eq!(Store::open(&path).unwrap().setting("sleep_minutes").unwrap().as_deref(), Some("15"));
}
