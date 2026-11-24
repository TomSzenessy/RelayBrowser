//! ⌘K palette: one input that opens URLs, searches, switches tabs, recalls
//! history and bookmarks, and runs commands. Pure logic; the GPUI view only
//! renders `Palette::items`.

use crate::keymap::{self, ActionId};
use relay_bridge::TabId;
use std::ops::Range;

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

/// One row per ⌘K command: its identity, palette label, and the keymap action
/// (if any) that carries its shortcut. `command_table!` expands this list into
/// `Command`, `ALL`, and every accessor; `keymap.rs` owns the shortcut itself.
struct CommandSpec {
    command: Command,
    action: Option<ActionId>,
    title: &'static str,
}

macro_rules! command_table {
    ($($variant:ident => $title:literal, $action:expr;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Command { $($variant),* }

        impl Command {
            /// Every palette command, in the order an empty query lists them.
            pub const ALL: &'static [Command] = &[$(Command::$variant),*];

            fn spec(self) -> &'static CommandSpec {
                static SPECS: &[CommandSpec] = &[$(
                    CommandSpec {
                        command: Command::$variant,
                        title: $title,
                        action: $action,
                    }
                ),*];
                SPECS.iter().find(|spec| spec.command == self).expect("every Command has a spec row")
            }

            /// The keymap action behind this command, `None` when palette-only.
            pub fn action(self) -> Option<ActionId> {
                self.spec().action
            }

            pub fn title(self) -> &'static str {
                self.spec().title
            }

            /// The shortcut shown next to the command, derived from the action
            /// table so a palette row can never advertise a stale key.
            pub fn keys(self) -> String {
                self.action().map(|action| keymap::shortcut_label(action.default_key())).unwrap_or_default()
            }
        }
    };
}

command_table! {
    NewTab => "New Tab", Some(ActionId::NewTab);
    CloseTab => "Close Tab", Some(ActionId::CloseTab);
    ReopenClosedTab => "Reopen Closed Tab", Some(ActionId::ReopenClosedTab);
    Reload => "Reload Page", Some(ActionId::Reload);
    Back => "Go Back", Some(ActionId::Back);
    Forward => "Go Forward", Some(ActionId::Forward);
    ToggleInspector => "Toggle Inspector", Some(ActionId::ToggleInspector);
    WebInspector => "Open WebKit Web Inspector", Some(ActionId::WebInspector);
    ToggleSplit => "Toggle Split View", Some(ActionId::ToggleSplit);
    LinkHints => "Link Hints", Some(ActionId::LinkHints);
    Settings => "Settings", Some(ActionId::ToggleSettings);
    ImportChrome => "Import from Chrome", None;
    CopyMcpSetup => "Copy MCP Setup Command", None;
    CopyCleanUrl => "Copy Clean URL", Some(ActionId::CopyCleanUrl);
    CopyMarkdown => "Copy Page as Markdown", Some(ActionId::CopyMarkdown);
    CloseDuplicateTabs => "Close Duplicate Tabs", None;
    ToggleBookmark => "Bookmark This Page", Some(ActionId::ToggleBookmark);
    TogglePin => "Pin / Unpin Tab", Some(ActionId::TogglePin);
    Find => "Find in Page", Some(ActionId::Find);
    ZoomIn => "Zoom In", Some(ActionId::ZoomIn);
    ZoomOut => "Zoom Out", Some(ActionId::ZoomOut);
    ZoomReset => "Actual Size", Some(ActionId::ZoomReset);
    ToggleSiteBlocking => "Toggle Blocking on This Site", None;
    ToggleCompatMode => "Toggle Compatibility Mode on This Site", None;
    ToggleTabStrip => "Fold / Unfold Tab Strip", Some(ActionId::ToggleTabStrip);
    NextTabPlacement => "Move Tab Strip", Some(ActionId::NextTabStripPosition);
    InstallWebApp => "Install This Site as an App", None;
    RemoveWebApp => "Remove This Site's App", None;
    ShowHistory => "Show History", Some(ActionId::ShowHistory);
    ShowBookmarks => "Show Bookmarks", Some(ActionId::ShowBookmarks);
    ToggleAgents => "Pause / Resume Agents", Some(ActionId::ToggleAgents);
    CycleAgentPolicy => "Cycle Agent Access on This Site", Some(ActionId::CycleAgentPolicy);
    CycleTheme => "Cycle Appearance (System / Light / Dark)", None;
    CycleAccent => "Cycle Accent Colour", None;
    ToggleReader => "Reading Mode", Some(ActionId::ToggleReader);
    TogglePictureInPicture => "Picture in Picture", Some(ActionId::TogglePictureInPicture);
    ShowDownloads => "Show Downloads", Some(ActionId::ShowDownloads);
    NewPrivateTab => "New Private Tab", Some(ActionId::NewPrivateTab);
    Print => "Print Page", Some(ActionId::Print);
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
            Item::Command(c) => c.keys(),
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
    /// Byte offset of the text cursor in `query`; always on a char boundary.
    pub cursor: usize,
    /// Selection anchor; `Some` when a selection is active.
    pub anchor: Option<usize>,
    pub selected: usize,
    pub items: Vec<Item>,
    /// Byte range of text the IME is currently composing, if any. Always on
    /// character boundaries. Rendering underlines this range; the platform
    /// maps it to/from UTF-16 through the helpers below.
    pub marked: Option<Range<usize>>,
    /// User-defined `(trigger, template-with-%s)` bangs; override built-ins.
    pub custom_bangs: Vec<(String, String)>,
    /// Inline address suffix after a URL prefix; `None` when nothing matches.
    pub completion: Option<String>,
}

/// Map a UTF-16 offset (the platform/IME unit) to a UTF-8 byte offset on a
/// character boundary. An offset inside a surrogate pair or inside a
/// multi-byte character floors to that character's start.
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += ch.len_utf16();
        if units > utf16 {
            return byte;
        }
    }
    text.len()
}

/// Map a UTF-8 byte offset to the number of UTF-16 code units before it.
/// An offset inside a multi-byte character floors to the character's start.
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    let mut units = 0;
    for (start, ch) in text.char_indices() {
        if start >= byte {
            return units;
        }
        if start + ch.len_utf8() > byte {
            return units;
        }
        units += ch.len_utf16();
    }
    units
}

fn char_boundary(text: &str, mut byte: usize) -> usize {
    byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// A URL prefix completion for the address field. Only a single token can
/// complete (spaces mean a search query), and the suffix is the remainder of
/// the first matching tab/bookmark/history URL. Full URLs are tried before the
/// scheme-less form so typing `https://exa` completes the real address.
fn inline_completion(query: &str, sources: &Sources) -> Option<String> {
    let q = query.trim();
    if q.chars().count() < 2 || q.chars().any(char::is_whitespace) || q.contains('=') {
        return None;
    }
    let lower = q.to_ascii_lowercase();
    let mut candidates: Vec<&str> = Vec::new();
    candidates.extend(sources.tabs.iter().map(|(_, _, url)| url.as_str()));
    candidates.extend(sources.bookmarks.iter().map(|(_, url)| url.as_str()));
    candidates.extend(sources.history.iter().map(|(_, url)| url.as_str()));
    for url in candidates.into_iter().take(800) {
        for form in [url, url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url)] {
            let low = form.to_ascii_lowercase();
            if low.starts_with(&lower) && form.len() > q.len() {
                return Some(form[q.len()..].to_string());
            }
        }
    }
    None
}

impl Palette {
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.cursor = self.query.len();
        self.anchor = None;
        self.marked = None;
        self.selected = 0;
        self.completion = None;
    }

    /// Accept the inline address completion at the caret. Returns true when
    /// something was appended; the caller refreshes the item list.
    pub fn accept_completion(&mut self) -> bool {
        let Some(suffix) = self.completion.take() else { return false };
        if self.cursor != self.query.len() || self.selection().is_some() {
            return false;
        }
        self.query.push_str(&suffix);
        self.cursor = self.query.len();
        self.anchor = None;
        self.marked = None;
        self.selected = 0;
        true
    }

    /// The platform range to replace: an explicit UTF-16 range, else the
    /// marked text, else the selection/caret.
    pub fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        if let Some(range) = range_utf16 {
            let start = utf16_to_byte(&self.query, range.start);
            let end = utf16_to_byte(&self.query, range.end);
            start.min(end)..start.max(end)
        } else if let Some(marked) = &self.marked {
            marked.clone()
        } else {
            let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
            start..end
        }
    }

    /// Replace a byte range with `text` and return the inserted byte range.
    /// This is the primitive behind both normal typing and IME replacement.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> Range<usize> {
        let start = char_boundary(&self.query, range.start);
        let end = char_boundary(&self.query, range.end).max(start);
        self.query.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
        self.marked = None;
        self.selected = 0;
        start..self.cursor
    }

    /// Replace a range with IME-composed text and mark the replacement.
    /// `selected` is relative to `text` in UTF-16 units.
    pub fn replace_and_mark(&mut self, range: Range<usize>, text: &str, selected: Option<Range<usize>>) -> Range<usize> {
        let inserted = self.replace_range(range, text);
        if !text.is_empty() {
            self.marked = Some(inserted.clone());
        }
        if let Some(selected) = selected {
            let start = inserted.start + utf16_to_byte(text, selected.start);
            let end = inserted.start + utf16_to_byte(text, selected.end);
            self.cursor = end;
            self.anchor = (start != end).then_some(start);
        }
        inserted
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            None
        } else {
            Some((anchor.min(self.cursor), anchor.max(self.cursor)))
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;
        Some(self.query[start..end].to_string())
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.query.len();
        self.marked = None;
    }

    pub fn set_cursor(&mut self, cursor: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = char_boundary(&self.query, cursor);
        self.marked = None;
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else { return false };
        self.query.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        self.marked = None;
        self.selected = 0;
        true
    }

    pub fn cut(&mut self) {
        if !self.delete_selection() && !self.query.is_empty() {
            self.clear_query();
        }
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.anchor = None;
        self.marked = None;
        self.selected = 0;
    }

    pub fn insert(&mut self, text: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.replace_range(start..end, text);
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let Some((i, _)) = self.query[..self.cursor].char_indices().next_back() else { return false };
        self.query.remove(i);
        self.cursor = i;
        self.marked = None;
        self.selected = 0;
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.query[self.cursor..].chars().next().is_none() {
            return false;
        }
        self.query.remove(self.cursor);
        self.marked = None;
        self.selected = 0;
        true
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn move_cursor(&mut self, delta: isize) {
        self.move_cursor_ext(delta, false, false);
    }

    pub fn move_cursor_ext(&mut self, delta: isize, extend: bool, by_word: bool) {
        let mut chars: Vec<usize> = self.query.char_indices().map(|(i, _)| i).collect();
        chars.push(self.query.len());
        let pos = chars.binary_search(&self.cursor).unwrap_or(0) as isize;
        let next = if by_word {
            let chars: Vec<char> = self.query.chars().collect();
            let mut p = pos as usize;
            if delta < 0 {
                while p > 0 {
                    p -= 1;
                    if chars[p].is_alphanumeric() { break; }
                }
                while p > 0 && chars[p - 1].is_alphanumeric() { p -= 1; }
            } else {
                while p < chars.len() && chars[p].is_alphanumeric() { p += 1; }
                while p < chars.len() && !chars[p].is_alphanumeric() { p += 1; }
            }
            p as isize
        } else {
            pos + delta
        };
        let next = next.clamp(0, chars.len() as isize - 1) as usize;
        self.set_cursor(chars[next], extend);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cursor_home(&mut self) {
        self.set_cursor(0, false);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cursor_end(&mut self) {
        self.set_cursor(self.query.len(), false);
    }

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
        for c in Command::ALL.iter().copied() {
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
        let mut deferred_search: Option<Item> = None;
        if let Some(url) = crate::bangs::resolve(q, &self.custom_bangs) {
            items.push(Item::Bang { label: format!("Bang: {}", short_url(&url)), url });
        } else {
            if let Some(result) = crate::calc::eval(q) {
                items.push(Item::Calc(result));
            }
            if !q.is_empty() {
                let resolved = resolve(q, search_template);
                // If the user typed a short prefix/alias (e.g. "you" -> youtube.com,
                // "dsh" -> DSH DeepSeek Harness) rather than an explicit URL or
                // multi-word search, promote the top matching tab/bookmark/history
                // entry to index 0 so Enter opens the predicted link directly.
                let promote_prediction = matches!(resolved, Item::Search { .. })
                    && scored.first().is_some_and(|(_, top)| is_strong_prediction(q, top));
                if promote_prediction {
                    deferred_search = Some(resolved);
                } else {
                    items.push(resolved);
                }
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
                if let Some(search_item) = deferred_search.take() {
                    items.push(search_item);
                }
            }
            if items.len() >= MAX_ITEMS {
                break;
            }
        }
        if let Some(search_item) = deferred_search.take() {
            items.push(search_item);
        }
        if q.is_empty() {
            items.extend(Command::ALL.iter().take(MAX_ITEMS.saturating_sub(items.len())).map(|c| Item::Command(*c)));
        }
        self.completion = if self.cursor == self.query.len() && self.selection().is_none() && self.marked.is_none() {
            inline_completion(q, sources)
        } else {
            None
        };
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

/// True when `q` is a single-token prefix (≥2 chars) of a tab/bookmark/history
/// item's host, title, or word-initial acronym (e.g. "you" -> youtube.com,
/// "dsh" -> "DSH DeepSeek Harness" or "DeepSeek Harness").
fn is_strong_prediction(q: &str, item: &Item) -> bool {
    let (title, url) = match item {
        Item::Tab { title, url, .. } | Item::Bookmark { title, url } | Item::History { title, url } => (title.as_str(), url.as_str()),
        _ => return false,
    };
    if q.chars().count() < 2 || q.contains(char::is_whitespace) {
        return false;
    }
    let ql = q.to_lowercase();
    let host = short_str(url).split(['/', '?', '#']).next().unwrap_or("").to_lowercase();
    if host.starts_with(&ql) {
        return true;
    }
    let tl = title.trim().to_lowercase();
    if tl.starts_with(&ql) {
        return true;
    }
    // Word-start acronym match (e.g. "dsh" matching "DeepSeek Harness" or "DSH ...").
    let initials: String = tl
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .collect();
    !initials.is_empty() && initials.starts_with(&ql)
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
    fn utf16_offsets_handle_cjk_and_surrogate_pairs() {
        let text = "aé中😀z";
        assert_eq!(byte_to_utf16(text, 0), 0);
        assert_eq!(byte_to_utf16(text, 2), 1, "inside é floors to its start");
        assert_eq!(byte_to_utf16(text, 3), 2);
        assert_eq!(byte_to_utf16(text, 6), 3);
        assert_eq!(byte_to_utf16(text, 10), 5, "the emoji is two UTF-16 units");
        assert_eq!(byte_to_utf16(text, 11), 6);
        assert_eq!(utf16_to_byte(text, 2), 3);
        assert_eq!(utf16_to_byte(text, 3), 6);
        assert_eq!(utf16_to_byte(text, 4), 6, "inside a surrogate pair floors to the emoji's start");
        assert_eq!(utf16_to_byte(text, 5), 10);
        assert_eq!(utf16_to_byte(text, 6), 11);
    }

    #[test]
    fn marked_replacement_tracks_utf16_ranges() {
        let mut p = Palette::default();
        p.set_query("ab".into());
        p.select_all();
        assert_eq!(p.replacement_range(None), 0..2, "selection is replaced when there is no marked text");
        let inserted = p.replace_and_mark(0..2, "日本", Some(1..2));
        assert_eq!(inserted, 0..6);
        assert_eq!(p.query, "日本");
        assert_eq!(p.marked, Some(0..6));
        assert_eq!(p.cursor, 6);
        assert_eq!(p.selection(), Some((3, 6)), "the relative UTF-16 selection becomes byte offsets");
        assert_eq!(p.replacement_range(Some(0..1)), 0..3, "explicit UTF-16 ranges win over the marked range");
        p.unmark();
        assert_eq!(p.marked, None);
        p.set_cursor(0, false);
        assert_eq!(p.replacement_range(None), 0..0, "after unmark the caret is the replacement");
    }

    #[test]
    fn cursor_editing_respects_char_boundaries() {
        let mut p = Palette::default();
        p.set_query("abc".into());
        p.move_cursor(-1);
        p.insert("é");
        assert_eq!(p.query, "abéc");
        assert_eq!(p.cursor, 4);
        p.move_cursor(-1);
        assert!(p.delete());
        assert_eq!(p.query, "abc");
        p.cursor_home();
        assert!(p.backspace() == false);
        p.cursor_end();
        assert!(p.backspace());
        assert_eq!((p.query.as_str(), p.cursor), ("ab", 2));
        p.clear_query();
        assert_eq!((p.query.as_str(), p.cursor), ("", 0));
    }

    #[test]
    fn selection_replaces_and_copy_text() {
        let mut p = Palette::default();
        p.set_query("hello world".into());
        p.select_all();
        assert_eq!(p.selected_text().as_deref(), Some("hello world"));
        p.insert("x");
        assert_eq!((p.query.as_str(), p.cursor), ("x", 1));
        p.set_query("abc".into());
        p.set_cursor(1, false);
        p.set_cursor(3, true);
        assert_eq!(p.selected_text().as_deref(), Some("bc"));
        assert!(p.backspace());
        assert_eq!((p.query.as_str(), p.cursor), ("a", 1));
        p.set_query("one two".into());
        p.set_cursor(7, false);
        p.move_cursor_ext(-1, false, true);
        assert!(p.cursor < 7);
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
        assert!(matches!(p.items[0], Item::Tab { id: 1, .. }), "strong tab prediction preselects first");
        assert!(matches!(p.items[1], Item::Search { .. }), "search fallback follows right after");
        assert_eq!(p.items.iter().filter(|i| i.detail().contains("docs.rs/gpui")).count(), 1);
        p.move_selection(-1);
        assert_eq!(p.selected, p.items.len() - 1, "selection wraps");
    }

    #[test]
    fn palette_commands_link_to_keymap_actions() {
        assert_eq!(Command::NewTab.action(), Some(crate::keymap::ActionId::NewTab));
        assert_eq!(Command::Settings.action(), Some(crate::keymap::ActionId::ToggleSettings));
        assert_eq!(Command::ImportChrome.action(), None);
        assert_eq!(Command::ALL.iter().filter(|c| c.action().is_some()).count(), 30);
    }

    #[test]
    fn command_table_rows_are_unique_and_complete() {
        let mut titles = std::collections::HashSet::new();
        let mut actions = std::collections::HashSet::new();
        let mut mapped = 0;
        for command in Command::ALL {
            assert!(!command.title().is_empty(), "{command:?}");
            assert!(titles.insert(command.title()), "duplicate palette title {:?}", command.title());
            match command.action() {
                Some(action) => {
                    mapped += 1;
                    assert!(
                        crate::keymap::ActionId::ALL.contains(&action),
                        "{command:?} points at {action:?}, which is not in the action table"
                    );
                    assert!(actions.insert(action), "two commands share {action:?}");
                    assert!(!command.keys().is_empty(), "{command:?} has an action but no displayed key");
                }
                None => assert_eq!(command.keys(), "", "palette-only {command:?} must not advertise a shortcut"),
            }
        }
        assert_eq!(Command::ALL.len(), 39);
        assert_eq!(mapped, 30, "30 commands run a keymap action; 9 are palette-only");
    }

    #[test]
    fn command_shortcuts_are_derived_from_the_action_table() {
        for command in Command::ALL {
            let expected = command
                .action()
                .map(|action| crate::keymap::shortcut_label(action.default_key()))
                .unwrap_or_default();
            assert_eq!(command.keys(), expected, "{command:?}");
        }
    }

    #[test]
    fn commands_without_a_shortcut_are_representable() {
        let palette_only: Vec<Command> =
            Command::ALL.iter().copied().filter(|command| command.action().is_none()).collect();
        assert_eq!(palette_only.len(), 9);
        for command in [Command::ImportChrome, Command::CopyMcpSetup, Command::CloseDuplicateTabs] {
            assert!(palette_only.contains(&command), "{command:?} is palette-only");
            assert_eq!(command.keys(), "", "{command:?} shows no key");
            assert!(!command.title().is_empty());
        }
        // Bindings are built from action-table rows only, so a palette-only
        // command has no path into the keymap.
        assert_eq!(crate::keymap::Keymap::default().bindings().len(), ActionId::ALL.len());
        assert!(Command::ALL.iter().all(|command| command.action().is_none() || !command.keys().is_empty()));
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
        assert_eq!(Command::ALL.len(), 39);
        for c in [Command::CopyCleanUrl, Command::CopyMarkdown, Command::CloseDuplicateTabs, Command::ToggleBookmark, Command::TogglePin, Command::Find, Command::ZoomIn, Command::ZoomOut, Command::ZoomReset, Command::ToggleTabStrip, Command::NextTabPlacement, Command::InstallWebApp, Command::RemoveWebApp, Command::ShowHistory, Command::ShowBookmarks, Command::ToggleAgents, Command::CycleAgentPolicy, Command::CycleTheme, Command::CycleAccent, Command::ToggleReader, Command::TogglePictureInPicture, Command::ShowDownloads] {
            assert!(Command::ALL.contains(&c));
            assert!(!c.title().is_empty());
        }
        assert_eq!(Command::CopyCleanUrl.keys(), "⌘⇧C");
        assert_eq!(Command::CopyMarkdown.keys(), "⌘⇧M");
        assert_eq!(Command::CloseDuplicateTabs.keys(), "");
        assert_eq!(Command::ToggleBookmark.keys(), "⌘D");
        assert_eq!(Command::TogglePin.keys(), "⌘⌥P");
        assert_eq!(Command::ToggleTabStrip.keys(), "⌘S");
        assert_eq!(Command::NextTabPlacement.keys(), "⌘⇧S");
        assert_eq!(Command::InstallWebApp.keys(), "");
        assert_eq!(Command::RemoveWebApp.keys(), "");
        let mut p = Palette { query: "duplicate".into(), ..Default::default() };
        p.refresh(&Sources::default(), DEFAULT_SEARCH);
        assert!(p.items.contains(&Item::Command(Command::CloseDuplicateTabs)));
    }

    #[test]
    fn strong_predictions_preselect_ahead_of_search() {
        let sources = Sources {
            tabs: vec![],
            bookmarks: vec![
                ("DSH DeepSeek Harness".into(), "https://dsh.internal/".into()),
                ("YouTube".into(), "https://www.youtube.com/".into()),
            ],
            history: vec![("GitHub".into(), "https://github.com/".into())],
        };
        let mut p = Palette { query: "you".into(), ..Default::default() };
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(matches!(&p.items[0], Item::Bookmark { url, .. } if url == "https://www.youtube.com/"));
        assert!(matches!(&p.items[1], Item::Search { query, .. } if query == "you"));

        p.set_query("dsh".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(matches!(&p.items[0], Item::Bookmark { title, .. } if title == "DSH DeepSeek Harness"));
        assert!(matches!(&p.items[1], Item::Search { query, .. } if query == "dsh"));

        // Multi-word searches keep Search at index 0 rather than hijacking Enter.
        p.set_query("youtube music".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(matches!(&p.items[0], Item::Search { query, .. } if query == "youtube music"));
    }

    #[test]
    fn inline_completion_appends_a_url_prefix_once() {
        let sources = Sources {
            tabs: vec![],
            bookmarks: vec![("Docs".into(), "https://docs.rs/gpui/latest".into())],
            history: vec![("Example".into(), "https://example.com/path".into())],
        };
        let mut p = Palette::default();
        p.set_query("exa".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        assert_eq!(p.completion.as_deref(), Some("mple.com/path"));
        assert!(p.accept_completion());
        assert_eq!(p.query, "example.com/path");
        assert!(p.completion.is_none());

        p.set_query("https://docs.rs/g".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        assert_eq!(p.completion.as_deref(), Some("pui/latest"));

        p.set_query("two words".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(p.completion.is_none(), "a search query is not a URL prefix");

        p.set_query("exa".into());
        p.refresh(&sources, DEFAULT_SEARCH);
        p.move_cursor_ext(-1, false, false);
        p.refresh(&sources, DEFAULT_SEARCH);
        assert!(p.completion.is_none(), "only complete with the caret at the end");
    }
}
