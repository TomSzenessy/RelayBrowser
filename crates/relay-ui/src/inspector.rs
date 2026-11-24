//! Native Inspector view-models: flat rows built from the same `TabState`
//! the MCP server reads. The GPUI view only paints these rows.

use relay_bridge::{ConsoleLevel, DomNode, NodeId, TabState};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    Elements,
    Network,
    Console,
    Cookies,
}

impl Panel {
    pub const ALL: [Panel; 4] = [Panel::Elements, Panel::Network, Panel::Console, Panel::Cookies];
    pub fn title(self) -> &'static str {
        match self {
            Panel::Elements => "Elements",
            Panel::Network => "Network",
            Panel::Console => "Console",
            Panel::Cookies => "Cookies",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementRow {
    pub id: NodeId,
    pub depth: usize,
    pub label: String,
    pub has_children: bool,
    pub expanded: bool,
}

/// Which element nodes are expanded. Nodes start expanded down to
/// [`AUTO_DEPTH`]; the user toggles from there.
const AUTO_DEPTH: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub toggled: HashSet<NodeId>,
}

impl Tree {
    pub fn toggle(&mut self, id: NodeId) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
    }

    fn expanded(&self, id: NodeId, depth: usize) -> bool {
        (depth < AUTO_DEPTH) != self.toggled.contains(&id)
    }

    pub fn rows(&self, tab: &TabState, limit: usize) -> Vec<ElementRow> {
        let mut out = vec![];
        if let Some(root) = tab.dom.root().and_then(|r| tab.dom.tree(r, 64)) {
            self.walk(&root, 0, limit, &mut out);
        }
        out
    }

    fn walk(&self, n: &DomNode, depth: usize, limit: usize, out: &mut Vec<ElementRow>) {
        if out.len() >= limit {
            return;
        }
        let expanded = self.expanded(n.id, depth);
        out.push(ElementRow { id: n.id, depth, label: element_label(n), has_children: !n.children.is_empty(), expanded });
        if expanded {
            for c in &n.children {
                self.walk(c, depth + 1, limit, out);
            }
        }
    }
}

/// `<button#pay.primary> Pay now`
pub fn element_label(n: &DomNode) -> String {
    let mut s = format!("<{}", n.tag);
    if let Some(id) = n.attrs.get("id") {
        s.push('#');
        s.push_str(id);
    }
    if let Some(class) = n.attrs.get("class") {
        for c in class.split_whitespace().take(3) {
            s.push('.');
            s.push_str(c);
        }
    }
    s.push('>');
    if !n.text.is_empty() {
        s.push(' ');
        s.push_str(&n.text);
    }
    s
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementDetail {
    pub id: NodeId,
    pub tag: String,
    pub selector: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub child_count: usize,
}

/// Inspect one node by id: tag, CSS selector preview, sorted attributes, text,
/// and direct child count. Returns `None` if the node is no longer in the DOM.
pub fn element_detail(tab: &TabState, id: NodeId) -> Option<ElementDetail> {
    let n = tab.dom.tree(id, 1)?;
    let mut selector = n.tag.clone();
    if let Some(dom_id) = n.attrs.get("id").filter(|s| !s.is_empty()) {
        selector.push('#');
        selector.push_str(dom_id);
    }
    if let Some(class) = n.attrs.get("class") {
        for c in class.split_whitespace().take(3) {
            selector.push('.');
            selector.push_str(c);
        }
    }
    let mut attrs: Vec<(String, String)> = n.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    Some(ElementDetail {
        id,
        tag: n.tag.clone(),
        selector,
        attrs,
        text: n.text.clone(),
        child_count: n.children.len(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkRow {
    pub id: String,
    pub api: String,
    pub method: String,
    pub status: String,
    pub url: String,
    pub full_url: String,
    pub time: String,
    pub failed: bool,
    pub content_type: Option<String>,
    pub transfer_size: Option<u64>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub error: Option<String>,
}

pub fn network_rows(tab: &TabState, filter: &str) -> Vec<NetworkRow> {
    network_rows_filtered(tab, filter, false)
}

pub fn network_rows_filtered(tab: &TabState, filter: &str, failed_only: bool) -> Vec<NetworkRow> {
    tab.network
        .iter()
        .rev()
        .filter(|n| (!failed_only || n.failed()) && (filter.is_empty() || n.url.contains(filter)))
        .map(|n| NetworkRow {
            id: n.id.clone(),
            api: n.api.clone(),
            method: n.method.clone(),
            status: match (&n.error, n.status) {
                (Some(_), _) => "ERR".into(),
                (None, Some(s)) => s.to_string(),
                (None, None) => "—".into(),
            },
            url: crate::palette::short_url(&n.url),
            full_url: n.url.clone(),
            time: n.duration_ms.map(|d| if d >= 1000.0 { format!("{:.2} s", d / 1000.0) } else { format!("{d:.0} ms") }).unwrap_or_default(),
            failed: n.failed(),
            content_type: n.content_type.clone(),
            transfer_size: n.transfer_size,
            request_body: n.request_body.clone(),
            response_body: n.response_body.clone(),
            error: n.error.clone(),
        })
        .collect()
}

fn posix_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else if ch == '\0' {
            continue;
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn strip_header_controls(s: &str) -> String {
    s.chars().filter(|&c| c >= ' ' && c != '\x7f').collect()
}

/// Format a recorded request as a POSIX-shell-safe `curl` command.
pub fn network_to_curl(row: &NetworkRow) -> String {
    let method: String = row.method.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let method = if method.is_empty() { "GET".to_string() } else { method.to_ascii_uppercase() };
    let url = strip_header_controls(&row.full_url);
    let mut cmd = format!("curl -X {} {}", posix_quote(&method), posix_quote(&url));
    if let Some(ct) = row.content_type.as_deref().map(strip_header_controls).filter(|s| !s.is_empty()) {
        if row.request_body.as_deref().is_some_and(|b| !b.is_empty()) {
            cmd.push_str(&format!(" -H {}", posix_quote(&format!("Content-Type: {ct}"))));
        }
    }
    if let Some(body) = row.request_body.as_deref().filter(|b| !b.is_empty()) {
        cmd.push_str(&format!(" --data-raw {}", posix_quote(body)));
    }
    cmd
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleRow {
    pub level: ConsoleLevel,
    pub text: String,
    pub source: String,
}

pub fn console_rows(tab: &TabState, min: ConsoleLevel) -> Vec<ConsoleRow> {
    tab.console
        .iter()
        .filter(|c| c.level >= min)
        .map(|c| ConsoleRow { level: c.level, text: c.message(), source: c.source.clone().unwrap_or_default() })
        .collect()
}

/// Vimium-style labels: the fewest characters from a home-row alphabet, and
/// no label is a prefix of another, so typing resolves without a timeout.
pub fn hint_labels(n: usize) -> Vec<String> {
    const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
    let k = ALPHABET.len();
    if n == 0 {
        return vec![];
    }
    // Shortest length L such that k^L >= n; all labels have that length.
    let mut len = 1;
    while k.pow(len as u32) < n {
        len += 1;
    }
    (0..n)
        .map(|mut i| {
            let mut s = vec![b'a'; len];
            for slot in s.iter_mut().rev() {
                *slot = ALPHABET[i % k];
                i /= k;
            }
            String::from_utf8(s).expect("ascii")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_bridge::{Owner, SharedHub};

    fn tab_with_dom() -> (SharedHub, u64) {
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab("https://a.test", Owner::User));
        hub.ingest_json(tab, r#"{"kind":"dom_snapshot","root":{"id":1,"tag":"body","children":[
            {"id":2,"tag":"div","attrs":{"class":"cart big"},"children":[{"id":3,"tag":"button","attrs":{"id":"pay","data-test":"buy"},"text":"Pay","children":[{"id":4,"tag":"span","children":[{"id":5,"tag":"b"}]}]}]}]}}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"network","id":"1","api":"fetch","method":"POST","url":"https://a.test/api/pay","status":502,"duration_ms":1234.5,"content_type":"application/json","request_body":"{\"total\":10}","response_body":"{\"err\":\"gateway\"}"}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"console","level":"error","args":["boom"],"source":"app.js:1:2"}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"console","level":"debug","args":["noise"]}"#).unwrap();
        (hub, tab)
    }

    #[test]
    fn element_rows_auto_expand_and_toggle() {
        let (hub, tab) = tab_with_dom();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        let mut tree = Tree::default();
        let rows = tree.rows(t, 100);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3, 4], "depth 3 auto-expanded; <b> hidden");
        assert_eq!(rows[2].label, "<button#pay> Pay");
        assert_eq!(rows[1].label, "<div.cart.big>");
        tree.toggle(2);
        assert_eq!(tree.rows(t, 100).len(), 2, "collapsing hides the subtree");
        tree.toggle(4);
        tree.toggle(2);
        assert_eq!(tree.rows(t, 100).len(), 5, "expanding a deep node shows its child");
    }

    #[test]
    fn element_detail_reads_sorted_attrs_and_handles_missing_node() {
        let (hub, tab) = tab_with_dom();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        let d = element_detail(t, 3).expect("button#pay exists");
        assert_eq!(d.tag, "button");
        assert_eq!(d.selector, "button#pay");
        assert_eq!(d.text, "Pay");
        assert_eq!(d.child_count, 1);
        assert_eq!(d.attrs, vec![("data-test".into(), "buy".into()), ("id".into(), "pay".into())]);
        assert!(element_detail(t, 9999).is_none());
    }

    #[test]
    fn network_and_console_rows() {
        let (hub, tab) = tab_with_dom();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        let n = network_rows(t, "");
        assert_eq!(n[0].method, "POST");
        assert_eq!(n[0].status, "502");
        assert_eq!(n[0].url, "a.test/api/pay");
        assert_eq!(n[0].full_url, "https://a.test/api/pay");
        assert_eq!(n[0].time, "1.23 s");
        assert!(n[0].failed);
        assert_eq!(n[0].request_body.as_deref(), Some(r#"{"total":10}"#));
        assert_eq!(n[0].response_body.as_deref(), Some(r#"{"err":"gateway"}"#));
        assert!(network_rows(t, "nothing").is_empty());
        let c = console_rows(t, ConsoleLevel::Log);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].source, "app.js:1:2");
    }

    #[test]
    fn network_to_curl_formats_post_and_escapes_shell_metacharacters() {
        let (hub, tab) = tab_with_dom();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        let row = &network_rows(t, "")[0];
        let curl = network_to_curl(row);
        assert_eq!(
            curl,
            "curl -X 'POST' 'https://a.test/api/pay' -H 'Content-Type: application/json' --data-raw '{\"total\":10}'"
        );

        // Negative test: single quotes, newlines, and shell metacharacters in URL/method/body
        // cannot break out of POSIX single-quoting.
        let hostile = NetworkRow {
            id: "x".into(),
            api: "fetch".into(),
            method: "POST; rm -rf /".into(),
            status: "200".into(),
            url: "evil.test".into(),
            full_url: "https://evil.test/?q='$(id)'\nrm -rf /".into(),
            time: "1 ms".into(),
            failed: false,
            content_type: Some("text/plain\r\nX-Injected: 1".into()),
            transfer_size: None,
            request_body: Some("a'b$(whoami)".into()),
            response_body: None,
            error: None,
        };
        let safe = network_to_curl(&hostile);
        assert!(safe.starts_with("curl -X 'POSTRMRF' "));
        assert!(!safe.contains('\n') && !safe.contains('\r'), "control chars stripped from URL/header: {safe}");
        assert!(safe.contains("'\\''$(id)'\\''"), "single quotes escaped: {safe}");
        assert!(safe.contains("--data-raw 'a'\\''b$(whoami)'"), "body single quotes escaped: {safe}");
    }

    #[test]
    fn network_rows_filtered_returns_only_failed() {
        let (hub, tab) = tab_with_dom();
        hub.ingest_json(tab, r#"{"kind":"network","id":"2","api":"fetch","method":"GET","url":"https://a.test/api/ok","status":200,"duration_ms":12.0}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"network","id":"3","api":"fetch","method":"GET","url":"https://a.test/api/down","error":"timeout","duration_ms":50.0}"#).unwrap();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        assert_eq!(network_rows_filtered(t, "", false).len(), 3);
        let failed = network_rows_filtered(t, "", true);
        assert_eq!(failed.len(), 2);
        assert!(failed.iter().all(|r| r.failed));
        assert_eq!(failed[0].status, "ERR");
        assert_eq!(failed[1].status, "502");
        let filtered_failed = network_rows_filtered(t, "pay", true);
        assert_eq!(filtered_failed.len(), 1);
        assert_eq!(filtered_failed[0].url, "a.test/api/pay");
    }

    #[test]
    fn hint_labels_are_minimal_and_prefix_free() {
        assert_eq!(hint_labels(3), ["a", "s", "d"]);
        let many = hint_labels(100);
        assert!(many.iter().all(|l| l.len() == 2));
        let set: HashSet<_> = many.iter().collect();
        assert_eq!(set.len(), 100);
        assert!(hint_labels(0).is_empty());
    }
}
