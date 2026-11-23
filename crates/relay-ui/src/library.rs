//! History and bookmarks library data: filtering, rows, and selection.
//!
//! The overlay is a modal like the palette, so the model is pure and the
//! `app.rs` view only paints rows and forwards keys.

use relay_extensions::store::{Store, Visit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    History,
    Bookmarks,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self {
            Mode::History => "History",
            Mode::Bookmarks => "Bookmarks",
        }
    }

    pub fn empty(self) -> &'static str {
        match self {
            Mode::History => "No history yet",
            Mode::Bookmarks => "No bookmarks yet",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Mode::History => "Click a row to open it, × to delete",
            Mode::Bookmarks => "Click to open · move sets a folder · × removes"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub title: String,
    pub url: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub mode: Mode,
    pub query: String,
    pub selected: usize,
    pub rows: Vec<Row>,
    /// Non-`None` while the user edits a bookmark's folder.
    pub folder_input: Option<String>,
}

impl Library {
    pub fn open(store: &Store, mode: Mode) -> Result<Self, relay_extensions::Error> {
        let mut lib = Self { mode, query: String::new(), selected: 0, rows: vec![], folder_input: None };
        lib.refresh(store)?;
        Ok(lib)
    }

    pub fn refresh(&mut self, store: &Store) -> Result<(), relay_extensions::Error> {
        self.rows = match self.mode {
            Mode::History => history_rows(store, &self.query)?,
            Mode::Bookmarks => bookmark_rows(store, &self.query)?,
        };
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        Ok(())
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(n)) as usize;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn chosen(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }
}

pub fn history_rows(store: &Store, query: &str) -> Result<Vec<Row>, relay_extensions::Error> {
    let visits: Vec<Visit> = if query.trim().is_empty() { store.recent_sites(200)? } else { store.search_history(query, 200)? };
    let now = now_unix();
    Ok(visits
        .into_iter()
        .map(|v| {
            let title = if v.title.trim().is_empty() { short_url(&v.url) } else { v.title };
            let visits = if v.visit_count > 1 { format!("{} visits", v.visit_count) } else { String::new() };
            let note = [when(v.last_visit, now), visits, short_url(&v.url)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            Row { title, url: v.url, note }
        })
        .collect())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A short, human time for a history row without a date library.
pub fn when(last_visit: i64, now: i64) -> String {
    if last_visit <= 0 || now <= 0 {
        return "unknown time".to_string();
    }
    let seconds = now.saturating_sub(last_visit).max(0);
    let days = seconds / 86_400;
    match days {
        0 if seconds < 3600 => "just now".to_string(),
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        2..=29 => format!("{days}d ago"),
        30..=364 => format!("{}mo ago", days / 30),
        _ => format!("{}y ago", days / 365),
    }
}

pub fn bookmark_rows(store: &Store, query: &str) -> Result<Vec<Row>, relay_extensions::Error> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<Row> = store
        .bookmarks()?
        .into_iter()
        .filter(|b| q.is_empty() || b.title.to_lowercase().contains(&q) || b.url.to_lowercase().contains(&q) || b.folder.join("/").to_lowercase().contains(&q))
        .map(|b| {
            let title = if b.title.trim().is_empty() { short_url(&b.url) } else { b.title };
            let note = if b.folder.is_empty() { "Bookmark".to_string() } else { b.folder.join(" / ") };
            Row { title, url: b.url, note }
        })
        .collect();
    rows.sort_by(|a, b| {
        // Foldered bookmarks first, then by folder text, then title.
        (a.note == "Bookmark")
            .cmp(&(b.note == "Bookmark"))
            .then_with(|| a.note.to_lowercase().cmp(&b.note.to_lowercase()))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    Ok(rows)
}

pub fn short_url(url: &str) -> String {
    let s = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    let s = s.strip_prefix("www.").unwrap_or(s);
    if s.chars().count() > 54 { format!("{}…", s.chars().take(54).collect::<String>()) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_extensions::chrome_import::Bookmark;

    fn store() -> Store {
        let s = Store::in_memory().unwrap();
        s.record_visit("https://fresh.test/", "Fresh", 200).unwrap();
        s.record_visit("https://often.test/", "Often", 100).unwrap();
        s.record_visit("https://often.test/", "Often", 101).unwrap();
        s.add_bookmark(&Bookmark { title: "Rust".into(), url: "https://rust.test/".into(), folder: vec!["Dev".into(), "Lang".into()] }).unwrap();
        s.add_bookmark(&Bookmark { title: "News".into(), url: "https://news.test/".into(), folder: vec![] }).unwrap();
        s
    }

    #[test]
    fn history_opens_newest_first_and_filters() {
        let s = store();
        let mut lib = Library::open(&s, Mode::History).unwrap();
        assert_eq!(lib.rows[0].title, "Fresh");
        assert!(lib.rows[0].note.ends_with("fresh.test"), "{}", lib.rows[0].note);
        lib.query = "often".into();
        lib.refresh(&s).unwrap();
        assert_eq!(lib.rows.len(), 1);
        assert!(lib.rows[0].note.contains("2 visits") && lib.rows[0].note.ends_with("often.test"), "{}", lib.rows[0].note);
        lib.query = "nothing".into();
        lib.refresh(&s).unwrap();
        assert!(lib.rows.is_empty());
        assert_eq!(lib.chosen(), None);
        lib.move_selection(1);
        assert_eq!(lib.selected, 0);
    }

    #[test]
    fn bookmarks_show_folders_and_match_folder_text() {
        let s = store();
        let mut lib = Library::open(&s, Mode::Bookmarks).unwrap();
        assert_eq!(lib.rows.len(), 2);
        assert_eq!(lib.rows[0].title, "Rust", "foldered rows sort before the bare bookmark");
        assert_eq!(lib.rows[0].note, "Dev / Lang");
        assert_eq!(lib.rows[1].note, "Bookmark");
        lib.query = "lang".into();
        lib.refresh(&s).unwrap();
        assert_eq!(lib.rows.len(), 1);
        assert_eq!(lib.rows[0].url, "https://rust.test/");
    }

    #[test]
    fn relative_time_is_stable_for_old_rows() {
        assert_eq!(when(0, 1_800_000_000), "unknown time");
        assert_eq!(when(1_800_000_000, 1_800_000_000), "just now");
        assert_eq!(when(1_800_000_000 - 86_400, 1_800_000_000), "yesterday");
        assert_eq!(when(1_800_000_000 - 86_400 * 5, 1_800_000_000), "5d ago");
        assert_eq!(when(1_800_000_000 - 86_400 * 400, 1_800_000_000), "1y ago");
    }

    #[test]
    fn selection_wraps_and_clamps_after_filtering() {
        let s = store();
        let mut lib = Library::open(&s, Mode::History).unwrap();
        assert_eq!(lib.rows.len(), 2);
        lib.selected = 1;
        lib.move_selection(1);
        assert_eq!(lib.selected, 0);
        lib.move_selection(-1);
        assert_eq!(lib.selected, 1);
        lib.query = "nothing".into();
        lib.refresh(&s).unwrap();
        assert_eq!(lib.selected, 0);
        assert_eq!(lib.chosen(), None);
    }
}
