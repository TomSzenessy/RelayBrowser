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

/// Which element nodes are expanded. Nodes start expanded down to `auto_depth`.
#[derive(Debug, Clone)]
pub struct Tree {
    pub toggled: HashSet<NodeId>,
    pub auto_depth: usize,
}

impl Default for Tree {
    fn default() -> Self {
        Self { toggled: HashSet::new(), auto_depth: 3 }
    }
}

impl Tree {
    pub fn toggle(&mut self, id: NodeId) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
    }

    fn expanded(&self, id: NodeId, depth: usize) -> bool {
        (depth < self.auto_depth) != self.toggled.contains(&id)
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
pub struct NetworkRow {
    pub method: String,
    pub status: String,
    pub url: String,
    pub time: String,
    pub failed: bool,
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
            method: n.method.clone(),
            status: match (&n.error, n.status) {
                (Some(_), _) => "ERR".into(),
                (None, Some(s)) => s.to_string(),
                (None, None) => "—".into(),
            },
            url: crate::palette::short_url(&n.url),
            time: n.duration_ms.map(|d| if d >= 1000.0 { format!("{:.2} s", d / 1000.0) } else { format!("{d:.0} ms") }).unwrap_or_default(),
            failed: n.failed(),
        })
        .collect()
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
            {"id":2,"tag":"div","attrs":{"class":"cart big"},"children":[{"id":3,"tag":"button","attrs":{"id":"pay"},"text":"Pay","children":[{"id":4,"tag":"span","children":[{"id":5,"tag":"b"}]}]}]}]}}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"network","id":"1","api":"fetch","method":"POST","url":"https://a.test/api/pay","status":502,"duration_ms":1234.5}"#).unwrap();
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
    fn network_and_console_rows() {
        let (hub, tab) = tab_with_dom();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        let n = network_rows(t, "");
        assert_eq!(n[0], NetworkRow { method: "POST".into(), status: "502".into(), url: "a.test/api/pay".into(), time: "1.23 s".into(), failed: true });
        assert!(network_rows(t, "nothing").is_empty());
        let c = console_rows(t, ConsoleLevel::Log);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].source, "app.js:1:2");
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
