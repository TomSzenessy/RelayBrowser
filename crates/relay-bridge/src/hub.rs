//! Shared Live Tab State: the one set of Rust structs that both the native
//! Inspector and the MCP server read. The WebKit layer writes into it from
//! bridge messages; nothing else does.

use crate::protocol::{
    Challenge, Command, CommandResult, ConsoleEntry, DomNode, NetworkEntry, NodeId, Op, PageMessage,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

pub type TabId = u64;

pub const MAX_CONSOLE: usize = 1000;
pub const MAX_NETWORK: usize = 500;

/// Who issues a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User,
    Agent,
}

/// Who opened a tab. Agent tabs live in the tinted Agent Tab Group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<f64>,
}

/// Flat DOM store: subtrees are replaced by id when the bridge sends patches.
#[derive(Debug, Default, Clone)]
pub struct DomStore {
    root: Option<NodeId>,
    nodes: HashMap<NodeId, StoredNode>,
}

#[derive(Debug, Clone)]
struct StoredNode {
    tag: String,
    attrs: BTreeMap<String, String>,
    text: String,
    interactive: bool,
    children: Vec<NodeId>,
}

fn unique_ids(node: &DomNode) -> bool {
    let mut seen = std::collections::HashSet::new();
    collect_ids(node, &mut seen)
}

fn collect_ids(node: &DomNode, seen: &mut std::collections::HashSet<NodeId>) -> bool {
    if !seen.insert(node.id) {
        return false;
    }
    node.children.iter().all(|c| collect_ids(c, seen))
}

/// Child ids that leave the patched subtree and therefore keep pointing into
/// the existing store after the patch.
fn collect_boundary(node: &DomNode, incoming: &std::collections::HashSet<NodeId>, out: &mut Vec<NodeId>) {
    for child in &node.children {
        if incoming.contains(&child.id) {
            collect_boundary(child, incoming, out);
        } else {
            out.push(child.id);
        }
    }
}

impl DomStore {
    /// Replace the whole tree. Returns false and keeps the previous tree when
    /// the page sends duplicate ids (which would make the graph cyclic).
    pub fn replace_all(&mut self, root: Option<DomNode>) -> bool {
        let Some(root) = root else {
            self.nodes.clear();
            self.root = None;
            return true;
        };
        if !unique_ids(&root) {
            return false;
        }
        self.nodes.clear();
        self.root = Some(self.insert(root));
        true
    }

    /// Replace each patched subtree in place; drop removed ids. Returns false
    /// and changes nothing when a message repeats an id or would close a
    /// cycle through the existing tree.
    pub fn patch(&mut self, nodes: Vec<DomNode>, removed: &[NodeId]) -> bool {
        let mut incoming = std::collections::HashSet::new();
        for node in &nodes {
            if !collect_ids(node, &mut incoming) {
                return false;
            }
        }
        if self.would_cycle(&nodes, &incoming) {
            return false;
        }
        for id in removed {
            self.remove_subtree(*id);
        }
        for node in nodes {
            if let Some(old) = self.nodes.get(&node.id) {
                for child in old.children.clone() {
                    self.remove_subtree(child);
                }
            }
            self.insert(node);
        }
        // Children lists may reference ids removed above.
        let live: std::collections::HashSet<NodeId> = self.nodes.keys().copied().collect();
        for n in self.nodes.values_mut() {
            n.children.retain(|c| live.contains(c));
        }
        true
    }

    /// A cycle can only be created by an edge from a patched subtree into an
    /// existing node that already reaches one of the patched ids. Walk those
    /// edges with a budget; ids inside the patch are ignored because the patch
    /// replaces them.
    fn would_cycle(&self, nodes: &[DomNode], incoming: &std::collections::HashSet<NodeId>) -> bool {
        let budget = self.nodes.len() + incoming.len() + 1;
        let mut boundary = Vec::new();
        for node in nodes {
            collect_boundary(node, incoming, &mut boundary);
        }
        boundary.into_iter().any(|child| self.reaches_incoming(child, incoming, budget))
    }

    fn reaches_incoming(&self, start: NodeId, incoming: &std::collections::HashSet<NodeId>, budget: usize) -> bool {
        let mut stack = vec![start];
        let mut seen = std::collections::HashSet::new();
        let mut left = budget;
        while let Some(id) = stack.pop() {
            if left == 0 {
                return true; // malformed/oversized graph: reject rather than risk a cycle
            }
            left -= 1;
            if !seen.insert(id) {
                return true;
            }
            if incoming.contains(&id) {
                return true;
            }
            if let Some(n) = self.nodes.get(&id) {
                stack.extend(n.children.iter().copied());
            }
        }
        false
    }

    fn insert(&mut self, node: DomNode) -> NodeId {
        let id = node.id;
        let children = node.children.into_iter().map(|c| self.insert(c)).collect();
        self.nodes.insert(
            id,
            StoredNode { tag: node.tag, attrs: node.attrs, text: node.text, interactive: node.interactive, children },
        );
        id
    }

    fn remove_subtree(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.remove(&id) {
            for c in n.children {
                self.remove_subtree(c);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Replace one node's attributes without touching its subtree. Returns
    /// false for an unknown id (the page may have raced a snapshot).
    pub fn set_attrs(&mut self, id: NodeId, attrs: BTreeMap<String, String>) -> bool {
        match self.nodes.get_mut(&id) {
            Some(n) => {
                n.attrs = attrs;
                true
            }
            None => false,
        }
    }

    /// Rebuild a tree (for the Inspector), bounded by depth. A node already
    /// seen in this walk is emitted without children, so a malformed store
    /// cannot recurse forever.
    pub fn tree(&self, id: NodeId, max_depth: usize) -> Option<DomNode> {
        self.tree_inner(id, max_depth, &mut std::collections::HashSet::new())
    }

    fn tree_inner(&self, id: NodeId, max_depth: usize, seen: &mut std::collections::HashSet<NodeId>) -> Option<DomNode> {
        let n = self.nodes.get(&id)?;
        let expand = max_depth > 0 && seen.insert(id);
        Some(DomNode {
            id,
            tag: n.tag.clone(),
            attrs: n.attrs.clone(),
            text: n.text.clone(),
            interactive: n.interactive,
            children: if expand { n.children.iter().filter_map(|c| self.tree_inner(*c, max_depth - 1, seen)).collect() } else { vec![] },
        })
    }

    /// Interactive element ids in document order (for link hints). Each node
    /// is visited at most once and the walk is capped even on a cycle.
    pub fn interactive_ids(&self, limit: usize) -> Vec<NodeId> {
        let mut out = vec![];
        let mut seen = std::collections::HashSet::new();
        let mut steps = self.nodes.len().saturating_mul(2).max(1);
        let mut stack: Vec<NodeId> = self.root.into_iter().collect();
        while let Some(id) = stack.pop() {
            if out.len() >= limit || steps == 0 {
                break;
            }
            steps -= 1;
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = self.nodes.get(&id) else { continue };
            if n.interactive {
                out.push(id);
            }
            stack.extend(n.children.iter().rev());
        }
        out
    }

    /// LLM-friendly outline: one line per meaningful element, e.g.
    /// `  [12] button "Checkout" #pay`. Interactive ids are what agents click.
    pub fn outline(&self, from: Option<NodeId>, max_depth: usize, interactive_only: bool, max_lines: usize) -> String {
        let mut out = String::new();
        let mut lines = 0;
        let mut seen = std::collections::HashSet::new();
        let mut steps = max_lines.saturating_mul(16).max(64).min(self.nodes.len().saturating_add(1));
        if let Some(start) = from.or(self.root) {
            self.outline_rec(start, 0, max_depth, interactive_only, max_lines, &mut lines, &mut seen, &mut steps, &mut out);
        }
        if lines >= max_lines {
            out.push_str(&format!("… truncated at {max_lines} lines; pass root_id to read a subtree\n"));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn outline_rec(
        &self,
        id: NodeId,
        depth: usize,
        max_depth: usize,
        interactive_only: bool,
        max_lines: usize,
        lines: &mut usize,
        seen: &mut std::collections::HashSet<NodeId>,
        steps: &mut usize,
        out: &mut String,
    ) {
        if *lines >= max_lines || *steps == 0 || depth > max_depth {
            return;
        }
        *steps -= 1;
        if !seen.insert(id) {
            return;
        }
        let Some(n) = self.nodes.get(&id) else { return };
        let shows = if interactive_only { n.interactive } else { n.interactive || !n.text.is_empty() || n.children.is_empty() || matches!(n.tag.as_str(), "form" | "main" | "nav" | "header" | "footer" | "section" | "dialog" | "h1" | "h2" | "h3" | "img" | "iframe" | "table") };
        if shows {
            let indent = if interactive_only { 0 } else { depth };
            out.push_str(&"  ".repeat(indent));
            out.push_str(&format!("[{id}] {}", n.tag));
            if let Some(t) = n.attrs.get("type") {
                out.push_str(&format!("[type={t}]"));
            }
            let label = if !n.text.is_empty() {
                Some(n.text.as_str())
            } else {
                ["aria-label", "placeholder", "title", "alt", "value", "name"].iter().find_map(|k| n.attrs.get(*k).map(|s| s.as_str()))
            };
            if let Some(l) = label {
                out.push_str(&format!(" {l:?}"));
            }
            if let Some(h) = n.attrs.get("href") {
                out.push_str(&format!(" -> {h}"));
            }
            if let Some(i) = n.attrs.get("id") {
                out.push_str(&format!(" #{i}"));
            }
            out.push('\n');
            *lines += 1;
        }
        if depth >= max_depth {
            return;
        }
        for c in &n.children {
            self.outline_rec(*c, depth + 1, max_depth, interactive_only, max_lines, lines, seen, steps, out);
        }
    }
}

/// Everything known about one tab.
#[derive(Debug, Clone)]
pub struct TabState {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub owner: Owner,
    pub dom: DomStore,
    pub console: VecDeque<ConsoleEntry>,
    pub network: VecDeque<NetworkEntry>,
    pub cookies: Vec<Cookie>,
    pub challenge: Option<Challenge>,
    /// Increments on every page load the bridge announces (`hello`).
    pub load_seq: u64,
    pub last_active_at: Instant,
    pub pinned: bool,
    pub playing_audio: bool,
    pub sleeping: bool,
    /// Why the last navigation failed (WebKit's message), cleared on reload.
    pub load_error: Option<String>,
    /// Hotkeys from the page, drained by the shell (`Hub::take_hotkeys`).
    pub hotkeys: Vec<String>,
    /// Last Markdown rendering of the page (`Op::Markdown`); cleared on load.
    pub markdown: Option<String>,
}

impl TabState {
    fn new(id: TabId, url: &str, owner: Owner) -> Self {
        Self {
            id,
            url: url.to_string(),
            title: String::new(),
            owner,
            dom: DomStore::default(),
            console: VecDeque::new(),
            network: VecDeque::new(),
            cookies: vec![],
            challenge: None,
            load_seq: 0,
            last_active_at: Instant::now(),
            pinned: false,
            playing_audio: false,
            sleeping: false,
            load_error: None,
            hotkeys: vec![],
            markdown: None,
        }
    }

    pub fn summary(&self) -> TabSummary {
        TabSummary {
            id: self.id,
            url: self.url.clone(),
            title: self.title.clone(),
            owner: self.owner,
            sleeping: self.sleeping,
            waiting_for_human: self.challenge,
            load_error: self.load_error.clone(),
            console_errors: self.console.iter().filter(|c| c.level == crate::protocol::ConsoleLevel::Error).count(),
            failed_requests: self.network.iter().filter(|n| n.failed()).count(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TabSummary {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub owner: Owner,
    pub sleeping: bool,
    pub load_error: Option<String>,
    pub waiting_for_human: Option<Challenge>,
    pub console_errors: usize,
    pub failed_requests: usize,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum HubError {
    #[error("no tab {0}")]
    NoTab(TabId),
    #[error("no active tab")]
    NoActiveTab,
    #[error("tab {0} is waiting for a human to finish a {1:?} prompt; ask the user, then retry")]
    WaitingForHuman(TabId, Challenge),
    #[error("tab {0} is asleep; activate it first")]
    Sleeping(TabId),
    #[error("the page did not answer within {0:?}")]
    Timeout(Duration),
    #[error("page error: {0}")]
    Page(String),
    #[error("browser error: {0}")]
    Browser(String),
}

#[derive(Default)]
pub struct Hub {
    tabs: BTreeMap<TabId, TabState>,
    order: Vec<TabId>,
    active: Option<TabId>,
    next_tab: TabId,
    next_cmd: u64,
    pending: HashMap<u64, (TabId, oneshot::Sender<CommandResult>)>,
}

/// Thread-safe handle shared by the UI, the WebKit layer, and the MCP server.
/// `changed` ticks on every mutation so UIs can re-render without polling.
#[derive(Clone)]
pub struct SharedHub {
    inner: Arc<Mutex<Hub>>,
    changed: Arc<watch::Sender<u64>>,
}

impl Default for SharedHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedHub {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(0);
        Self { inner: Arc::new(Mutex::new(Hub { next_tab: 1, next_cmd: 1, ..Default::default() })), changed: Arc::new(tx) }
    }

    pub fn lock(&self) -> MutexGuard<'_, Hub> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Mutate and notify subscribers.
    pub fn update<R>(&self, f: impl FnOnce(&mut Hub) -> R) -> R {
        let r = f(&mut self.lock());
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
        r
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Drain page hotkeys without ticking subscribers. Reading a queue is not
    /// a state change; ticking here would wake the shell's own pump forever.
    pub fn take_hotkeys(&self) -> Vec<(TabId, String)> {
        self.lock().take_hotkeys()
    }

    /// Ingest one raw JSON message from a tab's bridge.
    pub fn ingest_json(&self, tab: TabId, json: &str) -> Result<(), serde_json::Error> {
        let msg = PageMessage::parse(json)?;
        self.update(|h| h.ingest(tab, msg));
        Ok(())
    }

    /// Register a command and return the script to evaluate plus the reply.
    pub fn begin_command(&self, tab: TabId, op: Op) -> Result<(String, oneshot::Receiver<CommandResult>), HubError> {
        self.begin(tab, op, Actor::User)
    }

    /// Register a command and return the script to evaluate plus the reply.
    /// Agent commands pause on any tab showing a CAPTCHA/2FA prompt (agents
    /// act on the user's tabs too) and count as activity for tab sleeping.
    pub fn begin(&self, tab: TabId, op: Op, actor: Actor) -> Result<(String, oneshot::Receiver<CommandResult>), HubError> {
        let mut h = self.lock();
        // Drop replies nobody waits for any more (timed-out commands).
        h.pending.retain(|_, (_, tx)| !tx.is_closed());
        let t = h.tabs.get_mut(&tab).ok_or(HubError::NoTab(tab))?;
        if t.sleeping {
            return Err(HubError::Sleeping(tab));
        }
        if actor == Actor::Agent {
            if let (Some(c), true) = (t.challenge, op.mutates()) {
                return Err(HubError::WaitingForHuman(tab, c));
            }
            t.last_active_at = Instant::now();
        }
        let id = h.next_cmd;
        h.next_cmd += 1;
        let (tx, rx) = oneshot::channel();
        h.pending.insert(id, (tab, tx));
        Ok((Command { id, op }.to_script(), rx))
    }

    /// Wait until the tab announces a load newer than `after` (or timeout).
    pub async fn wait_for_load(&self, tab: TabId, after: u64, timeout: Duration) -> Result<(), HubError> {
        let mut rx = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let h = self.lock();
                let t = h.tabs.get(&tab).ok_or(HubError::NoTab(tab))?;
                if let Some(e) = &t.load_error {
                    return Err(HubError::Browser(format!("page failed to load: {e}")));
                }
                if t.load_seq > after && t.dom.root().is_some() {
                    return Ok(());
                }
            }
            if tokio::time::timeout_at(deadline, rx.changed()).await.is_err() {
                return Err(HubError::Timeout(timeout));
            }
        }
    }
}

impl Hub {
    pub fn create_tab(&mut self, url: &str, owner: Owner) -> TabId {
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.insert(id, TabState::new(id, url, owner));
        self.order.push(id);
        id
    }

    pub fn close_tab(&mut self, id: TabId) -> Option<TabState> {
        let t = self.tabs.remove(&id)?;
        let pos = self.order.iter().position(|x| *x == id);
        self.order.retain(|x| *x != id);
        self.pending.retain(|_, (tab, _)| *tab != id);
        if self.active == Some(id) {
            self.active = pos.and_then(|p| self.order.get(p.min(self.order.len().saturating_sub(1))).copied());
        }
        Some(t)
    }

    pub fn tab(&self, id: TabId) -> Option<&TabState> {
        self.tabs.get(&id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        self.tabs.get_mut(&id)
    }

    /// Tabs in display order.
    pub fn tabs(&self) -> impl Iterator<Item = &TabState> {
        self.order.iter().filter_map(|id| self.tabs.get(id))
    }

    pub fn tab_ids(&self) -> &[TabId] {
        &self.order
    }

    /// Pin or unpin a tab. Pinned tabs form a block at the top of the order:
    /// pinning moves a tab to the end of that block, unpinning to just after
    /// it. Returns the new state, or `None` for an unknown tab.
    pub fn set_pinned(&mut self, id: TabId, pinned: bool) -> Option<bool> {
        self.tabs.get_mut(&id)?.pinned = pinned;
        let others_pinned = self.order.iter().filter(|t| **t != id && self.tabs.get(t).is_some_and(|s| s.pinned)).count();
        self.move_tab(id, others_pinned);
        Some(pinned)
    }

    pub fn move_tab(&mut self, id: TabId, to: usize) {
        if let Some(pos) = self.order.iter().position(|x| *x == id) {
            self.order.remove(pos);
            self.order.insert(to.min(self.order.len()), id);
        }
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn set_active(&mut self, id: TabId) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.last_active_at = Instant::now();
            self.active = Some(id);
        }
    }

    /// Resolve an optional tab id to a concrete one (default: active tab).
    pub fn resolve(&self, id: Option<TabId>) -> Result<TabId, HubError> {
        match id {
            Some(id) if self.tabs.contains_key(&id) => Ok(id),
            Some(id) => Err(HubError::NoTab(id)),
            None => self.active.ok_or(HubError::NoActiveTab),
        }
    }

    /// Hotkeys pressed in any tab since the last call.
    pub fn take_hotkeys(&mut self) -> Vec<(TabId, String)> {
        let mut out = vec![];
        for t in self.tabs.values_mut() {
            out.extend(t.hotkeys.drain(..).map(|k| (t.id, k)));
        }
        out
    }

    pub fn set_cookies(&mut self, tab: TabId, cookies: Vec<Cookie>) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.cookies = cookies;
        }
    }

    /// The last Markdown the page produced for `Op::Markdown`, if any.
    pub fn page_markdown(&self, tab: TabId) -> Option<String> {
        self.tabs.get(&tab).and_then(|t| t.markdown.clone())
    }

    pub fn ingest(&mut self, tab: TabId, msg: PageMessage) {
        if let PageMessage::Result(r) = msg {
            if let Some((owner, tx)) = self.pending.remove(&r.id) {
                if owner == tab {
                    let _ = tx.send(r);
                } else {
                    // A page may only answer its own commands.
                    self.pending.insert(r.id, (owner, tx));
                }
            }
            return;
        }
        let Some(t) = self.tabs.get_mut(&tab) else { return };
        match msg {
            PageMessage::Hello { url, .. } => {
                t.url = url;
                t.load_seq += 1;
                t.dom = DomStore::default();
                t.challenge = None;
                t.playing_audio = false;
                t.load_error = None;
                t.markdown = None;
            }
            PageMessage::Page { url, title } => {
                t.url = url;
                t.title = title;
            }
            PageMessage::DomSnapshot { root } => {
                let _ = t.dom.replace_all(root);
            }
            PageMessage::DomPatch { nodes, removed } => {
                let _ = t.dom.patch(nodes, &removed);
            }
            PageMessage::DomAttrs { nodes } => {
                for update in nodes {
                    let _ = t.dom.set_attrs(update.id, update.attrs);
                }
            }
            PageMessage::Console(c) => {
                if t.console.len() == MAX_CONSOLE {
                    t.console.pop_front();
                }
                t.console.push_back(c);
            }
            PageMessage::Network(n) => {
                if t.network.len() == MAX_NETWORK {
                    t.network.pop_front();
                }
                t.network.push_back(n);
            }
            PageMessage::Challenge { challenge } => t.challenge = challenge,
            PageMessage::Media { playing } => t.playing_audio = playing,
            PageMessage::Hotkey { key } => t.hotkeys.push(key),
            PageMessage::Markdown { text } => t.markdown = Some(text),
            PageMessage::Result(_) => unreachable!(),
        }
    }
}

/// What the browser shell must provide so the Inspector and agents can act.
/// The macOS implementation drives WKWebViews on the main thread; tests use
/// a simulator. Calls are fire-and-forget; state arrives through the hub.
pub trait Browser: Send + Sync + 'static {
    /// Open a tab already registered in the hub.
    fn open(&self, tab: TabId, url: &str);
    fn navigate(&self, tab: TabId, url: &str);
    fn go_back(&self, tab: TabId);
    fn go_forward(&self, tab: TabId);
    fn reload(&self, tab: TabId);
    fn close(&self, tab: TabId);
    fn activate(&self, tab: TabId);
    /// Evaluate bridge command script in the tab's page.
    fn eval(&self, tab: TabId, script: String);
    /// Ask WebKit to refresh `TabState::cookies` from WKHTTPCookieStore.
    fn refresh_cookies(&self, tab: TabId);
    /// Open WebKit's own Web Inspector (the 5% fallback).
    fn open_web_inspector(&self, tab: TabId);
}

/// Run one agent op end to end: register, evaluate, await the reply.
pub async fn run_op(
    hub: &SharedHub,
    browser: &dyn Browser,
    tab: TabId,
    op: Op,
    timeout: Duration,
) -> Result<serde_json::Value, HubError> {
    let (script, rx) = hub.begin(tab, op, Actor::Agent)?;
    browser.eval(tab, script);
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(r)) if r.ok => Ok(r.value),
        Ok(Ok(r)) => Err(HubError::Page(r.error.unwrap_or_else(|| "unknown".into()))),
        Ok(Err(_)) => Err(HubError::Browser("tab closed".into())),
        Err(_) => Err(HubError::Timeout(timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: NodeId, tag: &str, text: &str, children: Vec<DomNode>) -> DomNode {
        DomNode { id, tag: tag.into(), attrs: BTreeMap::new(), text: text.into(), interactive: tag == "button", children }
    }

    #[test]
    fn dom_patch_replaces_subtrees_and_drops_removed() {
        let mut d = DomStore::default();
        d.replace_all(Some(node(1, "body", "", vec![node(2, "div", "", vec![node(3, "button", "Old", vec![])]), node(4, "p", "x", vec![])])));
        assert_eq!(d.len(), 4);
        d.patch(vec![node(2, "div", "", vec![node(5, "button", "New", vec![])])], &[4]);
        assert!(!d.contains(3) && !d.contains(4) && d.contains(5));
        let out = d.outline(None, 10, true, 100);
        assert_eq!(out.trim(), "[5] button \"New\"");
        assert_eq!(d.interactive_ids(10), vec![5]);
    }

    #[test]
    fn hotkeys_drain_does_not_tick_subscribers() {
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab("https://a.test", Owner::User));
        let rx = hub.subscribe();
        hub.update(|h| h.tab_mut(tab).unwrap().hotkeys.push("f".into()));
        let before = *rx.borrow();
        assert_eq!(hub.take_hotkeys(), vec![(tab, "f".to_string())]);
        assert_eq!(*rx.borrow(), before, "draining a queue must not bump the watch");
        assert!(hub.take_hotkeys().is_empty());
        assert_eq!(*rx.borrow(), before);
    }

    #[test]
    fn dom_patch_rejects_duplicate_ids_and_keeps_the_old_tree() {
        let mut d = DomStore::default();
        assert!(d.replace_all(Some(node(1, "body", "", vec![node(2, "div", "", vec![])]))));
        // A nested repeated id would make the in-memory graph cyclic.
        assert!(!d.patch(vec![node(2, "div", "", vec![node(3, "div", "", vec![node(2, "div", "", vec![])])])], &[]));
        assert!(d.contains(2) && !d.contains(3), "the rejected patch changed nothing");
        // A duplicate at the top of one message is rejected too.
        assert!(!d.patch(vec![node(4, "div", "", vec![]), node(4, "div", "", vec![])], &[]));
        assert!(!d.contains(4));
        // A coherent patch still applies.
        assert!(d.patch(vec![node(2, "div", "", vec![node(5, "button", "New", vec![])])], &[]));
        assert!(d.contains(5));
    }

    #[test]
    fn traversals_terminate_on_a_forced_cycle() {
        let mut d = DomStore::default();
        d.replace_all(Some(node(1, "body", "", vec![node(2, "div", "", vec![node(3, "button", "b", vec![])])])));
        d.nodes.get_mut(&2).unwrap().children.push(1); // bypass the validator
        assert_eq!(d.interactive_ids(10), vec![3]);
        let tree = d.tree(1, 10).unwrap();
        assert!(tree.children.len() <= 1);
        let out = d.outline(None, 10, false, 10);
        assert!(out.lines().count() <= 11, "{out}");
    }

    #[test]
    fn outline_bounds_a_deep_hidden_chain() {
        let mut d = DomStore::default();
        let mut child = node(500, "button", "end", vec![]);
        for id in (1..500).rev() {
            child = node(id, "div", "", vec![child]);
        }
        d.replace_all(Some(child));
        let out = d.outline(None, 5, true, 10);
        assert!(out.lines().count() <= 11, "{out}");
    }

    #[test]
    fn attribute_updates_replace_only_the_map() {
        let mut d = DomStore::default();
        d.replace_all(Some(node(1, "body", "", vec![node(2, "div", "kept", vec![])])));
        let mut attrs = BTreeMap::new();
        attrs.insert("class".to_string(), "dark".to_string());
        assert!(d.set_attrs(2, attrs));
        let stored = d.nodes.get(&2).unwrap();
        assert_eq!(stored.attrs.get("class").map(String::as_str), Some("dark"));
        assert_eq!(stored.text, "kept");
        assert!(!d.set_attrs(99, BTreeMap::new()));
    }

    #[test]
    fn outline_truncates() {
        let mut d = DomStore::default();
        let kids = (2..50).map(|i| node(i, "button", "b", vec![])).collect();
        d.replace_all(Some(node(1, "body", "", kids)));
        let out = d.outline(None, 5, true, 10);
        assert_eq!(out.lines().count(), 11);
        assert!(out.contains("truncated"));
    }

    #[test]
    fn ring_buffers_and_load_reset() {
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab("about:blank", Owner::User));
        for i in 0..(MAX_CONSOLE + 5) {
            hub.ingest_json(tab, &format!(r#"{{"kind":"console","level":"log","args":["{i}"]}}"#)).unwrap();
        }
        assert_eq!(hub.lock().tab(tab).unwrap().console.len(), MAX_CONSOLE);
        hub.ingest_json(tab, r#"{"kind":"challenge","challenge":"captcha"}"#).unwrap();
        hub.ingest_json(tab, r#"{"kind":"hello","url":"https://b.test/","version":1}"#).unwrap();
        let h = hub.lock();
        let t = h.tab(tab).unwrap();
        assert_eq!((t.load_seq, t.url.as_str(), t.challenge), (1, "https://b.test/", None));
    }

    #[test]
    fn pinned_tabs_form_a_block_at_the_top() {
        let hub = SharedHub::new();
        let [a, b, c, d] = hub.update(|h| ["a", "b", "c", "d"].map(|n| h.create_tab(&format!("https://{n}.test/"), Owner::User)));
        hub.update(|h| {
            assert_eq!(h.set_pinned(c, true), Some(true));
            assert_eq!(h.set_pinned(d, true), Some(true));
        });
        assert_eq!(hub.lock().tab_ids(), &[c, d, a, b], "pinning appends to the pinned block");
        hub.update(|h| h.set_pinned(c, false));
        assert_eq!(hub.lock().tab_ids(), &[d, c, a, b], "unpinning lands right after the block");
        assert!(!hub.lock().tab(c).unwrap().pinned);
        assert_eq!(hub.update(|h| h.set_pinned(999, true)), None);
    }

    #[test]
    fn markdown_message_is_stored_and_cleared_on_load() {
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab("https://a.test/", Owner::User));
        assert_eq!(hub.lock().page_markdown(tab), None);
        hub.ingest_json(tab, r##"{"kind":"markdown","text":"# Hi"}"##).unwrap();
        assert_eq!(hub.lock().page_markdown(tab).as_deref(), Some("# Hi"));
        hub.ingest_json(tab, r#"{"kind":"hello","url":"https://b.test/","version":1}"#).unwrap();
        assert_eq!(hub.lock().page_markdown(tab), None);
    }

    #[test]
    fn agent_tabs_pause_on_challenge_but_reads_continue() {
        let hub = SharedHub::new();
        // The user's own tab: agents act there too, so the pause must hold.
        let tab = hub.update(|h| h.create_tab("https://bank.test", Owner::User));
        hub.ingest_json(tab, r#"{"kind":"challenge","challenge":"two_factor"}"#).unwrap();
        assert_eq!(hub.begin(tab, Op::Click { target: 1 }, Actor::Agent).unwrap_err(), HubError::WaitingForHuman(tab, Challenge::TwoFactor));
        assert!(hub.begin(tab, Op::ReadText { target: None }, Actor::Agent).is_ok());
        assert!(hub.begin(tab, Op::Markdown, Actor::Agent).is_ok(), "markdown is read-only");
        assert!(hub.begin_command(tab, Op::Click { target: 1 }).is_ok(), "the human may click through");
        // Agent activity keeps a tab awake.
        let before = hub.lock().tab(tab).unwrap().last_active_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        hub.begin(tab, Op::PageInfo, Actor::Agent).unwrap();
        assert!(hub.lock().tab(tab).unwrap().last_active_at > before);
    }

    #[test]
    fn results_only_resolve_for_their_own_tab() {
        let hub = SharedHub::new();
        let (a, b) = hub.update(|h| (h.create_tab("a", Owner::User), h.create_tab("b", Owner::User)));
        let (_, mut rx) = hub.begin_command(a, Op::PageInfo).unwrap();
        hub.ingest_json(b, r#"{"kind":"result","id":1,"ok":true,"value":"forged"}"#).unwrap();
        assert!(rx.try_recv().is_err());
        hub.ingest_json(a, r#"{"kind":"result","id":1,"ok":true,"value":"real"}"#).unwrap();
        assert_eq!(rx.try_recv().unwrap().value, "real");
    }

    #[tokio::test]
    async fn failed_load_is_reported_instead_of_timing_out() {
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab("https://offline.test/", Owner::User));
        hub.update(|h| h.tab_mut(tab).unwrap().load_error = Some("The Internet connection appears to be offline.".into()));
        let err = hub.wait_for_load(tab, 0, Duration::from_millis(50)).await.unwrap_err();
        assert!(matches!(&err, HubError::Browser(m) if m.contains("offline")), "{err}");
        hub.ingest_json(tab, r#"{"kind":"hello","url":"https://offline.test/","version":1}"#).unwrap();
        assert!(hub.lock().tab(tab).unwrap().load_error.is_none(), "a successful load clears the error");
    }

    #[test]
    fn closing_active_tab_activates_neighbour() {
        let hub = SharedHub::new();
        hub.update(|h| {
            let a = h.create_tab("a", Owner::User);
            let b = h.create_tab("b", Owner::User);
            let c = h.create_tab("c", Owner::User);
            h.set_active(b);
            h.close_tab(b);
            assert_eq!(h.active(), Some(c));
            h.close_tab(c);
            assert_eq!(h.active(), Some(a));
            h.close_tab(a);
            assert_eq!(h.active(), None);
        });
    }
}
