//! The new-tab page's data: which sections show, and which pages, apps, and
//! bookmarks fill them. Pure logic over the local profile; `app.rs` paints it.
//!
//! The page is deliberately made of known sections rather than user HTML:
//! the same shape can later be read and changed by the shell MCP without
//! giving an agent a code-execution primitive in the browser process.

use relay_extensions::store::{Store, WebAppRecord};
use std::collections::HashSet;

/// Rows per section: enough to be useful, small enough that four sections
/// fit a 980×760 window without scrolling.
pub const MAX_ROWS: usize = 5;

/// The sections a new tab can show, in their default order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Recent,
    Frequent,
    Apps,
    Bookmarks,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::Recent, Section::Frequent, Section::Apps, Section::Bookmarks];

    pub fn as_str(self) -> &'static str {
        match self {
            Section::Recent => "recent",
            Section::Frequent => "frequent",
            Section::Apps => "apps",
            Section::Bookmarks => "bookmarks",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::Recent => "Recent",
            Section::Frequent => "Frequent",
            Section::Apps => "Apps",
            Section::Bookmarks => "Bookmarks",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "recent" | "history" => Some(Section::Recent),
            "frequent" | "top" => Some(Section::Frequent),
            "apps" | "applications" => Some(Section::Apps),
            "bookmarks" => Some(Section::Bookmarks),
            _ => None,
        }
    }
}

/// `start_sections` accepts `recent,frequent,apps,bookmarks` in any order
/// (spaces too). Unknown names are ignored, duplicates collapse, and an empty
/// or all-unknown value falls back to the default order. `none` explicitly
/// hides every section.
pub fn parse_sections(raw: Option<&str>) -> Vec<Section> {
    let Some(raw) = raw else { return Section::ALL.to_vec() };
    if raw.trim().eq_ignore_ascii_case("none") {
        return vec![];
    }
    let mut seen = HashSet::new();
    let mut sections = Vec::new();
    for token in raw.split([',', ' ']) {
        if let Some(section) = Section::parse(token) {
            if seen.insert(section) {
                sections.push(section);
            }
        }
    }
    if sections.is_empty() { Section::ALL.to_vec() } else { sections }
}

/// One row on the new-tab page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartItem {
    pub title: String,
    pub url: String,
    /// Secondary line: host, folder, visit count, or "App".
    pub note: String,
}

/// One configured section and its rows. An empty section is kept so the page
/// can say "Nothing yet" instead of silently changing shape.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartPage {
    pub sections: Vec<(Section, Vec<StartItem>)>,
}

impl StartPage {
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|(_, items)| items.is_empty())
    }

    pub fn build(store: &Store, raw_sections: Option<&str>) -> relay_extensions::Result<Self> {
        let wanted = parse_sections(raw_sections);
        let recent_visits = store.recent_sites(MAX_ROWS)?;
        let recent_urls: HashSet<String> = recent_visits.iter().map(|v| v.url.clone()).collect();
        let frequent: Vec<_> = store
            .frequent_sites(24)?
            .into_iter()
            .filter(|v| !recent_urls.contains(&v.url))
            .take(MAX_ROWS)
            .collect();
        let apps = store.web_apps()?.into_iter().take(MAX_ROWS).collect::<Vec<_>>();
        let bookmarks = store.bookmarks()?.into_iter().take(MAX_ROWS).collect::<Vec<_>>();

        let sections = wanted
            .into_iter()
            .map(|section| {
                let items = match section {
                    Section::Recent => recent_visits.iter().map(visit_item).collect(),
                    Section::Frequent => frequent.iter().map(visit_item).collect(),
                    Section::Apps => apps.iter().map(app_item).collect(),
                    Section::Bookmarks => bookmarks.iter().map(bookmark_item).collect(),
                };
                (section, items)
            })
            .collect();
        Ok(Self { sections })
    }
}

fn visit_item(v: &relay_extensions::store::Visit) -> StartItem {
    StartItem {
        title: if v.title.trim().is_empty() { short_url(&v.url) } else { v.title.clone() },
        url: v.url.clone(),
        note: if v.visit_count > 1 { format!("{} visits · {}", v.visit_count, short_url(&v.url)) } else { short_url(&v.url) },
    }
}

fn app_item(app: &WebAppRecord) -> StartItem {
    StartItem { title: app.name.clone(), url: app.start_url.clone(), note: "App".to_string() }
}

fn bookmark_item(b: &relay_extensions::chrome_import::Bookmark) -> StartItem {
    let folder = if b.folder.is_empty() { "Bookmark".to_string() } else { b.folder.join(" / ") };
    StartItem {
        title: if b.title.trim().is_empty() { short_url(&b.url) } else { b.title.clone() },
        url: b.url.clone(),
        note: folder,
    }
}

pub fn short_url(url: &str) -> String {
    let s = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    let s = s.strip_prefix("www.").unwrap_or(s);
    if s.chars().count() > 48 { format!("{}…", s.chars().take(48).collect::<String>()) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let s = Store::in_memory().unwrap();
        let now = 1_800_000_000;
        for _ in 0..3 {
            s.record_visit("https://often.test/page", "Often", now).unwrap();
        }
        for i in 0..10 {
            s.record_visit(&format!("https://fresh.test/{i}"), &format!("Fresh {i}"), now + 10 + i).unwrap();
        }
        s.toggle_bookmark("https://book.test/", "Book").unwrap();
        s.upsert_web_app(&WebAppRecord {
            id: "app.relay.webapp.one".into(),
            name: "Shop".into(),
            start_url: "https://shop.test/app".into(),
            bundle_path: "/tmp/Shop.app".into(),
            installed_at: now,
        })
        .unwrap();
        s
    }

    #[test]
    fn sections_parse_in_order_dedupe_and_fall_back() {
        assert_eq!(parse_sections(None), Section::ALL.to_vec());
        assert_eq!(parse_sections(Some("")), Section::ALL.to_vec());
        assert_eq!(parse_sections(Some("bookmarks, recent bookmarks")), vec![Section::Bookmarks, Section::Recent]);
        assert_eq!(parse_sections(Some("history top apps")), vec![Section::Recent, Section::Frequent, Section::Apps]);
        assert_eq!(parse_sections(Some("nope")), Section::ALL.to_vec());
        assert!(parse_sections(Some("none")).is_empty());
        assert_eq!(parse_sections(Some("RECENT")), vec![Section::Recent]);
    }

    #[test]
    fn page_fills_known_sections_and_skips_frequent_duplicates() {
        let page = StartPage::build(&store(), Some("recent,frequent,apps,bookmarks")).unwrap();
        assert_eq!(page.sections.len(), 4);
        let by = |s: Section| page.sections.iter().find(|(x, _)| *x == s).map(|(_, i)| i).unwrap();
        let recent = by(Section::Recent);
        assert_eq!(recent.len(), MAX_ROWS);
        assert_eq!(recent[0].title, "Fresh 9", "recent is recency-ranked");
        assert!(!recent.iter().any(|i| i.title == "Often"), "ten fresh pages push Often out of five recents");
        let frequent = by(Section::Frequent);
        assert!(frequent.iter().any(|i| i.title == "Often"));
        assert!(frequent.iter().all(|i| !recent.iter().any(|r| r.url == i.url)), "frequent does not repeat recent rows");
        assert!(frequent.iter().any(|i| i.note.contains("3 visits")));
        assert_eq!(by(Section::Apps)[0].title, "Shop");
        assert_eq!(by(Section::Apps)[0].note, "App");
        assert_eq!(by(Section::Bookmarks)[0].title, "Book");
        assert_eq!(by(Section::Bookmarks)[0].note, "Bookmark");
    }

    #[test]
    fn empty_profile_is_an_empty_page_with_configured_shape() {
        let s = Store::in_memory().unwrap();
        let page = StartPage::build(&s, Some("recent,apps")).unwrap();
        assert!(page.is_empty());
        assert_eq!(page.sections.len(), 2);
        assert!(page.sections.iter().all(|(_, items)| items.is_empty()));
    }

    #[test]
    fn notes_and_titles_survive_missing_metadata() {
        let s = Store::in_memory().unwrap();
        s.record_visit("https://www.example.test/a/very/long/path/that/needs/shortening/for/the/page", "", 1).unwrap();
        let page = StartPage::build(&s, Some("recent")).unwrap();
        let item = &page.sections[0].1[0];
        assert!(item.title.starts_with("example.test/"));
        assert!(item.note.starts_with("example.test/"));
        assert!(item.title.chars().count() <= 49);
    }
}
