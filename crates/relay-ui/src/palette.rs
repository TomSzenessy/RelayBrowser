//! ⌘K palette: one input that opens URLs, searches, switches tabs, recalls
//! history and bookmarks, and runs commands. Pure logic; the GPUI view only
//! renders `Palette::items`.

use relay_bridge::TabId;

pub const DEFAULT_SEARCH: &str = "https://duckduckgo.com/?q=%s";

pub const SEARCH_PRESETS: &[(&str, &str)] = &[
    ("DuckDuckGo", DEFAULT_SEARCH),
    ("Kagi", "https://kagi.com/search?q=%s"),
    ("Google", "https://www.google.com/search?q=%s"),
    ("Brave", "https://search.brave.com/search?q=%s"),
];

pub fn search_label(template: &str) -> &'static str {
    SEARCH_PRESETS.iter().find(|(_, url)| *url == template).map(|(name, _)| *name).unwrap_or("Custom")
}

const MAX_ITEMS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    CloseTab,
    ReopenClosedTab,
    Reload,
    Back,
    Forward,
    ToggleInspector,
    WebInspector,
    ToggleSplit,
    LinkHints,
    Settings,
    ImportChrome,
    CopyMcpSetup,
    CopyCleanUrl,
    CopyMarkdown,
    CloseDuplicateTabs,
    ToggleBookmark,
    TogglePin,
    Find,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleSiteBlocking,
    ToggleCompatMode,
    ToggleTabStrip,
    NextTabPlacement,
    InstallWebApp,
    RemoveWebApp,
    ShowHistory,
    ShowBookmarks,
    ToggleAgents,
    CycleAgentPolicy,
    CycleTheme,
    CycleAccent,
    ToggleReader,
    TogglePictureInPicture,
}

impl Command {
    pub const ALL: [Command; 36] = [
        Command::NewTab,
        Command::CloseTab,
        Command::ReopenClosedTab,
        Command::Reload,
        Command::Back,
        Command::Forward,
        Command::ToggleInspector,
        Command::WebInspector,
        Command::ToggleSplit,
        Command::LinkHints,
        Command::Settings,
        Command::ImportChrome,
        Command::CopyMcpSetup,
        Command::CopyCleanUrl,
        Command::CopyMarkdown,
        Command::CloseDuplicateTabs,
        Command::ToggleBookmark,
        Command::TogglePin,
        Command::Find,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::ZoomReset,
        Command::ToggleSiteBlocking,
        Command::ToggleCompatMode,
        Command::ToggleTabStrip,
        Command::NextTabPlacement,
        Command::InstallWebApp,
        Command::RemoveWebApp,
        Command::ShowHistory,
        Command::ShowBookmarks,
        Command::ToggleAgents,
        Command::CycleAgentPolicy,
        Command::CycleTheme,
        Command::CycleAccent,
        Command::ToggleReader,
        Command::TogglePictureInPicture,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Command::NewTab => "New Tab",
            Command::CloseTab => "Close Tab",
            Command::ReopenClosedTab => "Reopen Closed Tab",
            Command::Reload => "Reload Page",
            Command::Back => "Go Back",
            Command::Forward => "Go Forward",
            Command::ToggleInspector => "Toggle Inspector",
            Command::WebInspector => "Open WebKit Web Inspector",
            Command::ToggleSplit => "Toggle Split View",
            Command::LinkHints => "Link Hints",
            Command::Settings => "Settings",
            Command::ImportChrome => "Import from Chrome",
            Command::CopyMcpSetup => "Copy MCP Setup Command",
            Command::CopyCleanUrl => "Copy Clean URL",
            Command::CopyMarkdown => "Copy Page as Markdown",
            Command::CloseDuplicateTabs => "Close Duplicate Tabs",
            Command::ToggleBookmark => "Bookmark This Page",
            Command::TogglePin => "Pin / Unpin Tab",
            Command::Find => "Find in Page",
            Command::ZoomIn => "Zoom In",
            Command::ZoomOut => "Zoom Out",
            Command::ZoomReset => "Actual Size",
            Command::ToggleSiteBlocking => "Toggle Blocking on This Site",
            Command::ToggleCompatMode => "Toggle Compatibility Mode on This Site",
            Command::ToggleTabStrip => "Fold / Unfold Tab Strip",
            Command::NextTabPlacement => "Move Tab Strip",
            Command::InstallWebApp => "Install This Site as an App",
            Command::RemoveWebApp => "Remove This Site's App",
            Command::ShowHistory => "Show History",
            Command::ShowBookmarks => "Show Bookmarks",
            Command::ToggleAgents => "Pause / Resume Agents",
            Command::CycleAgentPolicy => "Cycle Agent Access on This Site",
            Command::CycleTheme => "Cycle Appearance (System / Light / Dark)",
            Command::CycleAccent => "Cycle Accent Colour",
            Command::ToggleReader => "Reading Mode",
            Command::TogglePictureInPicture => "Picture in Picture",
        }
    }

    pub fn keys(self) -> &'static str {
        match self {
            Command::NewTab => "⌘T",
            Command::CloseTab => "⌘W",
            Command::ReopenClosedTab => "⌘⇧T",
            Command::Reload => "⌘R",
            Command::Back => "⌘[",
            Command::Forward => "⌘]",
            Command::ToggleInspector => "⌘⌥I",
            Command::WebInspector => "⌘⌥U",
            Command::ToggleSplit => "⌘\\",
            Command::LinkHints => "⌘J",
            Command::Settings => "⌘,",
            Command::CopyCleanUrl => "⌘⇧C",
            Command::CopyMarkdown => "⌘⇧M",
            Command::ToggleBookmark => "⌘D",
            Command::TogglePin => "⌘⌥P",
            Command::Find => "⌘F",
            Command::ZoomIn => "⌘=",
            Command::ZoomOut => "⌘-",
            Command::ZoomReset => "⌘0",
            Command::ToggleTabStrip => "⌘S",
            Command::NextTabPlacement => "⇧⌘S",
            Command::ImportChrome
            | Command::CopyMcpSetup
            | Command::CloseDuplicateTabs
            | Command::ToggleSiteBlocking
            | Command::ToggleCompatMode
            | Command::InstallWebApp
            | Command::RemoveWebApp => "",
            Command::ShowHistory => "⌘Y",
            Command::ShowBookmarks => "⇧⌘B",
            Command::ToggleAgents => "⇧⌘.",
            Command::CycleAgentPolicy => "⌘⌥A",
            Command::CycleTheme => "",
            Command::CycleAccent => "",
            Command::ToggleReader => "⇧⌘R",
            Command::TogglePictureInPicture => "⇧⌘P",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Open(String),
    /// An offline `!bang` hit: opens `url`, shown as `label` (`Bang: host/path`).
    Bang { label: String, url: String },
    /// An inline calculator result; Enter copies it.
    Calc(String),
    Search { query: String, url: String },
    Tab { id: TabId, title: String, url: String },
    History { title: String, url: String },
    Bookmark { title: String, url: String },
    Command(Command),
}

impl Item {
    pub fn title(&self) -> String {
        match self {
            Item::Open(u) => format!("Open {u}"),
            Item::Bang { label, .. } => label.clone(),
            Item::Calc(r) => format!("= {r}"),
            Item::Search { query, .. } => format!("Search “{query}”"),
            Item::Tab { title, url, .. } | Item::History { title, url } | Item::Bookmark { title, url } => {
                if title.is_empty() { url.clone() } else { title.clone() }
            }
            Item::Command(c) => c.title().to_string(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Item::Open(_) | Item::Bang { .. } | Item::Search { .. } => String::new(),
            Item::Calc(_) => "Enter to copy".to_string(),
            Item::Tab { url, .. } => format!("Tab · {}", short_url(url)),
            Item::History { url, .. } => format!("History · {}", short_url(url)),
            Item::Bookmark { url, .. } => format!("Bookmark · {}", short_url(url)),
            Item::Command(c) => c.keys().to_string(),
        }
    }
}

pub fn short_url(url: &str) -> String {
    let s = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    let s = s.strip_prefix("www.").unwrap_or(s);
    if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s.to_string() }
}

/// Does typed text look like an address rather than a search?
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if ["http://", "https://", "about:", "file://"].iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    let host = lower.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let name = host.split(':').next().unwrap_or(host);
    if name == "localhost" || name.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    match name.rsplit_once('.') {
        Some((left, tld)) => !left.is_empty() && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()),
        None => false,
    }
}

/// Turn typed text into the URL to load.
pub fn resolve(input: &str, search_template: &str) -> Item {
    let t = input.trim();
    if looks_like_url(t) {
        let lower = t.to_ascii_lowercase();
        let local = lower.starts_with("localhost") || lower.split([':', '/']).next().is_some_and(|h| h.parse::<std::net::Ipv4Addr>().is_ok());
        let url = if t.contains("://") || lower.starts_with("about:") {
            t.to_string()
        } else if local {
            format!("http://{t}")
        } else {
            format!("https://{t}")
        };
        Item::Open(url)
    } else {
        Item::Search { query: t.to_string(), url: search_template.replace("%s", &encode_query(t)) }
    }
}

fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fuzzy subsequence score (higher is better); `None` when not all query
/// characters appear in order. Rewards prefixes, word starts, and runs.
#[cfg_attr(not(test), allow(dead_code))]
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    fuzzy_score_chars(&q, candidate)
}

/// Allocation-free scorer. `q` is already lowercased and whitespace-free;
/// the candidate is lowercased as it is iterated.
pub fn fuzzy_score_chars(q: &[char], candidate: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let mut score = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    let mut prev_char: Option<char> = None;
    let mut len = 0usize;
    for (i, ch) in candidate.chars().flat_map(char::to_lowercase).enumerate() {
        len = i + 1;
        if qi < q.len() && ch == q[qi] {
            score += 1;
            if i == 0 {
                score += 8;
            } else if prev_char.is_some_and(|p| !p.is_alphanumeric()) {
                score += 5;
            }
            if prev == Some(i.wrapping_sub(1)) {
                score += 4;
            }
            prev = Some(i);
            qi += 1;
        }
        prev_char = Some(ch);
    }
    if qi < q.len() {
        return None;
    }
    Some(score * 10 - len.min(200) as i32 / 10)
}

/// The palette's candidate sources, filled by the shell.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    pub tabs: Vec<(TabId, String, String)>,
    pub history: Vec<(String, String)>,
    pub bookmarks: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
    pub items: Vec<Item>,
    /// User-defined `(trigger, template-with-%s)` bangs; override built-ins.
    pub custom_bangs: Vec<(String, String)>,
}

impl Palette {
    pub fn refresh(&mut self, sources: &Sources, search_template: &str) {
        let q = self.query.trim();
        let qchars: Vec<char> = q.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let mut scored: Vec<(i32, Item)> = vec![];
        for (id, title, url) in &sources.tabs {
            if let Some(s) = best_chars(&qchars, title, url) {
                scored.push((s + 300, Item::Tab { id: *id, title: title.clone(), url: url.clone() }));
            }
        }
        for (title, url) in &sources.bookmarks {
            if let Some(s) = best_chars(&qchars, title, url) {
                scored.push((s + 200, Item::Bookmark { title: title.clone(), url: url.clone() }));
            }
        }
        for (rank, (title, url)) in sources.history.iter().enumerate() {
            if let Some(s) = best_chars(&qchars, title, url) {
                // History arrives pre-ranked by frecency.
                scored.push((s + 100 - rank as i32, Item::History { title: title.clone(), url: url.clone() }));
            }
        }
        for c in Command::ALL {
            if q.is_empty() {
                continue;
            }
            if let Some(s) = fuzzy_score_chars(&qchars, c.title()) {
                scored.push((s + 250, Item::Command(c)));
            }
        }
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        let mut seen = std::collections::HashSet::new();
        let mut items: Vec<Item> = vec![];
        if let Some(url) = crate::bangs::resolve(q, &self.custom_bangs) {
            items.push(Item::Bang { label: format!("Bang: {}", short_url(&url)), url });
        } else {
            if let Some(result) = crate::calc::eval(q) {
                items.push(Item::Calc(result));
            }
            if !q.is_empty() {
                items.push(resolve(q, search_template));
            }
        }
        for (_, item) in scored {
            let key = match &item {
                Item::Tab { url, .. } | Item::History { url, .. } | Item::Bookmark { url, .. } => url.clone(),
                other => other.title(),
            };
            // An open tab beats a history row for the same URL.
            if seen.insert(key) {
                items.push(item);
            }
            if items.len() >= MAX_ITEMS {
                break;
            }
        }
        if q.is_empty() {
            items.extend(Command::ALL.iter().take(MAX_ITEMS.saturating_sub(items.len())).map(|c| Item::Command(*c)));
        }
        self.items = items;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(n)) as usize;
    }

    pub fn chosen(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }
}

fn best_chars(q: &[char], title: &str, url: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let t = fuzzy_score_chars(q, title);
    let u = fuzzy_score_chars(q, short_str(url)).map(|s| s - 5);
    t.max(u)
}

/// The scoring view of a URL (`host/path`), borrowed instead of allocated.
fn short_str(url: &str) -> &str {
    let s = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    s.strip_prefix("www.").unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        for yes in ["example.com", "docs.rs/gpui", "localhost:3000", "127.0.0.1:8080/x", "https://a", "about:blank", "sub.domain.co.uk/path?q=1"] {
            assert!(looks_like_url(yes), "{yes}");
        }
        // "file.rs" is genuinely ambiguous (.rs is a TLD) and opens as a URL.
        for no in ["rust gpui tutorial", "hello", "what is 2.5", "v1.2", ".com"] {
            assert!(!looks_like_url(no), "{no}");
        }
    }

    #[test]
    fn resolve_adds_schemes_and_builds_searches() {
        assert_eq!(resolve("docs.rs", DEFAULT_SEARCH), Item::Open("https://docs.rs".into()));
        assert_eq!(resolve("localhost:3000", DEFAULT_SEARCH), Item::Open("http://localhost:3000".into()));
        assert_eq!(
            resolve("rust & gpui", DEFAULT_SEARCH),
            Item::Search { query: "rust & gpui".into(), url: "https://duckduckgo.com/?q=rust+%26+gpui".into() }
        );
    }

    #[test]
    fn allocation_free_scorer_matches_the_string_scorer() {
        let cases = [
            ("gh", "GitHub"), ("nt", "New Tab"), ("docs", "docs.rs/gpui"), ("rust", "Rust & GPUI"),
            ("", "anything"), ("xyz", "GitHub"), ("é", "Éditeur"), ("rel", "Relay Browser"),
        ];
        for (q, c) in cases {
            let chars: Vec<char> = q.to_lowercase().chars().filter(|ch| !ch.is_whitespace()).collect();
            assert_eq!(fuzzy_score_chars(&chars, c), fuzzy_score(q, c), "{q:?} {c:?}");
        }
    }

    #[test]
    fn fuzzy_prefers_prefix_and_word_starts() {
        let a = fuzzy_score("gh", "GitHub").unwrap();
        let b = fuzzy_score("gh", "Big thing").unwrap();
        assert!(a > b);
        assert!(fuzzy_score("xyz", "GitHub").is_none());
        assert!(fuzzy_score("nt", "New Tab").unwrap() > fuzzy_score("nt", "Reopen Closed Tab").unwrap());
    }

    #[test]
    fn palette_ranks_tabs_over_history_and_dedupes() {
        let sources = Sources {
            tabs: vec![(1, "GPUI docs".into(), "https://docs.rs/gpui".into())],
            history: vec![("GPUI docs".into(), "https://docs.rs/gpui".into()), ("gpui examples".into(), "https://github.com/zed-industries/zed".into())],
            bookmarks: vec![],
        };
        let mut p = Palette { query: "gpui".into(), ..Default::default() };
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(matches!(p.items[0], Item::Search { .. }), "typed text is always first");
        assert!(matches!(p.items[1], Item::Tab { id: 1, .. }));
        assert_eq!(p.items.iter().filter(|i| i.detail().contains("docs.rs/gpui")).count(), 1);
        p.move_selection(-1);
        assert_eq!(p.selected, p.items.len() - 1, "selection wraps");
    }

    #[test]
    fn empty_query_lists_commands() {
        let mut p = Palette::default();
        p.refresh(&Sources::default(), DEFAULT_SEARCH);
        assert_eq!(p.items.first(), Some(&Item::Command(Command::NewTab)));
    }

    #[test]
    fn search_presets_and_labels() {
        for &(name, template) in SEARCH_PRESETS {
            assert_eq!(search_label(template), name);
        }
        assert_eq!(search_label(DEFAULT_SEARCH), "DuckDuckGo");
        assert_eq!(search_label("https://example.com/search?q=%s"), "Custom");
    }

    fn top(query: &str, custom: Vec<(String, String)>) -> Item {
        let mut p = Palette { query: query.into(), custom_bangs: custom, ..Default::default() };
        p.refresh(&Sources::default(), DEFAULT_SEARCH);
        p.items[0].clone()
    }

    #[test]
    fn bang_is_top_item_labelled() {
        let item = top("!gh rust lang", vec![]);
        assert_eq!(item, Item::Bang { label: "Bang: github.com/search?q=rust+lang".into(), url: "https://github.com/search?q=rust+lang".into() });
        assert_eq!(item.title(), "Bang: github.com/search?q=rust+lang");
        assert!(matches!(top("cats !yt", vec![]), Item::Bang { .. }));
        let custom = vec![("gh".to_string(), "https://example.com/%s".to_string())];
        assert_eq!(top("!gh x", custom), Item::Bang { label: "Bang: example.com/x".into(), url: "https://example.com/x".into() });
    }

    #[test]
    fn calc_is_top_item_and_search_follows() {
        let mut p = Palette { query: "2+3*4".into(), ..Default::default() };
        p.refresh(&Sources::default(), DEFAULT_SEARCH);
        assert_eq!(p.items[0], Item::Calc("14".into()));
        assert_eq!(p.items[0].title(), "= 14");
        assert!(matches!(p.items[1], Item::Search { .. }));
    }

    #[test]
    fn ordinary_input_behaves_as_before() {
        assert_eq!(top("docs.rs", vec![]), Item::Open("https://docs.rs".into()));
        assert_eq!(top("https://x.com/a!b", vec![]), Item::Open("https://x.com/a!b".into()));
        assert!(matches!(top("hello!", vec![]), Item::Search { .. }));
        assert!(matches!(top("rust gpui tutorial", vec![]), Item::Search { .. }));
        assert!(matches!(top("!nope x", vec![]), Item::Search { .. }));
        for s in ["192.168.1.1", "1.2.3", "v1.2", "2.5", "2024-01-05", "5"] {
            assert!(!matches!(top(s, vec![]), Item::Calc(_) | Item::Bang { .. }), "{s}");
        }
    }

    #[test]
    fn new_commands_are_registered() {
        assert_eq!(Command::ALL.len(), 36);
        for c in [Command::CopyCleanUrl, Command::CopyMarkdown, Command::CloseDuplicateTabs, Command::ToggleBookmark, Command::TogglePin, Command::Find, Command::ZoomIn, Command::ZoomOut, Command::ZoomReset, Command::ToggleTabStrip, Command::NextTabPlacement, Command::InstallWebApp, Command::RemoveWebApp, Command::ShowHistory, Command::ShowBookmarks, Command::ToggleAgents, Command::CycleAgentPolicy, Command::CycleTheme, Command::CycleAccent, Command::ToggleReader, Command::TogglePictureInPicture] {
            assert!(Command::ALL.contains(&c));
            assert!(!c.title().is_empty());
        }
        assert_eq!(Command::CopyCleanUrl.keys(), "⌘⇧C");
        assert_eq!(Command::CopyMarkdown.keys(), "⌘⇧M");
        assert_eq!(Command::CloseDuplicateTabs.keys(), "");
        assert_eq!(Command::ToggleBookmark.keys(), "⌘D");
        assert_eq!(Command::TogglePin.keys(), "⌘⌥P");
        assert_eq!(Command::ToggleTabStrip.keys(), "⌘S");
        assert_eq!(Command::NextTabPlacement.keys(), "⇧⌘S");
        assert_eq!(Command::InstallWebApp.keys(), "");
        assert_eq!(Command::RemoveWebApp.keys(), "");
        let mut p = Palette { query: "duplicate".into(), ..Default::default() };
        p.refresh(&Sources::default(), DEFAULT_SEARCH);
        assert!(p.items.contains(&Item::Command(Command::CloseDuplicateTabs)));
    }
}
