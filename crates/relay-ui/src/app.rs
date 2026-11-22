//! The GPUI shell: vertical tab bar, content panes (WebKit on macOS), the
//! ⌘K palette, the native Inspector, link hints, and the status bar. It reads
//! the same `SharedHub` the MCP server reads; MCP requests reach WebKit via
//! the pump loop below.

use crate::inspector::{self, Panel, Tree};
use crate::palette::{self, Command, Item, Palette, Sources};
use crate::theme::Theme;
use crate::web::{WebEvent, WebHost};
use gpui::{
    actions, canvas, div, img, prelude::*, px, Bounds, ClipboardItem, Context, FocusHandle, Image, ImageFormat,
    KeyBinding, KeyDownEvent, Pixels, SharedString, Window,
};
use relay_bridge::{BrowserRequest, ConsoleLevel, NodeId, Op, Owner, SharedHub, TabId};
use relay_extensions::store::Store;
use relay_webkit::sleep::SleepPolicy;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

actions!(
    relay,
    [
        OpenPalette,
        OpenLocation,
        NewTab,
        CloseTab,
        ReopenClosedTab,
        Reload,
        Back,
        Forward,
        NextTab,
        PrevTab,
        ToggleInspector,
        WebInspector,
        ToggleSplit,
        LinkHints,
        Quit,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
    ]
);

/// Every global shortcut. `secondary` is ⌘ on macOS and Ctrl elsewhere.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("secondary-k", OpenPalette, None),
        KeyBinding::new("secondary-l", OpenLocation, None),
        KeyBinding::new("secondary-t", NewTab, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("secondary-shift-t", ReopenClosedTab, None),
        KeyBinding::new("secondary-r", Reload, None),
        KeyBinding::new("secondary-[", Back, None),
        KeyBinding::new("secondary-]", Forward, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("secondary-alt-i", ToggleInspector, None),
        KeyBinding::new("secondary-alt-u", WebInspector, None),
        KeyBinding::new("secondary-\\", ToggleSplit, None),
        KeyBinding::new("secondary-j", LinkHints, None),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-1", SelectTab1, None),
        KeyBinding::new("secondary-2", SelectTab2, None),
        KeyBinding::new("secondary-3", SelectTab3, None),
        KeyBinding::new("secondary-4", SelectTab4, None),
        KeyBinding::new("secondary-5", SelectTab5, None),
        KeyBinding::new("secondary-6", SelectTab6, None),
        KeyBinding::new("secondary-7", SelectTab7, None),
        KeyBinding::new("secondary-8", SelectTab8, None),
        KeyBinding::new("secondary-9", SelectTab9, None),
    ]
}

struct Hints {
    tab: TabId,
    labels: Vec<(NodeId, String)>,
    typed: String,
}

pub struct RelayApp {
    hub: SharedHub,
    web: Rc<RefCell<WebHost>>,
    requests: UnboundedReceiver<BrowserRequest>,
    store: Option<Store>,
    focus: FocusHandle,
    theme: Theme,
    palette: Option<Palette>,
    /// ⌘T opens the palette for a new tab; ⌘K/⌘L target the current tab.
    palette_new_tab: bool,
    sources: Sources,
    inspector_open: bool,
    panel: Panel,
    tree: Tree,
    console_min: ConsoleLevel,
    split: Option<TabId>,
    closed: Vec<String>,
    hints: Option<Hints>,
    toast: Option<(String, Instant)>,
    search: String,
    seen_version: u64,
    recorded: HashMap<TabId, (u64, String)>,
    sleep_policy: SleepPolicy,
    last_sleep_check: Instant,
    placements: Rc<RefCell<Vec<(TabId, Bounds<Pixels>)>>>,
    snapshots: HashMap<TabId, Arc<Image>>,
    mcp_socket: String,
    /// Last title pushed to the native window; avoids a call every frame.
    window_title: String,
}

impl RelayApp {
    pub fn new(
        hub: SharedHub,
        requests: UnboundedReceiver<BrowserRequest>,
        store: Option<Store>,
        mcp_socket: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let web = WebHost::new(hub.clone(), window);
        let rules = relay_webkit::content_rules::compile(&blocklist());
        web.load_content_rules(&rules.to_json());
        let search = store.as_ref().and_then(|s| s.setting("search").ok().flatten()).unwrap_or_else(|| palette::DEFAULT_SEARCH.to_string());
        let sleep_minutes = store.as_ref().and_then(|s| s.setting("sleep_minutes").ok().flatten()).and_then(|m| m.parse::<u64>().ok());
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut app = Self {
            hub,
            web: Rc::new(RefCell::new(web)),
            requests,
            store,
            focus,
            theme: Theme::for_appearance(window.appearance()),
            palette: None,
            palette_new_tab: false,
            sources: Sources::default(),
            inspector_open: false,
            panel: Panel::Elements,
            tree: Tree::default(),
            console_min: ConsoleLevel::Log,
            split: None,
            closed: vec![],
            hints: None,
            toast: None,
            search,
            seen_version: 0,
            recorded: HashMap::new(),
            sleep_policy: SleepPolicy { idle_after: Duration::from_secs(sleep_minutes.unwrap_or(15) * 60), keep_awake_min: 1 },
            last_sleep_check: Instant::now(),
            placements: Rc::default(),
            snapshots: HashMap::new(),
            mcp_socket,
            window_title: String::new(),
        };
        app.load_installed_extensions();
        app.start_pump(cx);
        app
    }

    fn load_installed_extensions(&self) {
        let Some(store) = &self.store else { return };
        for (id, name, path) in store.enabled_extensions().unwrap_or_default() {
            self.web.borrow().load_extension(std::path::Path::new(&path), &id, move |r| {
                if let Err(e) = r {
                    eprintln!("relay: extension {name} failed to load: {e}");
                }
            });
        }
    }

    // ----------------------------------------------------------- pump

    /// One loop drains MCP requests, WebKit events, and hub changes at
    /// display rate. Cheap when idle: only `try_recv`s and a counter compare.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            if this.update(cx, |app, cx| app.pump(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(req) = self.requests.try_recv() {
            changed = true;
            if let BrowserRequest::Open { tab, .. } = &req {
                // Agent tabs join the window without stealing focus.
                if self.hub.lock().active().is_none() {
                    self.hub.update(|h| h.set_active(*tab));
                }
            }
            self.web.borrow_mut().apply(req);
        }
        let events = self.web.borrow().take_events();
        for e in events {
            changed = true;
            match e {
                WebEvent::OpenInNewTab { opener, url } => {
                    if !is_web_url(&url) {
                        continue; // window.open("file:///…") and friends stay closed
                    }
                    let owner = self.hub.lock().tab(opener).map(|t| t.owner).unwrap_or(Owner::User);
                    let tab = self.hub.update(|h| {
                        let id = h.create_tab(&url, owner);
                        if let Some(pos) = h.tab_ids().iter().position(|t| *t == opener) {
                            h.move_tab(id, pos + 1);
                        }
                        if owner == Owner::User {
                            h.set_active(id);
                        }
                        id
                    });
                    self.web.borrow_mut().apply(BrowserRequest::Open { tab, url });
                }
                WebEvent::Slept(tab) => {
                    if let Some(jpeg) = self.web.borrow().snapshot(tab) {
                        self.snapshots.insert(tab, Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg)));
                    }
                }
            }
        }
        let version = *self.hub.subscribe().borrow();
        if version != self.seen_version {
            self.seen_version = version;
            changed = true;
            self.after_hub_change();
        }
        if self.last_sleep_check.elapsed() > Duration::from_secs(30) {
            self.last_sleep_check = Instant::now();
            self.sleep_idle_tabs(cx);
        }
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
            self.toast = None;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn after_hub_change(&mut self) {
        let hotkeys = self.hub.update(|h| h.take_hotkeys());
        let mut loads = vec![];
        {
            let h = self.hub.lock();
            for t in h.tabs() {
                let rec = self.recorded.get(&t.id);
                if rec.map(|r| r.0) != Some(t.load_seq) {
                    loads.push(t.id);
                }
                if t.load_seq > 0 && rec != Some(&(t.load_seq, t.title.clone())) {
                    if let (Some(store), Owner::User) = (&self.store, t.owner) {
                        let _ = store.record_visit(&t.url, &t.title, now_unix());
                    }
                    self.recorded.insert(t.id, (t.load_seq, t.title.clone()));
                }
            }
        }
        for tab in loads {
            self.web.borrow_mut().after_load(tab);
            self.snapshots.remove(&tab);
        }
        for (tab, key) in hotkeys {
            if key == "f" && Some(tab) == self.active() {
                self.start_hints();
            }
        }
    }

    fn sleep_idle_tabs(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<TabId> = self.visible_tabs();
        let due = {
            let h = self.hub.lock();
            self.sleep_policy.due(&h, &visible, Instant::now())
        };
        for tab in due {
            // Ask the page for its scroll offset, then sleep it.
            let Ok((script, rx)) = self.hub.begin_command(tab, Op::PageInfo) else { continue };
            self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
            cx.spawn(async move |this, cx| {
                let reply = futures_timeout(rx, Duration::from_millis(500), cx).await;
                let scroll = reply
                    .map(|v| (v["scroll_x"].as_f64().unwrap_or(0.0), v["scroll_y"].as_f64().unwrap_or(0.0)))
                    .unwrap_or((0.0, 0.0));
                let _ = this.update(cx, |app, cx| {
                    // The user (or an agent) may have used the tab meanwhile.
                    let still_due = {
                        let h = app.hub.lock();
                        app.sleep_policy.due(&h, &app.visible_tabs(), Instant::now()).contains(&tab)
                    };
                    if still_due {
                        app.web.borrow_mut().sleep(tab, scroll);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    // -------------------------------------------------------- helpers

    fn active(&self) -> Option<TabId> {
        self.hub.lock().active()
    }

    fn visible_tabs(&self) -> Vec<TabId> {
        self.active().into_iter().chain(self.split).collect()
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn open_url(&mut self, url: String, new_tab: bool) {
        let active = self.active();
        match (new_tab, active) {
            (false, Some(tab)) => {
                self.web.borrow_mut().apply(BrowserRequest::Navigate { tab, url });
            }
            _ => {
                let tab = self.hub.update(|h| {
                    let id = h.create_tab(&url, Owner::User);
                    h.set_active(id);
                    id
                });
                self.web.borrow_mut().apply(BrowserRequest::Open { tab, url });
            }
        }
    }

    fn activate(&mut self, tab: TabId) {
        self.hub.update(|h| h.set_active(tab));
        self.web.borrow_mut().wake(tab);
        self.web.borrow().focus_page(tab);
    }

    fn close_active(&mut self) {
        let Some(tab) = self.active() else { return };
        if let Some(t) = self.hub.update(|h| h.close_tab(tab)) {
            if t.url.starts_with("http") {
                self.closed.push(t.url);
            }
        }
        if self.split == Some(tab) {
            self.split = None;
        }
        self.snapshots.remove(&tab);
        self.web.borrow_mut().apply(BrowserRequest::Close(tab));
        if let Some(next) = self.active() {
            self.activate(next);
        }
    }

    fn cycle(&mut self, delta: isize) {
        let (ids, active) = {
            let h = self.hub.lock();
            (h.tab_ids().to_vec(), h.active())
        };
        if ids.is_empty() {
            return;
        }
        let pos = active.and_then(|a| ids.iter().position(|t| *t == a)).unwrap_or(0) as isize;
        let next = ids[((pos + delta).rem_euclid(ids.len() as isize)) as usize];
        self.activate(next);
    }

    fn select_index(&mut self, i: usize) {
        let ids = self.hub.lock().tab_ids().to_vec();
        // ⌘9 is always the last tab, as in Safari and Chrome.
        let target = if i == 8 { ids.last() } else { ids.get(i) };
        if let Some(t) = target.copied() {
            self.activate(t);
        }
    }

    fn eval_op(&self, tab: TabId, op: Op) {
        if let Ok((script, _rx)) = self.hub.begin_command(tab, op) {
            self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
        }
    }

    fn open_palette(&mut self, prefill: Option<String>, new_tab: bool) {
        self.palette_new_tab = new_tab;
        self.sources = Sources {
            tabs: self.hub.lock().tabs().map(|t| (t.id, t.title.clone(), t.url.clone())).collect(),
            history: self
                .store
                .as_ref()
                .and_then(|s| s.search_history("", 500).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|v| (v.title, v.url))
                .collect(),
            bookmarks: self.store.as_ref().and_then(|s| s.bookmarks().ok()).unwrap_or_default().into_iter().map(|b| (b.title, b.url)).collect(),
        };
        let mut p = Palette { query: prefill.unwrap_or_default(), ..Default::default() };
        p.refresh(&self.sources, &self.search);
        self.palette = Some(p);
        self.web.borrow().focus_shell();
    }

    fn close_palette(&mut self) {
        self.palette = None;
        if let Some(t) = self.active() {
            self.web.borrow().focus_page(t);
        }
    }

    fn run_item(&mut self, item: Item, cx: &mut Context<Self>) {
        let new_tab = self.palette_new_tab || self.active().is_none();
        match item {
            Item::Open(url) | Item::Search { url, .. } | Item::History { url, .. } | Item::Bookmark { url, .. } => self.open_url(url, new_tab),
            Item::Tab { id, .. } => self.activate(id),
            Item::Command(c) => self.run_command(c, cx),
        }
    }

    fn run_command(&mut self, c: Command, cx: &mut Context<Self>) {
        let active = self.active();
        match c {
            Command::NewTab => self.open_palette(None, true),
            Command::CloseTab => self.close_active(),
            Command::ReopenClosedTab => {
                if let Some(url) = self.closed.pop() {
                    self.open_url(url, true);
                }
            }
            Command::Reload => active.into_iter().for_each(|t| self.web.borrow_mut().apply(BrowserRequest::Reload(t))),
            Command::Back => active.into_iter().for_each(|t| self.web.borrow_mut().apply(BrowserRequest::GoBack(t))),
            Command::Forward => active.into_iter().for_each(|t| self.web.borrow_mut().apply(BrowserRequest::GoForward(t))),
            Command::ToggleInspector => {
                self.inspector_open = !self.inspector_open;
                if self.inspector_open {
                    if let Some(t) = active {
                        self.web.borrow_mut().apply(BrowserRequest::RefreshCookies(t));
                    }
                }
            }
            Command::WebInspector => active.into_iter().for_each(|t| self.web.borrow_mut().apply(BrowserRequest::OpenWebInspector(t))),
            Command::ToggleSplit => self.toggle_split(),
            Command::LinkHints => self.start_hints(),
            Command::ImportChrome => self.import_chrome(cx),
            Command::CopyMcpSetup => {
                cx.write_to_clipboard(ClipboardItem::new_string(mcp_setup_command()));
                self.toast("Copied: claude mcp add relay -- relay mcp");
            }
        }
    }

    fn toggle_split(&mut self) {
        if self.split.take().is_some() {
            return;
        }
        let (ids, active) = {
            let h = self.hub.lock();
            (h.tab_ids().to_vec(), h.active())
        };
        // Pair the active tab with the most recent other tab.
        let other = {
            let h = self.hub.lock();
            ids.iter().filter(|t| Some(**t) != active).max_by_key(|t| h.tab(**t).map(|s| s.last_active_at)).copied()
        };
        match other {
            Some(t) => {
                self.split = Some(t);
                self.web.borrow_mut().wake(t);
            }
            None => self.toast("Split view needs a second tab (⌘T)"),
        }
    }

    fn start_hints(&mut self) {
        let Some(tab) = self.active() else { return };
        let ids = self.hub.lock().tab(tab).map(|t| t.dom.interactive_ids(300)).unwrap_or_default();
        if ids.is_empty() {
            self.toast("No clickable elements on this page");
            return;
        }
        let names = inspector::hint_labels(ids.len());
        let labels: Vec<(NodeId, String)> = ids.into_iter().zip(names).collect();
        self.eval_op(tab, Op::ShowHints { hints: labels.clone() });
        self.hints = Some(Hints { tab, labels, typed: String::new() });
        self.web.borrow().focus_shell();
    }

    fn end_hints(&mut self) {
        if let Some(h) = self.hints.take() {
            self.eval_op(h.tab, Op::HideHints);
            self.web.borrow().focus_page(h.tab);
        }
    }

    fn import_chrome(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = relay_extensions::chrome_import::default_profile() else {
            self.toast("No Chrome profile found");
            return;
        };
        let preview = relay_extensions::chrome_import::preview(&profile, 20_000);
        let Some(store) = self.store.as_mut() else {
            self.toast("Profile store unavailable");
            return;
        };
        match store.import(&preview.bookmarks, &preview.history) {
            Ok(c) => {
                let mut msg = format!("Imported {} bookmarks, {} history entries", c.bookmarks, c.history);
                if !preview.extensions.is_empty() {
                    // Installing grants permissions, so it stays an explicit step.
                    msg.push_str(&format!("; {} Chrome extensions found: run `relay import-chrome --dry-run` to list them", preview.extensions.len()));
                }
                self.toast(msg);
                let _ = cx;
            }
            Err(e) => self.toast(format!("Import failed: {e}")),
        }
    }

    // ------------------------------------------------------ key input

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if let Some(h) = self.hints.as_mut() {
            match k.key.as_str() {
                "escape" => self.end_hints(),
                "backspace" => {
                    h.typed.pop();
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.modified()) {
                        h.typed.push_str(&c.to_lowercase());
                        let typed = h.typed.clone();
                        let tab = h.tab;
                        if let Some((id, _)) = h.labels.iter().find(|(_, l)| *l == typed).cloned() {
                            self.end_hints();
                            self.eval_op(tab, Op::Click { target: id });
                        } else if !h.labels.iter().any(|(_, l)| l.starts_with(&typed)) {
                            self.end_hints();
                        }
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(p) = self.palette.as_mut() else { return };
        match k.key.as_str() {
            "escape" => self.close_palette(),
            "enter" => {
                let item = p.chosen().cloned();
                self.palette = None;
                if let Some(item) = item {
                    self.run_item(item, cx);
                }
            }
            "up" => p.move_selection(-1),
            "down" | "tab" => p.move_selection(1),
            "backspace" => {
                if k.modifiers.secondary() || k.modifiers.alt {
                    p.query.clear();
                } else {
                    p.query.pop();
                }
                p.refresh(&self.sources, &self.search);
            }
            "v" if k.modifiers.secondary() => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    p.query.push_str(text.lines().next().unwrap_or_default());
                    p.refresh(&self.sources, &self.search);
                }
            }
            _ => {
                if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                    p.query.push_str(c);
                    p.selected = 0;
                    p.refresh(&self.sources, &self.search);
                } else {
                    return;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    // -------------------------------------------------------- rendering

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let h = self.hub.lock();
        let active = h.active();
        let row = |tab: &relay_bridge::TabState, cx: &mut Context<Self>| {
            let id = tab.id;
            let is_active = Some(id) == active;
            let title: SharedString = if tab.title.is_empty() { palette::short_url(&tab.url).into() } else { tab.title.clone().into() };
            let agent = tab.owner == Owner::Agent;
            let mut r = div()
                .id(("tab", id as usize))
                .flex()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .text_color(if tab.sleeping { t.muted } else { t.text })
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.activate(id);
                    cx.notify();
                }))
                .child(div().size(px(8.)).rounded_full().bg(if agent { t.agent } else if is_active { t.accent } else { t.border }))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title));
            if agent {
                r = r.bg(t.agent_bg);
            }
            if is_active {
                r = r.bg(t.active);
            }
            if tab.challenge.is_some() {
                r = r.child(div().text_color(t.warn).child("⚠"));
            }
            if tab.sleeping {
                r = r.child(div().text_color(t.muted).text_size(px(11.)).child("zz"));
            }
            r
        };
        let mut col = div().flex().flex_col().gap_0p5().p_2().w(px(220.)).h_full().bg(t.surface).border_r_1().border_color(t.border);
        col = col.child(div().px_2().pb_2().text_size(px(11.)).text_color(t.muted).child("TABS"));
        let user: Vec<_> = h.tabs().filter(|x| x.owner == Owner::User).cloned().collect();
        let agents: Vec<_> = h.tabs().filter(|x| x.owner == Owner::Agent).cloned().collect();
        drop(h);
        for tab in &user {
            col = col.child(row(tab, cx));
        }
        if !agents.is_empty() {
            col = col.child(div().mt_3().px_2().pb_1().text_size(px(11.)).text_color(t.agent).child("AGENT"));
            for tab in &agents {
                col = col.child(row(tab, cx));
            }
        }
        col.child(div().flex_1()).child(
            div()
                .id("new-tab")
                .px_2()
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.open_palette(None, true);
                    cx.notify();
                }))
                .child("+ New tab   ⌘T"),
        )
    }

    fn render_pane(&self, tab: TabId, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let placements = self.placements.clone();
        let web = self.web.clone();
        let (url, sleeping, has_dom, title, load_error) = {
            let h = self.hub.lock();
            h.tab(tab)
                .map(|s| (s.url.clone(), s.sleeping, s.dom.root().is_some(), s.title.clone(), s.load_error.clone()))
                .unwrap_or_default()
        };
        let renders = web.borrow().renders_pages();
        let mut pane = div().relative().flex_1().h_full().bg(t.page).overflow_hidden();
        if let Some(snap) = self.snapshots.get(&tab).filter(|_| sleeping) {
            pane = pane.child(img(snap.clone()).size_full().opacity(0.6));
        } else if !renders {
            pane = pane.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(t.muted)
                    .child(div().text_size(px(15.)).text_color(t.text).child(SharedString::from(if title.is_empty() { palette::short_url(&url) } else { title })))
                    .child(div().text_size(px(12.)).child(SharedString::from(url)))
                    .child(div().text_size(px(12.)).child("WebKit renders this page on macOS. This build has no WebKit.")),
            );
        } else if let Some(error) = load_error.filter(|_| !has_dom && !sleeping) {
            pane = pane.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .p_8()
                    .child(div().text_size(px(16.)).text_color(t.text).child("This page did not load"))
                    .child(div().max_w(px(520.)).text_size(px(12.)).text_color(t.muted).child(SharedString::from(error)))
                    .child(
                        div()
                            .id("retry-page")
                            .px_3()
                            .py_1p5()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.surface)
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.web.borrow_mut().apply(BrowserRequest::Reload(tab));
                                app.toast("Retrying…");
                                cx.notify();
                            }))
                            .child("Try again"),
                    ),
            );
        } else if !has_dom && !sleeping {
            pane = pane.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(t.accent));
        }
        // Report this pane's bounds so the native web view tracks the layout.
        pane.child(
            canvas(
                move |bounds, _, _| bounds,
                move |_, bounds, _, _| {
                    let mut p = placements.borrow_mut();
                    p.retain(|(id, _)| *id != tab);
                    p.push((tab, bounds));
                    web.borrow().layout(&p);
                },
            )
            .absolute()
            .size_full(),
        )
    }

    fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(active) = self.active() else {
            return div().flex_1().h_full().child(self.render_welcome(cx)).into_any_element();
        };
        let mut row = div().flex_1().h_full().flex();
        row = row.child(self.render_pane(active, cx));
        if let Some(s) = self.split.filter(|s| *s != active && self.hub.lock().tab(*s).is_some()) {
            row = row.child(div().w(px(1.)).h_full().bg(t.border)).child(self.render_pane(s, cx));
        }
        row.into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let button = |id: &'static str, label: &'static str, cmd: Command, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .text_size(px(13.))
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.run_command(cmd, cx);
                    cx.notify();
                }))
                .child(label)
        };
        div().size_full().flex().items_center().justify_center().child(
            div()
                .w(px(460.))
                .flex()
                .flex_col()
                .gap_4()
                .p_6()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .child(div().text_size(px(22.)).text_color(t.text).child("Relay"))
                .child(div().text_size(px(13.)).text_color(t.muted).child("Press ⌘K to open a URL, search, or run any command."))
                .child(div().flex().gap_2().child(button("import", "Import from Chrome", Command::ImportChrome, cx)).child(button("mcp", "Copy MCP setup", Command::CopyMcpSetup, cx)))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .font_family("Menlo")
                        .child(SharedString::from(format!("Agents: {}\nSocket: {}", mcp_setup_command(), self.mcp_socket))),
                ),
        )
    }

    fn render_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut tabs = div().flex().gap_1().px_2().h(px(32.)).items_center().border_b_1().border_color(t.border);
        for p in Panel::ALL {
            let on = p == self.panel;
            tabs = tabs.child(
                div()
                    .id(p.title())
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.panel = p;
                        if p == Panel::Cookies {
                            if let Some(tab) = app.active() {
                                app.web.borrow_mut().apply(BrowserRequest::RefreshCookies(tab));
                            }
                        }
                        cx.notify();
                    }))
                    .child(p.title()),
            );
        }
        let body = self.render_panel_body(cx);
        div()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.border)
            .font_family("Menlo")
            .child(tabs)
            .child(div().id("inspector-body").flex_1().overflow_y_scroll().p_1().text_size(px(12.)).child(body))
    }

    fn render_panel_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let h = self.hub.lock();
        let Some(tab) = h.active().and_then(|a| h.tab(a)) else {
            return div().p_2().text_color(t.muted).child("No tab").into_any_element();
        };
        let tab_id = tab.id;
        let mut col = div().flex().flex_col();
        match self.panel {
            Panel::Elements => {
                let rows = self.tree.rows(tab, 1500);
                if rows.is_empty() {
                    return div().p_2().text_color(t.muted).child("Waiting for page…").into_any_element();
                }
                for r in rows {
                    let id = r.id;
                    let marker = if !r.has_children { "  " } else if r.expanded { "▾ " } else { "▸ " };
                    col = col.child(
                        div()
                            .id(("el", id as usize))
                            .pl(px(4. + 12. * r.depth as f32))
                            .h(px(18.))
                            .whitespace_nowrap()
                            .text_color(t.code)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_hover(cx.listener(move |app, hovered: &bool, _, _| {
                                app.eval_op(tab_id, if *hovered { Op::Highlight { target: id } } else { Op::Unhighlight });
                            }))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.tree.toggle(id);
                                cx.notify();
                            }))
                            .child(SharedString::from(format!("{marker}{}", r.label))),
                    );
                }
            }
            Panel::Network => {
                let rows = inspector::network_rows(tab, "");
                if rows.is_empty() {
                    return div().p_2().text_color(t.muted).child("No requests yet").into_any_element();
                }
                for r in rows.into_iter().take(500) {
                    col = col.child(
                        div()
                            .flex()
                            .gap_2()
                            .h(px(18.))
                            .px_1()
                            .text_color(if r.failed { t.error } else { t.text })
                            .child(div().w(px(36.)).child(SharedString::from(r.method)))
                            .child(div().w(px(32.)).child(SharedString::from(r.status)))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(r.url)))
                            .child(div().text_color(t.muted).child(SharedString::from(r.time))),
                    );
                }
            }
            Panel::Console => {
                let rows = inspector::console_rows(tab, self.console_min);
                if rows.is_empty() {
                    return div().p_2().text_color(t.muted).child("Console is empty").into_any_element();
                }
                for r in rows.into_iter().rev().take(500) {
                    let color = match r.level {
                        ConsoleLevel::Error => t.error,
                        ConsoleLevel::Warn => t.warn,
                        _ => t.text,
                    };
                    col = col.child(
                        div()
                            .px_1()
                            .py_0p5()
                            .border_b_1()
                            .border_color(t.border)
                            .text_color(color)
                            .child(SharedString::from(r.text))
                            .when(!r.source.is_empty(), |d| d.child(div().text_color(t.muted).child(SharedString::from(r.source)))),
                    );
                }
            }
            Panel::Cookies => {
                if tab.cookies.is_empty() {
                    return div().p_2().text_color(t.muted).child("No cookies for this site").into_any_element();
                }
                for c in &tab.cookies {
                    col = col.child(
                        div()
                            .px_1()
                            .py_0p5()
                            .border_b_1()
                            .border_color(t.border)
                            .child(SharedString::from(format!("{} = {}", c.name, truncate(&c.value, 80))))
                            .child(div().text_color(t.muted).child(SharedString::from(format!(
                                "{}{}  {}{}",
                                c.domain,
                                c.path,
                                if c.secure { "Secure " } else { "" },
                                if c.http_only { "HttpOnly" } else { "" }
                            )))),
                    );
                }
            }
        }
        col.into_any_element()
    }

    fn render_palette(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().flex().flex_col().py_1();
        for (i, item) in p.items.iter().enumerate() {
            let on = i == p.selected;
            let item_c = item.clone();
            list = list.child(
                div()
                    .id(("pal", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(32.))
                    .px_3()
                    .mx_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.palette = None;
                        app.run_item(item_c.clone(), cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.)).child(SharedString::from(item.title())))
                    .child(div().text_size(px(12.)).text_color(t.muted).child(SharedString::from(item.detail()))),
            );
        }
        let query: SharedString = if p.query.is_empty() { "Search or enter address…".into() } else { p.query.clone().into() };
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(90.)).bg(t.scrim).child(
            div()
                .id("palette")
                .w(px(560.))
                .max_h(px(480.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_palette();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .text_size(px(15.))
                        .text_color(if p.query.is_empty() { t.muted } else { t.text })
                        .child(query)
                        .child(div().w(px(1.5)).h(px(18.)).ml_0p5().bg(t.accent)),
                )
                .child(list),
        )
    }

    /// What the window title should say: the active page, or Relay itself.
    fn document_title(&self) -> String {
        let h = self.hub.lock();
        match h.active().and_then(|a| h.tab(a)) {
            Some(t) if !t.title.is_empty() => format!("{} — Relay", t.title),
            Some(t) => format!("{} — Relay", palette::short_url(&t.url)),
            None => "Relay".to_string(),
        }
    }

    fn render_status(&self) -> impl IntoElement {
        let t = self.theme;
        let (url, total, asleep, agents) = {
            let h = self.hub.lock();
            let url = h.active().and_then(|a| h.tab(a)).map(|x| x.url.clone()).unwrap_or_default();
            (url, h.tabs().count(), h.tabs().filter(|x| x.sleeping).count(), h.tabs().filter(|x| x.owner == Owner::Agent).count())
        };
        let mut right = format!("{total} tabs");
        if asleep > 0 {
            right.push_str(&format!(" · {asleep} asleep"));
        }
        if agents > 0 {
            right.push_str(&format!(" · {agents} agent"));
        }
        right.push_str(" · agents: relay mcp");
        div()
            .h(px(24.))
            .flex()
            .items_center()
            .px_3()
            .gap_3()
            .text_size(px(11.))
            .text_color(t.muted)
            .bg(t.surface)
            .border_t_1()
            .border_color(t.border)
            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(match (&self.hints, &self.toast) {
                // Native web views cover GPUI overlays, so hint status lives here.
                (Some(h), _) => format!("Link hints: type a label{} · Esc cancels", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed.to_uppercase()) }),
                (None, Some((m, _))) => m.clone(),
                (None, None) => url,
            })))
            .child(SharedString::from(right))
    }
}

impl Render for RelayApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = Theme::for_appearance(window.appearance());
        let title = self.document_title();
        if title != self.window_title {
            self.window_title = title.clone();
            window.set_window_title(&title);
        }
        // Panes re-register their bounds every frame; drop tabs not shown.
        let visible = self.visible_tabs();
        self.placements.borrow_mut().retain(|(id, _)| visible.contains(id));
        if self.palette.is_some() {
            // Keep native views from covering the palette overlay.
            self.web.borrow().layout(&[]);
            self.placements.borrow_mut().clear();
        }
        let t = self.theme;
        let mut main = div().flex_1().flex().overflow_hidden().child(self.render_tab_bar(cx));
        let palette_open = self.palette.is_some();
        main = main.child(if palette_open { div().flex_1().h_full().bg(t.page).into_any_element() } else { self.render_content(cx).into_any_element() });
        if self.inspector_open {
            main = main.child(self.render_inspector(cx));
        }
        let mut root = div()
            .key_context("Relay")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .font_family(".SystemUIFont")
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|app, _: &OpenPalette, _, cx| {
                if app.palette.is_some() {
                    app.close_palette();
                } else {
                    app.open_palette(None, false);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &OpenLocation, _, cx| {
                let url = app.active().and_then(|a| app.hub.lock().tab(a).map(|t| t.url.clone()));
                app.open_palette(url, false);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &NewTab, _, cx| app.run_command(Command::NewTab, cx)))
            .on_action(cx.listener(|app, _: &CloseTab, _, cx| app.run_command(Command::CloseTab, cx)))
            .on_action(cx.listener(|app, _: &ReopenClosedTab, _, cx| app.run_command(Command::ReopenClosedTab, cx)))
            .on_action(cx.listener(|app, _: &Reload, _, cx| app.run_command(Command::Reload, cx)))
            .on_action(cx.listener(|app, _: &Back, _, cx| app.run_command(Command::Back, cx)))
            .on_action(cx.listener(|app, _: &Forward, _, cx| app.run_command(Command::Forward, cx)))
            .on_action(cx.listener(|app, _: &NextTab, _, cx| {
                app.cycle(1);
                cx.notify()
            }))
            .on_action(cx.listener(|app, _: &PrevTab, _, cx| {
                app.cycle(-1);
                cx.notify()
            }))
            .on_action(cx.listener(|app, _: &ToggleInspector, _, cx| {
                app.run_command(Command::ToggleInspector, cx);
                cx.notify()
            }))
            .on_action(cx.listener(|app, _: &WebInspector, _, cx| app.run_command(Command::WebInspector, cx)))
            .on_action(cx.listener(|app, _: &ToggleSplit, _, cx| {
                app.run_command(Command::ToggleSplit, cx);
                cx.notify()
            }))
            .on_action(cx.listener(|app, _: &LinkHints, _, cx| {
                app.run_command(Command::LinkHints, cx);
                cx.notify()
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()));
        macro_rules! select {
            ($($a:ident => $i:expr),*) => {
                $(root = root.on_action(cx.listener(|app, _: &$a, _, cx| { app.select_index($i); cx.notify() }));)*
            };
        }
        select!(SelectTab1 => 0, SelectTab2 => 1, SelectTab3 => 2, SelectTab4 => 3, SelectTab5 => 4, SelectTab6 => 5, SelectTab7 => 6, SelectTab8 => 7, SelectTab9 => 8);
        root = root.child(main).child(self.render_status());
        if let Some(p) = &self.palette {
            root = root.child(self.render_palette(p, cx));
        }
        root
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Built-in list plus the user's `relay_home()/filters.txt` (EasyList syntax).
fn blocklist() -> String {
    let mut s = relay_webkit::content_rules::BUILTIN_FILTERS.to_string();
    if let Ok(extra) = std::fs::read_to_string(relay_bridge::paths::relay_home().join("filters.txt")) {
        s.push('\n');
        s.push_str(&extra);
    }
    s
}

fn is_web_url(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://") || l == "about:blank"
}

pub fn mcp_setup_command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "relay".into());
    format!("claude mcp add relay -- {exe} mcp")
}

/// Await a oneshot with a deadline on GPUI's executor.
async fn futures_timeout(
    rx: tokio::sync::oneshot::Receiver<relay_bridge::CommandResult>,
    d: Duration,
    cx: &mut gpui::AsyncApp,
) -> Option<serde_json::Value> {
    let timer = cx.background_executor().timer(d);
    let rx = std::pin::pin!(rx);
    let timer = std::pin::pin!(timer);
    match futures_select(rx, timer).await {
        Some(Ok(r)) if r.ok => Some(r.value),
        _ => None,
    }
}

/// Minimal two-future race without pulling in the `futures` crate.
async fn futures_select<A, B>(mut a: std::pin::Pin<&mut A>, mut b: std::pin::Pin<&mut B>) -> Option<A::Output>
where
    A: std::future::Future,
    B: std::future::Future,
{
    std::future::poll_fn(move |cx| {
        if let std::task::Poll::Ready(v) = a.as_mut().poll(cx) {
            return std::task::Poll::Ready(Some(v));
        }
        if b.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(None);
        }
        std::task::Poll::Pending
    })
    .await
}
