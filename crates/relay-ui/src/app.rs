//! The GPUI shell: vertical tab bar, content panes (WebKit on macOS), the
//! ⌘K palette, the native Inspector, link hints, and the status bar. It reads
//! the same `SharedHub` the MCP server reads; MCP requests reach WebKit via
//! the pump loop below.

use crate::inspector::{self, ConsoleRow, ElementRow, NetworkRow, Panel, Tree};
use crate::keymap::{self, ActionId, Keymap};
use crate::library::{Library, Mode};
use crate::mcp_config;
use crate::palette::{self, Command, Item, Palette, Sources};
use crate::startpage::{Section, StartItem, StartPage};
use crate::tabstrip;
use crate::theme::{AccentChoice, Theme, ThemeChoice};
use crate::web::{EditAction, WebEvent, WebHost};
use crate::webapp;
use gpui::{
    actions, canvas, div, img, prelude::*, px, Bounds, ClipboardItem, Context, FocusHandle, Image, ImageFormat,
    KeyBinding, KeyDownEvent, Menu, MenuItem, MouseButton, Pixels, SharedString, Window,
};
use relay_bridge::{host_of, is_http_url, is_openable_url, BrowserRequest, ConsoleLevel, Hub, NodeId, Op, Owner, SharedHub, TabId, TabView};
use relay_extensions::store::{SessionTab, Store, WebAppRecord};
use relay_webkit::sleep::SleepPolicy;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
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
        ToggleSettings,
        LinkHints,
        CopyCleanUrl,
        CopyMarkdown,
        ToggleBookmark,
        TogglePin,
        Find,
        ZoomIn,
        ZoomOut,
        ZoomReset,
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
        ToggleTabStrip,
        NextTabStripPosition,
        InstallWebApp,
        RemoveWebApp,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Undo,
        Redo,
        Minimize,
        Help,
        ShowHistory,
        ShowBookmarks,
        ToggleAgents,
        CycleAgentPolicy,
        ToggleReader,
        TogglePictureInPicture,
        ShowDownloads,
    ]
);

/// Every global shortcut. `secondary` is ⌘ on macOS and Ctrl elsewhere.
///
/// These bindings are also what the native menu bar shows as key
/// equivalents: on macOS the menu receives them before the focused
/// WKWebView, which is why shortcuts work while a page has focus.
pub fn key_bindings() -> Vec<KeyBinding> {
    crate::keymap::Keymap::default().bindings()
}

pub fn app_menus() -> Vec<Menu> {
    app_menus_named(None)
}

fn app_menus_named(app_name: Option<&str>) -> Vec<Menu> {
    fn item(name: &'static str, action: impl gpui::Action) -> MenuItem {
        MenuItem::action(name, action)
    }
    let sep = MenuItem::separator;
    vec![
        Menu {
            name: app_name.map(|s| SharedString::from(s.to_string())).unwrap_or_else(|| "Relay".into()),
            items: vec![
                item("Settings…", ToggleSettings),
                item("Pause / Resume Agents", ToggleAgents),
                item("Cycle Agent Access on This Site", CycleAgentPolicy),
                sep(),
                item("Quit Relay", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                item("Command Palette…", OpenPalette),
                item("Open Location…", OpenLocation),
                sep(),
                item("New Tab", NewTab),
                item("Close Tab", CloseTab),
                item("Reopen Closed Tab", ReopenClosedTab),
                sep(),
                item("Install This Site as an App…", InstallWebApp),
                item("Remove This Site's App", RemoveWebApp),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                item("Undo", Undo),
                item("Redo", Redo),
                sep(),
                item("Cut", Cut),
                item("Copy", Copy),
                item("Paste", Paste),
                item("Select All", SelectAll),
                sep(),
                item("Find in Page…", Find),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                item("Fold / Unfold Tab Strip", ToggleTabStrip),
                item("Move Tab Strip", NextTabStripPosition),
                sep(),
                item("Reading Mode", ToggleReader),
                item("Picture in Picture", TogglePictureInPicture),
                sep(),
                item("Downloads…", ShowDownloads),
                item("Reload Page", Reload),
                item("Zoom In", ZoomIn),
                item("Zoom Out", ZoomOut),
                item("Actual Size", ZoomReset),
                sep(),
                item("Toggle Inspector", ToggleInspector),
                item("WebKit Web Inspector", WebInspector),
                item("Toggle Split View", ToggleSplit),
            ],
        },
        Menu {
            name: "History".into(),
            items: vec![item("Back", Back), item("Forward", Forward), sep(), item("Show All History…", ShowHistory)],
        },
        Menu {
            name: "Bookmarks".into(),
            items: vec![
                item("Bookmark This Page", ToggleBookmark),
                item("Pin / Unpin Tab", TogglePin),
                sep(),
                item("Show All Bookmarks…", ShowBookmarks),
                sep(),
                item("Copy Clean URL", CopyCleanUrl),
                item("Copy Page as Markdown", CopyMarkdown),
            ],
        },
        Menu {
            name: "Tab".into(),
            items: vec![
                item("Next Tab", NextTab),
                item("Previous Tab", PrevTab),
                item("Link Hints", LinkHints),
                sep(),
                item("Select Tab 1", SelectTab1),
                item("Select Tab 2", SelectTab2),
                item("Select Tab 3", SelectTab3),
                item("Select Tab 4", SelectTab4),
                item("Select Tab 5", SelectTab5),
                item("Select Tab 6", SelectTab6),
                item("Select Tab 7", SelectTab7),
                item("Select Tab 8", SelectTab8),
                item("Select Tab 9", SelectTab9),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![item("Minimize", Minimize)],
        },
        Menu {
            name: "Help".into(),
            items: vec![item("Relay on GitHub", Help)],
        },
    ]
}

/// Inspector row data built once per hub version/tab/panel/filter, then
/// painted without walking the DOM or formatting network/console entries
/// again on every frame.
#[derive(Debug)]
enum InspectorRows {
    Elements(Vec<ElementRow>),
    Network(Vec<NetworkRow>),
    Console(Vec<ConsoleRow>),
    Cookies(Vec<relay_bridge::Cookie>),
    Empty(&'static str),
}

#[derive(Debug)]
struct InspectorCache {
    version: u64,
    tab: TabId,
    panel: Panel,
    tree_epoch: u64,
    console_min: ConsoleLevel,
    failed_only: bool,
    rows: InspectorRows,
}

/// The ⌘F bar: what is being searched in which tab, and the last answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Agent,
    Inspector,
}

struct ShortcutEditor {
    recording: Option<ActionId>,
}

struct FindBar {
    tab: TabId,
    query: String,
    found: Option<bool>,
}

struct Hints {
    tab: TabId,
    labels: Vec<(NodeId, String)>,
    typed: String,
}

/// The last load recorded for a tab. Title changes within one load must not
/// count as another visit; they refresh this in memory for the next load.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedVisit {
    load_seq: u64,
    title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitDecision {
    Record,
    TitleOnly,
    Ignore,
}

fn visit_decision(load_seq: u64, title: &str, recorded: Option<&RecordedVisit>) -> VisitDecision {
    if load_seq == 0 {
        return VisitDecision::Ignore;
    }
    match recorded {
        None => VisitDecision::Record,
        Some(old) if old.load_seq != load_seq => VisitDecision::Record,
        Some(old) if old.title != title => VisitDecision::TitleOnly,
        Some(_) => VisitDecision::Ignore,
    }
}

pub struct RelayApp {
    hub: SharedHub,
    web: Rc<RefCell<WebHost>>,
    requests: UnboundedReceiver<BrowserRequest>,
    store: Option<Store>,
    focus: FocusHandle,
    theme: Theme,
    theme_choice: ThemeChoice,
    accent_choice: AccentChoice,
    /// Decoded tab icons by host; `None` means the fetch failed and retrying
    /// the same host this session is pointless.
    favicons: HashMap<String, Option<Arc<Image>>>,
    favicon_pending: HashSet<String>,
    shortcuts: Option<ShortcutEditor>,
    keymap: Keymap,
    palette: Option<Palette>,
    /// ⌘T opens the palette for a new tab; ⌘K/⌘L target the current tab.
    palette_new_tab: bool,
    settings_open: bool,
    blocking: bool,
    agent_eval: bool,
    shell_control: bool,
    sleep_minutes: u64,
    mcp_enabled: bool,
    confirm_clear_history: bool,
    sources: Sources,
    inspector_open: bool,
    sidebar_tab: SidebarTab,
    connect_open: bool,
    downloads_open: bool,
    downloads_selected: usize,
    reader_tabs: HashSet<TabId>,
    /// History/Bookmarks modal; `None` when closed.
    library: Option<Library>,
    panel: Panel,
    tree: Tree,
    tree_epoch: u64,
    inspector_cache: Option<InspectorCache>,
    console_min: ConsoleLevel,
    network_failed_only: bool,
    split: Option<TabId>,
    closed: Vec<String>,
    hints: Option<Hints>,
    find: Option<FindBar>,
    toast: Option<(String, Instant)>,
    search: String,
    /// `bangs` setting, parsed; invalid settings are ignored with a log line.
    custom_bangs: Vec<(String, String)>,
    seen_version: u64,
    recorded: HashMap<TabId, RecordedVisit>,
    sleep_policy: SleepPolicy,
    last_sleep_check: Instant,
    placements: Rc<RefCell<Vec<(TabId, Bounds<Pixels>)>>>,
    snapshots: HashMap<TabId, Arc<Image>>,
    mcp_socket: String,
    /// `current_exe` is a syscall; compute the setup line once per process.
    mcp_setup: String,
    /// Last title pushed to the native window; avoids a call every frame.
    window_title: String,
    /// Document title cached for `seen_version` (title updates tick the hub).
    title_cache: (u64, String),
    /// One long-lived subscription: `subscribe()` per 16 ms tick allocated a
    /// receiver and dropped it, which also made the pump look busy.
    hub_events: tokio::sync::watch::Receiver<u64>,
    /// The session as last written, so the periodic save skips no-op writes.
    saved_session: Vec<SessionTab>,
    /// Tabs whose web view currently sends the compat (Safari) user agent.
    compat_tabs: std::collections::HashSet<TabId>,
    /// `restore_session` setting: when off, no tabs are written at all.
    session_enabled: bool,
    /// Folded/edge placement, persisted as `tabs_position` + `tabs_folded`.
    strip: tabstrip::StripState,
    /// Which row the pointer is over, so its close button can appear.
    hovered_tab: Option<TabId>,
    /// Focus the page once its native view is attached (a slept tab wakes on
    /// the next frame; focusing before that is a silent no-op).
    focus_pending: Option<TabId>,
    /// The first real page gets focus once, after the CLI URL is attached.
    focus_initial: bool,
    /// An installed web app window: one page, no browser chrome.
    app_mode: bool,
    /// Cached new-tab data (recent/frequent/apps/bookmarks), refreshed when
    /// a visit, bookmark, or installed app changes.
    start_page: StartPage,
    /// The installed app's display name, for its menu and title.
    app_name: Option<String>,
    /// Second click on "Remove This Site's App" within the toast window
    /// actually deletes the bundle.
    pending_remove_app: Option<(String, Instant)>,
}

impl RelayApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hub: SharedHub,
        requests: UnboundedReceiver<BrowserRequest>,
        store: Option<Store>,
        mcp_socket: String,
        app_mode: bool,
        app_name: Option<String>,
        app_icon: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let web = WebHost::new(hub.clone(), window);
        let search = store.as_ref().and_then(|s| s.setting("search").ok().flatten()).unwrap_or_else(|| palette::DEFAULT_SEARCH.to_string());
        let blocking = setting_enabled(store.as_ref().and_then(|s| s.setting("blocking").ok().flatten()).as_deref());
        let agent_eval = setting_enabled(store.as_ref().and_then(|s| s.setting("agent_eval").ok().flatten()).as_deref());
        let shell_control = setting_enabled(store.as_ref().and_then(|s| s.setting("shell_control").ok().flatten()).as_deref());
        hub.set_agent_eval(agent_eval);
        let custom_bangs = store
            .as_ref()
            .and_then(|s| s.setting("bangs").ok().flatten())
            .map(|raw| {
                crate::bangs::parse_custom(&raw).unwrap_or_else(|bad| {
                    eprintln!("relay: ignoring the bangs setting, invalid entry {bad:?}");
                    vec![]
                })
            })
            .unwrap_or_default();
        let sleep_minutes = store.as_ref().and_then(|s| s.setting("sleep_minutes").ok().flatten()).and_then(|m| m.parse::<u64>().ok()).unwrap_or(15);
        let mcp_enabled = setting_enabled(store.as_ref().and_then(|s| s.setting("mcp").ok().flatten()).as_deref());
        let strip = tabstrip::StripState::from_settings(
            store.as_ref().and_then(|s| s.setting("tabs_position").ok().flatten()).as_deref(),
            store.as_ref().and_then(|s| s.setting("tabs_folded").ok().flatten()).as_deref(),
        );
        if let Some(store) = &store {
            for (host, policy) in store.all_agent_policies().unwrap_or_default() {
                hub.set_agent_policy(&host, policy);
            }
        }
        let start_page = match &store {
            Some(store) => {
                let sections = store.setting("start_sections").ok().flatten();
                StartPage::build(store, sections.as_deref()).unwrap_or_default()
            }
            None => StartPage::welcome(),
        };
        if blocking {
            web.load_content_rules(&rules_json(store.as_ref()));
        }
        let focus = cx.focus_handle();
        let hub_events = hub.subscribe();
        window.focus(&focus);
        let theme_choice = store
            .as_ref()
            .and_then(|s| s.setting("theme").ok().flatten())
            .and_then(|v| ThemeChoice::parse(&v))
            .unwrap_or_default();
        let accent_choice = store
            .as_ref()
            .and_then(|s| s.setting("accent").ok().flatten())
            .and_then(|v| AccentChoice::parse(&v))
            .unwrap_or_default();
        let keymap = Keymap::from_setting(store.as_ref().and_then(|s| s.setting("keybindings").ok().flatten()).as_deref());
        let mut app = Self {
            hub,
            web: Rc::new(RefCell::new(web)),
            requests,
            store,
            focus,
            theme: Theme::for_choice(window.appearance(), theme_choice, accent_choice),
            theme_choice,
            accent_choice,
            favicons: cached_favicons(),
            favicon_pending: HashSet::new(),
            shortcuts: None,
            keymap,
            palette: None,
            palette_new_tab: false,
            settings_open: false,
            blocking,
            agent_eval,
            shell_control,
            sleep_minutes,
            mcp_enabled,
            confirm_clear_history: false,
            sources: Sources::default(),
            inspector_open: false,
            sidebar_tab: SidebarTab::Inspector,
            connect_open: false,
            downloads_open: false,
            downloads_selected: 0,
            reader_tabs: HashSet::new(),
            library: None,
            panel: Panel::Elements,
            tree: Tree::default(),
            tree_epoch: 0,
            inspector_cache: None,
            console_min: ConsoleLevel::Log,
            network_failed_only: false,
            split: None,
            closed: vec![],
            hints: None,
            find: None,
            toast: None,
            search,
            custom_bangs,
            seen_version: 0,
            recorded: HashMap::new(),
            sleep_policy: SleepPolicy { idle_after: Duration::from_secs(sleep_minutes.saturating_mul(60)), keep_awake_min: 1 },
            last_sleep_check: Instant::now(),
            placements: Rc::default(),
            snapshots: HashMap::new(),
            mcp_socket,
            hub_events,
            mcp_setup: mcp_setup_command(),
            window_title: String::new(),
            title_cache: (u64::MAX, String::new()),
            saved_session: vec![],
            session_enabled: false,
            compat_tabs: Default::default(),
            strip,
            hovered_tab: None,
            focus_pending: None,
            focus_initial: true,
            app_mode,
            app_name: app_name.clone(),
            start_page,
            pending_remove_app: None,
        };
        if app_mode {
            // A web app window is one page, not the user's last browsing
            // session; never restore or overwrite that session from here.
            // Extensions stay with the browser: a second copy of every
            // background page in each installed app would only cost memory.
            app.session_enabled = false;
        } else {
            app.load_installed_extensions();
            app.restore_session();
        }
        let menus = match app_name.as_deref() {
            Some(name) => app_menus_named(Some(name)),
            None => app_menus(),
        };
        cx.clear_key_bindings();
        cx.bind_keys(app.keymap.bindings());
        cx.set_menus(menus);
        if let Some(icon) = app_icon {
            app.web.borrow().set_application_icon(&icon);
        }
        // Menu, Dock, logout and ⌘Q all end here, not only the ⌘Q action.
        cx.on_app_quit(|app, _| {
            app.save_session();
            async {}
        })
        .detach();
        app.start_pump(cx);
        app
    }

    /// Reopen the saved session's tabs asleep (no WKWebView, 0 MB); only the
    /// previously active tab loads now. Off with `restore_session = off`.
    fn restore_session(&mut self) {
        let Some(store) = &self.store else { return };
        self.session_enabled = setting_enabled(store.setting("restore_session").ok().flatten().as_deref());
        if !self.session_enabled {
            // Turning restore off also drops whatever an earlier run saved.
            let _ = store.clear_session();
            return;
        }
        let saved = store.load_session().unwrap_or_default();
        let (restored, active) = self.hub.update(|h| restore_into(h, &saved));
        for (tab, url, scroll_y) in &restored {
            self.web.borrow_mut().restore_asleep(*tab, url, *scroll_y);
        }
        if let Some(tab) = active {
            self.web.borrow_mut().wake(tab);
        }
        self.saved_session = saved;
    }

    /// Persist the user's open tabs when they changed since the last write.
    fn save_session(&mut self) {
        if !self.session_enabled {
            return;
        }
        let Some(store) = &self.store else { return };
        let web = self.web.borrow();
        let tabs = session_snapshot(&self.hub.lock(), |t| web.slept_scroll_y(t));
        drop(web);
        if tabs == self.saved_session {
            return;
        }
        match store.save_session(&tabs) {
            Ok(()) => self.saved_session = tabs,
            Err(e) => eprintln!("relay: could not save the session: {e}"),
        }
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
                    if !is_openable_url(&url) {
                        continue; // window.open("file:///…") and friends stay closed
                    }
                    // A web app window has no tab strip; a popup navigates the
                    // one page instead of adding chrome the user cannot see.
                    if self.app_mode {
                        self.open_url(url, false);
                        continue;
                    }
                    let owner = self.hub.lock().tab(opener).map(|t| t.owner).unwrap_or(Owner::User);
                    let tab = self.hub.update(|h| {
                        let id = h.create_tab(&url, owner);
                        if let Some(pos) = h.tab_ids().iter().position(|t| *t == opener) {
                            // Never inside the pinned block at the top.
                            let pinned = h.tabs().filter(|t| t.pinned).count();
                            h.move_tab(id, (pos + 1).max(pinned));
                        }
                        if owner == Owner::User {
                            h.set_active(id);
                        }
                        id
                    });
                    self.web.borrow_mut().apply(BrowserRequest::Open { tab, url });
                }
                WebEvent::RulesApplied { reload } => {
                    self.web.borrow_mut().apply(BrowserRequest::Reload(reload));
                }
                WebEvent::Found { tab, found } => {
                    if let Some(f) = self.find.as_mut().filter(|f| f.tab == tab) {
                        f.found = Some(found);
                    }
                }
                WebEvent::Slept(tab) => {
                    if let Some(jpeg) = self.web.borrow().snapshot(tab) {
                        self.snapshots.insert(tab, Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg)));
                    }
                }
            }
        }
        let version = *self.hub_events.borrow();
        if version != self.seen_version {
            self.seen_version = version;
            changed = true;
            self.after_hub_change(cx);
        }
        if self.last_sleep_check.elapsed() > Duration::from_secs(30) {
            self.last_sleep_check = Instant::now();
            self.sleep_idle_tabs(cx);
            self.save_session();
            // Pick up a start_sections change made from the CLI while open.
            let before = self.start_page.clone();
            self.refresh_start_page();
            if self.start_page != before {
                changed = true;
            }
        }
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
            self.toast = None;
            changed = true;
        }
        self.try_focus_pending();
        if self.focus_initial && self.palette.is_none() && !self.settings_open && self.hints.is_none() {
            if let Some(tab) = self.active() {
                if self.web.borrow().focus_page(tab) {
                    self.focus_initial = false;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn after_hub_change(&mut self, cx: &mut Context<Self>) {
        let hotkeys = self.hub.take_hotkeys();
        let mut loads = vec![];
        let mut ready = vec![];
        let mut visited = false;
        {
            let h = self.hub.lock();
            for t in h.tabs() {
                let rec = self.recorded.get(&t.id);
                if page_loaded_since(rec.map(|r| r.load_seq), t.load_seq) {
                    loads.push(t.id);
                }
                if t.dom.root().is_some() {
                    ready.push(t.id);
                }
                match visit_decision(t.load_seq, &t.title, rec) {
                    VisitDecision::Record => {
                        if let (Some(store), Owner::User) = (&self.store, t.owner) {
                            let _ = store.record_visit(&t.url, &t.title, now_unix());
                            visited = true;
                        }
                        self.recorded.insert(t.id, RecordedVisit { load_seq: t.load_seq, title: t.title.clone() });
                    }
                    VisitDecision::TitleOnly => {
                        if let Some(old) = self.recorded.get_mut(&t.id) {
                            old.title = t.title.clone();
                        }
                    }
                    VisitDecision::Ignore => {}
                }
            }
        }
        for tab in loads {
            self.web.borrow_mut().after_load(tab);
            self.apply_site_prefs(tab);
        }
        // Keep the sleep snapshot until the reloaded page has a DOM; it is
        // what covers the white WebKit view while the tab warms up.
        for tab in ready {
            self.snapshots.remove(&tab);
        }
        for (tab, key) in hotkeys {
            if key == "f" && Some(tab) == self.active() {
                self.start_hints();
            }
        }
        if visited {
            self.refresh_start_page();
        }
        let mut shell_changed = false;
        for change in self.hub.take_shell_changes() {
            match change {
                relay_bridge::ShellChange::Theme { theme, accent } => {
                    if let Some(theme) = ThemeChoice::parse(&theme) {
                        self.theme_choice = theme;
                    }
                    if let Some(accent) = AccentChoice::parse(&accent) {
                        self.accent_choice = accent;
                    }
                    shell_changed = true;
                }
                relay_bridge::ShellChange::StartSections { .. } => {
                    self.refresh_start_page();
                    shell_changed = true;
                }
                relay_bridge::ShellChange::Setting { key, value } => {
                    self.apply_shell_setting(&key, &value);
                    shell_changed = true;
                }
            }
        }
        if shell_changed {
            self.title_cache = (u64::MAX, String::new());
            cx.notify();
        }
        self.request_favicons(cx);
    }

    /// Fetch at most one icon per host from the page itself. Cross-origin
    /// icons without CORS fail closed to the letter/dot fallback.
    fn request_favicons(&mut self, cx: &mut Context<Self>) {
        let mut fetch: Vec<(TabId, String)> = vec![];
        {
            let h = self.hub.lock();
            for t in h.tabs() {
                let Some(host) = host_of(&t.url) else { continue };
                if !self.favicons.contains_key(&host) && !self.favicon_pending.contains(&host) {
                    fetch.push((t.id, host));
                }
            }
        }
        for (tab, host) in fetch {
            let Ok((script, rx)) = self.hub.begin_command(tab, Op::Favicon) else { continue };
            self.favicon_pending.insert(host.clone());
            self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
            cx.spawn(async move |this, cx| {
                let reply = futures_timeout(rx, Duration::from_secs(4), cx).await;
                let _ = this.update(cx, |app, cx| {
                    app.finish_favicon(host, reply);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn finish_favicon(&mut self, host: String, reply: Option<serde_json::Value>) {
        self.favicon_pending.remove(&host);
        let payload = reply.as_ref().and_then(favicon_payload);
        if let Some((bytes, ext)) = &payload {
            cache_favicon_file(&host, bytes, ext);
        }
        let image = payload.as_ref().map(|(bytes, ext)| Arc::new(favicon_decode(bytes, ext)));
        self.favicons.insert(host, image);
    }

    /// Rebuild the new-tab cache after a profile change. Cheap: a few
    /// indexed SQLite reads, only on a visit/bookmark/app change.
    fn refresh_start_page(&mut self) {
        let Some(store) = &self.store else {
            self.start_page = StartPage::welcome();
            return;
        };
        let sections = store.setting("start_sections").ok().flatten();
        self.start_page = StartPage::build(store, sections.as_deref()).unwrap_or_default();
    }

    fn sleep_idle_tabs(&mut self, cx: &mut Context<Self>) {
        if self.sleep_minutes == 0 {
            return;
        }
        let visible: Vec<TabId> = self.visible_tabs();
        let due = {
            let h = self.hub.lock();
            self.sleep_policy.due(&h, &visible, Instant::now())
        };
        for tab in due {
            let _ = self.sleep_with_scroll(tab, cx, |app, tab| {
                // The user (or an agent) may have used the tab meanwhile.
                app.sleep_minutes != 0 && {
                    let h = app.hub.lock();
                    app.sleep_policy.due(&h, &app.visible_tabs(), Instant::now()).contains(&tab)
                }
            });
        }
    }

    /// Ask the page for its scroll offset, then sleep it if `still` holds
    /// when the reply (or a 500 ms timeout) arrives.
    fn sleep_with_scroll(&mut self, tab: TabId, cx: &mut Context<Self>, still: fn(&RelayApp, TabId) -> bool) -> bool {
        let Ok((script, rx)) = self.hub.begin_command(tab, Op::PageInfo) else { return false };
        self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
        cx.spawn(async move |this, cx| {
            let reply = futures_timeout(rx, Duration::from_millis(500), cx).await;
            let scroll = reply
                .map(|v| (v["scroll_x"].as_f64().unwrap_or(0.0), v["scroll_y"].as_f64().unwrap_or(0.0)))
                .unwrap_or((0.0, 0.0));
            let _ = this.update(cx, |app, cx| {
                if still(app, tab) {
                    // The woken view starts with WebKit's default agent.
                    app.compat_tabs.remove(&tab);
                    app.web.borrow_mut().sleep(tab, scroll);
                    cx.notify();
                }
            });
        })
        .detach();
        true
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
        // A web app window always reuses its single page.
        let new_tab = new_tab && !self.app_mode;
        match (new_tab, active) {
            (false, Some(tab)) => {
                self.web.borrow_mut().apply(BrowserRequest::Navigate { tab, url });
                self.focus_pending = Some(tab);
                self.try_focus_pending();
            }
            _ => {
                let tab = self.hub.update(|h| {
                    let id = h.create_tab(&url, Owner::User);
                    h.set_active(id);
                    id
                });
                self.web.borrow_mut().apply(BrowserRequest::Open { tab, url });
                self.focus_pending = Some(tab);
                self.try_focus_pending();
            }
        }
    }

    fn activate(&mut self, tab: TabId) {
        if self.find.as_ref().is_some_and(|f| f.tab != tab) {
            self.find = None;
        }
        // Hints belong to the tab they were drawn on; do not leave them armed
        // for the next tab (Esc would then focus the old page).
        if let Some(h) = self.hints.take() {
            self.eval_op(h.tab, Op::HideHints);
        }
        self.hub.update(|h| h.set_active(tab));
        self.web.borrow_mut().wake(tab);
        self.focus_pending = Some(tab);
        self.try_focus_pending();
    }

    /// Focus the page only when the native view exists and no overlay owns
    /// the keyboard. Called from the pump until it succeeds.
    fn try_focus_pending(&mut self) {
        let Some(tab) = self.focus_pending else { return };
        if self.active() != Some(tab) || self.palette.is_some() || self.settings_open || self.hints.is_some() {
            self.focus_pending = None;
            return;
        }
        if self.web.borrow().focus_page(tab) {
            self.focus_pending = None;
        }
    }

    fn focus_active_page(&mut self) {
        if let Some(tab) = self.active() {
            self.focus_pending = Some(tab);
            self.try_focus_pending();
        }
    }

    fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active() {
            self.close_tab(tab, cx);
        }
    }

    /// Close one tab from ⌘W, its row's ×, or a middle click. Pinned tabs
    /// are put to sleep; the last tab in a web app quits the app window.
    fn close_tab(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.hub.lock().tab(tab).is_some_and(|t| t.pinned) {
            self.put_pinned_away(tab, cx);
            return;
        }
        let last = self.hub.lock().tabs().count() <= 1;
        if let Some(t) = self.hub.update(|h| h.close_tab(tab)) {
            if t.url.starts_with("http") {
                self.closed.push(t.url);
            }
        }
        if self.split == Some(tab) {
            self.split = None;
        }
        if self.hovered_tab == Some(tab) {
            self.hovered_tab = None;
        }
        self.snapshots.remove(&tab);
        self.compat_tabs.remove(&tab);
        self.web.borrow_mut().apply(BrowserRequest::Close(tab));
        if let Some(next) = self.active() {
            self.activate(next);
        }
        if self.app_mode && last {
            self.save_session();
            cx.quit();
        }
    }

    /// ⌘W on a pinned tab keeps it: switch to a neighbour and sleep it (0 MB).
    fn put_pinned_away(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.hub.lock().tab_ids().len() < 2 {
            self.toast("Pinned tabs stay open; unpin with ⌘⌥P to close");
            return;
        }
        if self.split == Some(tab) {
            self.split = None;
        }
        if self.active() == Some(tab) {
            self.cycle(1);
        }
        let asleep = self.hub.lock().tab(tab).is_some_and(|t| t.sleeping);
        if asleep || self.sleep_with_scroll(tab, cx, |app, tab| app.active() != Some(tab) && !app.visible_tabs().contains(&tab)) {
            self.toast("Pinned tab put to sleep");
        } else {
            self.toast("Switched away; the pinned tab stays open");
        }
    }

    /// Apply the tab's host prefs after each load: zoom (1.0 when none) and
    /// compat mode. A compat site first fetched with WebKit's own user agent
    /// is reloaded once with the Safari one.
    fn apply_site_prefs(&mut self, tab: TabId) {
        let Some(host) = self.hub.lock().tab(tab).map(|t| crate::clean_url::host_of(&t.url)).filter(|h| !h.is_empty()) else {
            return;
        };
        let prefs = self.store.as_ref().and_then(|s| s.site_pref(&host).ok()).unwrap_or_default();
        self.web.borrow().set_zoom(tab, prefs.zoom);
        match compat_change(self.compat_tabs.contains(&tab), prefs.compat_mode) {
            CompatChange::None => {}
            CompatChange::Enable => {
                self.compat_tabs.insert(tab);
                self.web.borrow().set_user_agent(tab, Some(SAFARI_USER_AGENT));
                self.web.borrow_mut().apply(BrowserRequest::Reload(tab));
            }
            CompatChange::Disable => {
                self.compat_tabs.remove(&tab);
                self.web.borrow().set_user_agent(tab, None);
            }
        }
    }

    /// Palette: send a Safari user agent on this site (for sites that reject
    /// WebKit's default one), or stop doing so, and reload.
    fn toggle_compat_mode(&mut self) {
        let Some(tab) = self.active() else { return };
        let Some(host) = self.hub.lock().tab(tab).filter(|t| is_http_url(&t.url)).map(|t| crate::clean_url::host_of(&t.url)) else {
            self.toast("Compatibility mode applies to web pages only");
            return;
        };
        let Some(store) = &self.store else {
            self.toast("Site settings are unavailable: the profile could not be opened");
            return;
        };
        let mut prefs = store.site_pref(&host).unwrap_or_default();
        prefs.compat_mode = !prefs.compat_mode;
        let on = prefs.compat_mode;
        if let Err(e) = store.set_site_pref(&host, prefs) {
            self.toast(format!("Could not save the site setting: {e}"));
            return;
        }
        if on {
            self.compat_tabs.insert(tab);
        } else {
            self.compat_tabs.remove(&tab);
        }
        self.web.borrow().set_user_agent(tab, on.then_some(SAFARI_USER_AGENT));
        self.web.borrow_mut().apply(BrowserRequest::Reload(tab));
        self.toast(if on { format!("Compatibility mode on for {host}") } else { format!("Compatibility mode off for {host}") });
    }

    /// ⌘= / ⌘- / ⌘0: step the active page's zoom and remember it per host.
    fn zoom(&mut self, direction: i8) {
        let Some(tab) = self.active() else { return };
        let Some(host) = self.hub.lock().tab(tab).map(|t| crate::clean_url::host_of(&t.url)).filter(|h| !h.is_empty()) else {
            return;
        };
        let mut prefs = self.store.as_ref().and_then(|s| s.site_pref(&host).ok()).unwrap_or_default();
        prefs.zoom = zoom_step(prefs.zoom, direction);
        let zoom = prefs.zoom;
        self.web.borrow().set_zoom(tab, zoom);
        if let Some(Err(e)) = self.store.as_ref().map(|s| s.set_site_pref(&host, prefs)) {
            eprintln!("relay: could not save zoom for {host}: {e}");
        }
        self.toast(format!("Zoom {}% on {host}", (zoom * 100.0).round()));
    }

    /// Palette: turn ad/tracker blocking off (or back on) for the active
    /// page's site, recompile the rule list, and reload the page.
    fn toggle_site_blocking(&mut self) {
        let Some(tab) = self.active() else { return };
        let Some(host) = self.hub.lock().tab(tab).filter(|t| is_http_url(&t.url)).map(|t| crate::clean_url::host_of(&t.url)) else {
            self.toast("Blocking applies to web pages only");
            return;
        };
        if !relay_webkit::content_rules::is_site_host(&host) {
            self.toast("Per-site blocking needs a domain name or IPv4 address, not an IPv6 literal");
            return;
        }
        let Some(store) = &self.store else {
            self.toast("Site settings are unavailable: the profile could not be opened");
            return;
        };
        let mut prefs = store.site_pref(&host).unwrap_or_default();
        prefs.blocking_off = !prefs.blocking_off;
        let off = prefs.blocking_off;
        if let Err(e) = store.set_site_pref(&host, prefs) {
            self.toast(format!("Could not save the site setting: {e}"));
            return;
        }
        if self.blocking {
            // Reload only once WebKit has applied the recompiled list.
            self.web.borrow().load_content_rules_and_reload(&rules_json(self.store.as_ref()), tab);
        }
        self.toast(match (off, self.blocking) {
            (true, _) => format!("Blocking off on {host}"),
            (false, true) => format!("Blocking on for {host}"),
            (false, false) => format!("{host} will be blocked once blocking is turned on in Settings"),
        });
    }

    /// ⌘S: fold the tab strip to a rail (or unfold it). The rail stays
    /// clickable so the strip is never lost.
    fn toggle_tab_strip(&mut self, cx: &mut Context<Self>) {
        self.strip = self.strip.toggled();
        self.persist_strip();
        cx.notify();
    }

    /// ⇧⌘S: left → top → right, like Search.
    fn rotate_tab_strip(&mut self, cx: &mut Context<Self>) {
        self.strip = self.strip.rotated();
        self.persist_strip();
        self.toast(format!("Tab strip: {}", self.strip.placement.label()));
        cx.notify();
    }

    fn persist_strip(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("tabs_position", self.strip.position_setting());
            let _ = store.set_setting("tabs_folded", self.strip.folded_setting());
        }
    }

    /// Ask the page for its manifest, then write a small `.app` bundle.
    fn install_web_app(&mut self, cx: &mut Context<Self>) {
        if !cfg!(target_os = "macos") {
            self.toast("Installing web apps is available in the macOS build");
            return;
        }
        if self.app_mode {
            self.toast("Install apps from the browser window, not an app window");
            return;
        }
        let Some(tab) = self.active() else {
            self.toast("Open a web page first");
            return;
        };
        let Some(page_url) = self.hub.lock().tab(tab).filter(|t| is_http_url(&t.url)).map(|t| t.url.clone()) else {
            self.toast("Only http(s) pages can be installed as apps");
            return;
        };
        let (script, rx) = match self.hub.begin_command(tab, Op::WebApp) {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Could not read the page: {e}"));
                return;
            }
        };
        self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
        self.toast("Reading the web app manifest…");
        cx.spawn(async move |this, cx| {
            let reply = futures_timeout(rx, Duration::from_secs(8), cx).await;
            let _ = this.update(cx, |app, cx| {
                app.finish_install(reply, page_url);
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_install(&mut self, reply: Option<serde_json::Value>, page_url: String) {
        let Some(value) = reply else {
            self.toast("This page did not answer the app check");
            return;
        };
        let manifest = match webapp::manifest_from_result(&value, &page_url) {
            Ok(m) => m,
            Err(reason) => {
                self.toast(reason);
                return;
            }
        };
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.toast(format!("Could not find the Relay binary: {e}"));
                return;
            }
        };
        let apps = webapp::default_apps_dir();
        match webapp::install(&apps, &manifest, &exe) {
            Ok(app) => {
                if let Some(store) = &self.store {
                    let record = WebAppRecord {
                        id: app.bundle_id,
                        name: manifest.name.clone(),
                        start_url: app.start_url,
                        bundle_path: app.path.display().to_string(),
                        installed_at: now_unix(),
                    };
                    if let Err(e) = store.upsert_web_app(&record) {
                        eprintln!("relay: installed the app but could not record it: {e}");
                    }
                }
                self.refresh_start_page();
                self.toast(format!("Installed “{}” in ~/Applications", app.name));
            }
            Err(e) => self.toast(format!("Could not install the app: {e}")),
        }
    }

    /// Two clicks within six seconds remove a Relay-created bundle; the
    /// bundle's marker is checked again before anything is deleted.
    fn remove_web_app(&mut self, cx: &mut Context<Self>) {
        if self.app_mode {
            self.toast("Remove apps from the browser window");
            return;
        }
        let Some(page_url) = self.active().and_then(|t| self.hub.lock().tab(t).map(|s| s.url.clone())) else {
            self.toast("Open the installed app's site first");
            return;
        };
        let apps = self.store.as_ref().and_then(|s| s.web_apps().ok()).unwrap_or_default();
        let record = match webapp::select_app_for_url(&apps, &page_url) {
            Ok(record) => record,
            Err(reason) => {
                self.toast(reason);
                return;
            }
        };
        let now = Instant::now();
        let confirmed = matches!(&self.pending_remove_app, Some((url, at)) if url == &page_url && now.duration_since(*at) < Duration::from_secs(6));
        if !confirmed {
            self.pending_remove_app = Some((page_url, now));
            self.toast(format!("Click again to remove “{}”", record.name));
            return;
        }
        self.pending_remove_app = None;
        match webapp::remove_bundle(Path::new(&record.bundle_path), &record.start_url) {
            Ok(()) => {
                if let Some(store) = &self.store {
                    let _ = store.remove_web_app(&record.start_url);
                }
                self.refresh_start_page();
                self.toast(format!("Removed “{}”", record.name));
            }
            Err(e) => self.toast(format!("Could not remove the app: {e}")),
        }
        cx.notify();
    }

    /// Router for the Edit menu / ⌘X⌘C⌘V⌘A: palette and find bar take text,
    /// the page keeps its native editing behaviour.
    fn apply_edit(&mut self, action: EditAction, cx: &mut Context<Self>) {
        // With an overlay focused, the page is not the editing target. Paste
        // is the only command the overlay implements; the rest are no-ops so
        // ⌘Z cannot silently change the page behind the palette.
        if self.palette.is_some() {
            let clipboard = || cx.read_from_clipboard().and_then(|c| c.text()).and_then(|t| t.lines().next().map(str::to_string));
            let Some(p) = self.palette.as_mut() else { return };
            let mut changed = false;
            match action {
                EditAction::Paste => {
                    if let Some(text) = clipboard() {
                        p.insert(&text);
                        changed = true;
                    }
                }
                EditAction::Copy | EditAction::Cut => {
                    let text = p.selected_text().or_else(|| (!p.query.is_empty()).then(|| p.query.clone()));
                    if let Some(text) = text {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    if action == EditAction::Cut {
                        p.cut();
                        changed = true;
                    }
                }
                EditAction::SelectAll => p.select_all(),
                EditAction::Undo | EditAction::Redo => {}
            }
            if changed {
                p.refresh(&self.sources, &self.search);
            }
            cx.notify();
            return;
        }
        if let Some(f) = self.find.as_mut() {
            if action != EditAction::Paste {
                return;
            }
            let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()).and_then(|t| t.lines().next().map(str::to_string)) else {
                return;
            };
            f.query.push_str(&text);
            self.run_find(false);
            cx.notify();
            return;
        }
        self.web.borrow().edit(action);
    }

    fn cycle_agent_policy(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.active().and_then(|t| self.hub.lock().tab(t).map(|s| s.url.clone())) else {
            self.toast("Open a site first");
            return;
        };
        let Some(host) = relay_bridge::host_of(&url) else {
            self.toast("Site policies apply to http(s) pages");
            return;
        };
        let next = self.hub.agent_policy_for_host(&host).next();
        self.hub.set_agent_policy(&host, next);
        if let Some(store) = &self.store {
            if let Err(e) = store.set_agent_policy(&host, next) {
                eprintln!("relay: could not save the agent policy: {e}");
            }
        }
        self.toast(format!("Agents on {host}: {}. Each MCP call is checked in the hub.", next.label()));
        cx.notify();
    }

    fn toggle_picture_in_picture(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else {
            self.toast("Open a video first");
            return;
        };
        self.eval_op(tab, Op::PictureInPicture);
        self.toast("Picture-in-picture requested");
        cx.notify();
    }

    fn toggle_reader(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else {
            self.toast("Open an article first");
            return;
        };
        let on = !self.reader_tabs.contains(&tab);
        if let Ok((script, _rx)) = self.hub.begin_command(tab, Op::Reader { on }) {
            self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
        }
        if on {
            self.reader_tabs.insert(tab);
        } else {
            self.reader_tabs.remove(&tab);
        }
        self.toast(if on { "Reading mode on" } else { "Reading mode off" });
        cx.notify();
    }

    fn toggle_agents(&mut self, cx: &mut Context<Self>) {
        let paused = !self.hub.agents_paused();
        self.hub.set_agents_paused(paused);
        self.toast(if paused { "Agents paused; page automation is stopped" } else { "Agents resumed" });
        cx.notify();
    }

    fn open_help(&mut self) {
        let url = "https://github.com/TomSzenessy/RelayBrowser#readme".to_string();
        if self.app_mode {
            // A web app window is not a browser: hand the docs to the user's
            // default browser instead of replacing the app with GitHub.
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(&url).spawn();
            }
            return;
        }
        self.open_url(url, true);
    }

    fn open_library(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let Some(store) = &self.store else {
            self.toast("Profile store unavailable");
            return;
        };
        match Library::open(store, mode) {
            Ok(lib) => {
                self.palette = None;
                self.settings_open = false;
                self.connect_open = false;
                self.downloads_open = false;
                self.find = None;
                self.end_hints();
                self.library = Some(lib);
                self.web.borrow().focus_shell();
            }
            Err(e) => self.toast(format!("Could not open {}: {e}", mode.title())),
        }
        cx.notify();
    }

    fn close_library(&mut self) {
        self.library = None;
        self.focus_active_page();
    }

    fn refresh_library(&mut self) {
        let Some(store) = &self.store else { return };
        if let Some(lib) = self.library.as_mut() {
            if let Err(e) = lib.refresh(store) {
                eprintln!("relay: could not refresh the library: {e}");
            }
        }
    }

    fn open_library_row(&mut self, index: usize) {
        let url = self.library.as_ref().and_then(|l| l.rows.get(index)).map(|r| r.url.clone());
        self.library = None;
        if let Some(url) = url {
            self.open_url(url, false);
        }
    }

    fn delete_library_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let result = {
            let Some(lib) = self.library.as_mut() else { return };
            let Some(row) = lib.rows.get(index).cloned() else { return };
            let result = match lib.mode {
                Mode::History => store.delete_history(&row.url),
                Mode::Bookmarks => store.remove_bookmark(&row.url),
            };
            let _ = lib.refresh(store);
            result
        };
        match result {
            Ok(true) => {}
            Ok(false) => self.toast("Already gone"),
            Err(e) => self.toast(format!("Could not delete: {e}")),
        }
        cx.notify();
    }

    fn save_library_folder(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let Some(lib) = self.library.as_mut() else { return };
        let Some(input) = lib.folder_input.clone() else { return };
        let Some(row) = lib.chosen().cloned() else { return };
        let folder: Vec<String> = input.split('/').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        match store.move_bookmark(&row.url, &folder) {
            Ok(true) => {
                lib.folder_input = None;
                let _ = lib.refresh(store);
            }
            Ok(false) => self.toast("That bookmark is gone"),
            Err(e) => self.toast(format!("Could not move the bookmark: {e}")),
        }
        cx.notify();
    }

    fn bookmark_current_in_library(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else {
            self.toast("Open a page first");
            return;
        };
        let (url, title) = {
            let h = self.hub.lock();
            let Some(t) = h.tab(tab) else { return };
            (t.url.clone(), t.title.clone())
        };
        if !url.starts_with("http") {
            self.toast("Only web pages can be bookmarked");
            return;
        }
        let Some(store) = &self.store else { return };
        match store.toggle_bookmark(&url, &title) {
            Ok(true) => self.toast("Bookmarked"),
            Ok(false) => self.toast("Bookmark removed"),
            Err(e) => self.toast(format!("Could not save the bookmark: {e}")),
        }
        self.refresh_library();
        cx.notify();
    }

    fn open_find(&mut self) {
        let Some(tab) = self.active() else { return };
        self.palette = None;
        self.library = None;
        self.connect_open = false;
        self.end_hints();
        let query = self.find.take().map(|f| f.query).unwrap_or_default();
        self.find = Some(FindBar { tab, query, found: None });
        self.web.borrow().focus_shell();
    }

    fn close_find(&mut self) {
        if let Some(f) = self.find.take() {
            self.focus_pending = Some(f.tab);
            self.try_focus_pending();
        }
    }

    fn run_find(&mut self, backwards: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.query.is_empty() {
            f.found = None;
            return;
        }
        self.web.borrow().find(f.tab, &f.query, backwards);
    }

    /// ⌘⌥P: pin or unpin the active tab (pinned tabs sit at the top).
    fn toggle_pin(&mut self) {
        let Some(tab) = self.active() else { return };
        let now = self.hub.update(|h| {
            let pinned = h.tab(tab).is_some_and(|t| t.pinned);
            h.set_pinned(tab, !pinned)
        });
        match now {
            Some(true) => self.toast("Pinned tab"),
            Some(false) => self.toast("Unpinned tab"),
            None => {}
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
        self.library = None;
        self.connect_open = false;
        self.downloads_open = false;
        self.settings_open = false;
        self.find = None;
        self.confirm_clear_history = false;
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
        let mut p = Palette { query: prefill.unwrap_or_default(), custom_bangs: self.custom_bangs.clone(), ..Default::default() };
        p.cursor = p.query.len();
        p.refresh(&self.sources, &self.search);
        self.palette = Some(p);
        self.web.borrow().focus_shell();
    }

    fn close_palette(&mut self) {
        self.palette = None;
        self.focus_active_page();
    }

    fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.palette = None;
            self.library = None;
            self.connect_open = false;
            self.downloads_open = false;
            self.confirm_clear_history = false;
            self.settings_open = true;
            self.web.borrow().focus_shell();
        }
    }

    fn close_settings(&mut self) {
        self.settings_open = false;
        self.confirm_clear_history = false;
        self.focus_active_page();
    }

    fn run_item(&mut self, item: Item, cx: &mut Context<Self>) {
        let new_tab = self.palette_new_tab || self.active().is_none();
        match item {
            Item::Open(url) | Item::Search { url, .. } | Item::Bang { url, .. } | Item::History { url, .. } | Item::Bookmark { url, .. } => {
                self.open_url(url, new_tab);
            }
            Item::Calc(result) => {
                cx.write_to_clipboard(ClipboardItem::new_string(result.clone()));
                self.toast(format!("Copied: {result}"));
            }
            Item::Tab { id, .. } => self.activate(id),
            Item::Command(c) => self.run_command(c, cx),
        }
    }

    fn run_command(&mut self, c: Command, cx: &mut Context<Self>) {
        let active = self.active();
        match c {
            Command::NewTab => {
                if self.app_mode {
                    // A web app has one page; ⌘T is "go somewhere" here.
                    self.open_palette(None, false);
                } else {
                    self.open_palette(None, true);
                }
            }
            Command::CloseTab => self.close_active(cx),
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
            Command::Settings => self.toggle_settings(),
            Command::ImportChrome => self.import_chrome(cx),
            Command::CopyMcpSetup => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.mcp_setup.clone()));
                self.toast("Copied: claude mcp add relay -- relay mcp");
            }
            Command::CopyCleanUrl => {
                let url = active.and_then(|t| self.hub.lock().tab(t).map(|s| s.url.clone())).unwrap_or_default();
                if url.is_empty() {
                    self.toast("No active URL to copy");
                } else {
                    let cleaned = crate::clean_url::clean(&url);
                    cx.write_to_clipboard(ClipboardItem::new_string(cleaned));
                    self.toast("Copied clean URL");
                }
            }
            Command::CopyMarkdown => self.copy_markdown(cx),
            Command::ToggleBookmark => self.toggle_bookmark(),
            Command::TogglePin => self.toggle_pin(),
            Command::Find => self.open_find(),
            Command::ZoomIn => self.zoom(1),
            Command::ZoomOut => self.zoom(-1),
            Command::ZoomReset => self.zoom(0),
            Command::ToggleSiteBlocking => self.toggle_site_blocking(),
            Command::ToggleCompatMode => self.toggle_compat_mode(),
            Command::CloseDuplicateTabs => self.close_duplicate_tabs(),
            Command::ToggleTabStrip => self.toggle_tab_strip(cx),
            Command::NextTabPlacement => self.rotate_tab_strip(cx),
            Command::InstallWebApp => self.install_web_app(cx),
            Command::RemoveWebApp => self.remove_web_app(cx),
            Command::ShowHistory => self.open_library(Mode::History, cx),
            Command::ShowBookmarks => self.open_library(Mode::Bookmarks, cx),
            Command::ToggleAgents => self.toggle_agents(cx),
            Command::CycleAgentPolicy => self.cycle_agent_policy(cx),
            Command::CycleTheme => self.cycle_theme(cx),
            Command::CycleAccent => self.cycle_accent(cx),
            Command::ToggleReader => self.toggle_reader(cx),
            Command::TogglePictureInPicture => self.toggle_picture_in_picture(cx),
            Command::ShowDownloads => self.open_downloads(cx),
        }
    }

    /// Ask the active page for its main content as Markdown (`Op::Markdown`,
    /// read-only, capped at 200 KB by the bridge) and copy it.
    fn copy_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else {
            self.toast("No page to copy");
            return;
        };
        let (script, rx) = match self.hub.begin_command(tab, Op::Markdown) {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Could not read the page: {e}"));
                return;
            }
        };
        self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
        cx.spawn(async move |this, cx| {
            let reply = futures_timeout(rx, Duration::from_secs(3), cx).await;
            let _ = this.update(cx, |app, cx| {
                match reply.as_ref().and_then(|v| v.as_str()).filter(|t| !t.trim().is_empty()) {
                    Some(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                        app.toast(format!("Copied page as Markdown ({})", byte_size(text.len())));
                    }
                    None => app.toast("This page has no content to copy as Markdown"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⌘D: bookmark the active http(s) page, or remove it if already saved.
    fn toggle_bookmark(&mut self) {
        let page = self.active().and_then(|t| self.hub.lock().tab(t).map(|s| (s.url.clone(), s.title.clone())));
        let Some((url, title)) = page.filter(|(u, _)| is_http_url(u)) else {
            self.toast("Only web pages can be bookmarked");
            return;
        };
        let title = if title.trim().is_empty() { palette::short_url(&url) } else { title };
        let msg = match self.store.as_ref().map(|s| s.toggle_bookmark(&url, &title)) {
            None => "Bookmarks are unavailable: the profile could not be opened".to_string(),
            Some(Ok(true)) => format!("Bookmarked {title}"),
            Some(Ok(false)) => "Bookmark removed".to_string(),
            Some(Err(e)) => format!("Could not update bookmarks: {e}"),
        };
        self.refresh_start_page();
        self.toast(msg);
    }

    fn close_duplicate_tabs(&mut self) {
        let active = self.active();
        let to_close: Vec<TabId> = {
            let h = self.hub.lock();
            let mut seen = std::collections::HashSet::new();
            let mut dups = Vec::new();
            for &id in h.tab_ids() {
                let Some(tab) = h.tab(id) else { continue };
                if tab.url.is_empty() {
                    continue;
                }
                if !seen.insert(tab.url.clone()) && tab.owner == Owner::User && Some(id) != active {
                    dups.push(id);
                }
            }
            dups
        };
        let count = to_close.len();
        for id in to_close {
            if self.split == Some(id) {
                self.split = None;
            }
            self.snapshots.remove(&id);
            self.compat_tabs.remove(&id);
            self.web.borrow_mut().apply(BrowserRequest::Close(id));
            self.hub.update(|h| h.close_tab(id));
        }
        if count == 0 {
            self.toast("No duplicate tabs");
        } else {
            self.toast(format!("Closed {count} duplicate tab{}", if count == 1 { "" } else { "s" }));
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
        let imported = store.import(&preview.bookmarks, &preview.history);
        match imported {
            Ok(c) => {
                self.refresh_start_page();
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
        if self.shortcuts.is_some() {
            let recording = self.shortcuts.as_ref().and_then(|s| s.recording);
            if let Some(id) = recording {
                if k.key == "escape" {
                    if let Some(editor) = self.shortcuts.as_mut() {
                        editor.recording = None;
                    }
                    self.apply_keymap(cx);
                } else if let Some(key) = keymap::from_keystroke(k) {
                    self.finish_recording(id, &key, cx);
                }
            } else if k.key == "escape" {
                self.close_shortcuts();
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.connect_open {
            if k.key == "escape" {
                self.close_connect();
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.downloads_open {
            let len = self.hub.lock().downloads().len();
            match k.key.as_str() {
                "escape" => self.close_downloads(),
                "up" => self.downloads_selected = self.downloads_selected.saturating_sub(1),
                "down" => self.downloads_selected = self.downloads_selected.saturating_add(1).min(len.saturating_sub(1)),
                "home" => self.downloads_selected = 0,
                "end" => self.downloads_selected = len.saturating_sub(1),
                "enter" => self.activate_selected_download(cx),
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.library.as_ref().is_some_and(|l| l.folder_input.is_some()) {
            match k.key.as_str() {
                "escape" => {
                    if let Some(lib) = self.library.as_mut() {
                        lib.folder_input = None;
                    }
                }
                "enter" => self.save_library_folder(cx),
                "backspace" => {
                    if let Some(lib) = self.library.as_mut() {
                        if let Some(input) = lib.folder_input.as_mut() {
                            input.pop();
                        }
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(lib) = self.library.as_mut() {
                            if let Some(input) = lib.folder_input.as_mut() {
                                input.push_str(c);
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.library.is_some() {
            match k.key.as_str() {
                "escape" => self.close_library(),
                "enter" => {
                    let selected = self.library.as_ref().map(|l| l.selected).unwrap_or(0);
                    self.open_library_row(selected);
                }
                "up" => {
                    if let Some(lib) = self.library.as_mut() {
                        lib.move_selection(-1);
                    }
                }
                "down" | "tab" => {
                    if let Some(lib) = self.library.as_mut() {
                        lib.move_selection(1);
                    }
                }
                "backspace" => {
                    if let Some(lib) = self.library.as_mut() {
                        if k.modifiers.secondary() || k.modifiers.alt {
                            lib.query.clear();
                        } else {
                            lib.query.pop();
                        }
                    }
                    self.refresh_library();
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(lib) = self.library.as_mut() {
                            lib.query.push_str(c);
                        }
                        self.refresh_library();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
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
        if self.settings_open {
            if k.key.as_str() == "escape" {
                self.close_settings();
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if let Some(f) = self.find.as_mut() {
            match k.key.as_str() {
                "escape" => self.close_find(),
                "enter" => {
                    let back = k.modifiers.shift;
                    self.run_find(back);
                }
                "backspace" => {
                    f.query.pop();
                    self.run_find(false);
                }
                _ => match k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                    Some(c) => {
                        f.query.push_str(c);
                        self.run_find(false);
                    }
                    None => return, // ⌘ shortcuts keep working while the bar is open
                },
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.palette.is_none() && self.active().is_none() {
            if let Some(c) = k
                .key_char
                .as_deref()
                .filter(|c| !c.is_empty() && c.chars().all(|ch| !ch.is_control()) && !k.modifiers.secondary() && !k.modifiers.control)
            {
                self.open_palette(Some(c.to_string()), true);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let Some(p) = self.palette.as_mut() else { return };
        match k.key.as_str() {
            "escape" => self.close_palette(),
            "enter" => {
                let item = p.chosen().cloned();
                self.close_palette();
                if let Some(item) = item {
                    self.run_item(item, cx);
                }
            }
            "up" => p.move_selection(-1),
            "down" | "tab" => p.move_selection(1),
            "left" => p.move_cursor_ext(-1, k.modifiers.shift, k.modifiers.alt),
            "right" => p.move_cursor_ext(1, k.modifiers.shift, k.modifiers.alt),
            "home" => p.set_cursor(0, k.modifiers.shift),
            "end" => p.set_cursor(p.query.len(), k.modifiers.shift),
            "delete" => {
                p.delete();
                p.refresh(&self.sources, &self.search);
            }
            "backspace" => {
                if k.modifiers.secondary() || k.modifiers.alt {
                    p.clear_query();
                } else {
                    p.backspace();
                }
                p.refresh(&self.sources, &self.search);
            }
            _ => {
                if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                    p.insert(c);
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

    /// The tab strip: vertical on the left or right, horizontal across the
    /// top, or folded to a rail. Every row carries its own close button.
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip = self.strip;
        if strip.folded {
            return self.render_folded_strip(cx);
        }
        let (active, user, agents) = {
            let h = self.hub.lock();
            let active = h.active();
            let mut user = vec![];
            let mut agents = vec![];
            for view in h.tab_views() {
                if view.owner == Owner::Agent { agents.push(view) } else { user.push(view) }
            }
            (active, user, agents)
        };
        let vertical = strip.placement.is_vertical();
        let mut bar = div().flex();
        if vertical {
            bar = bar
                .flex_col()
                .gap_0p5()
                .p_2()
                .w(px(strip.width()))
                .h_full()
                .bg(t.surface)
                .border_color(t.border);
            bar = if strip.placement == tabstrip::Placement::Left { bar.border_r_1() } else { bar.border_l_1() };
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .pb_2()
                    .child(div().text_size(px(11.)).text_color(t.muted).child("TABS"))
                    .child(self.fold_button(if strip.placement == tabstrip::Placement::Left { "«" } else { "»" }, cx)),
            );
            for tab in &user {
                bar = bar.child(self.tab_row(tab, active, true, cx));
            }
            if !agents.is_empty() {
                bar = bar.child(div().mt_3().px_2().pb_1().text_size(px(11.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    bar = bar.child(self.tab_row(tab, active, true, cx));
                }
            }
            bar.child(div().flex_1()).child(self.new_tab_button(true, cx)).into_any_element()
        } else {
            bar = bar
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(strip.height()))
                .w_full()
                .bg(t.surface)
                .border_b_1()
                .border_color(t.border)
                .child(div().text_size(px(11.)).text_color(t.muted).child("TABS"));
            for tab in &user {
                bar = bar.child(self.tab_row(tab, active, false, cx));
            }
            if !agents.is_empty() {
                bar = bar.child(div().px_1().text_size(px(10.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    bar = bar.child(self.tab_row(tab, active, false, cx));
                }
            }
            bar.child(self.new_tab_button(false, cx))
                .child(div().flex_1())
                .child(self.fold_button("⌃", cx))
                .into_any_element()
        }
    }

    fn tab_row(&self, tab: &TabView, active: Option<TabId>, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let id = tab.id;
        let is_active = active == Some(id);
        let agent = tab.owner == Owner::Agent;
        let title: SharedString = if tab.title.is_empty() { palette::short_url(&tab.url).into() } else { tab.title.clone().into() };
        let show_close = is_active || self.hovered_tab == Some(id);
        let mut row = div()
            .id(("tab", id as usize))
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.))
            .px_2()
            .rounded(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .text_color(if tab.sleeping { t.muted } else { t.text })
            .hover(|s| s.bg(t.hover))
            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                let next = hovered.then_some(id);
                if app.hovered_tab != next {
                    app.hovered_tab = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |app, _, _, cx| {
                app.activate(id);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |app, _, _, cx| {
                app.close_tab(id, cx);
                cx.notify();
            }));
        let icon = host_of(&tab.url).and_then(|host| self.favicons.get(&host)).and_then(|icon| icon.clone());
        row = row.child(match icon {
            Some(icon) => img(icon).size(px(14.)).rounded(px(3.)).into_any_element(),
            None => div()
                .size(px(8.))
                .rounded_full()
                .bg(if agent { t.agent } else if is_active { t.accent } else { t.border })
                .into_any_element(),
        });
        let title_el = if vertical {
            div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
        } else {
            div().max_w(px(170.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
        };
        row = row.child(title_el);
        if agent {
            row = row.bg(t.agent_bg);
        }
        if is_active {
            row = row.bg(t.active);
        }
        if tab.challenge.is_some() {
            row = row.child(div().text_color(t.warn).child("⚠"));
        }
        if tab.playing_audio {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("♪"));
        }
        if tab.pinned {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("pin"));
        }
        if tab.sleeping {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("zz"));
        }
        row.child(
            div()
                .id(("close-tab", id as usize))
                .w(px(18.))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(14.))
                .line_height(px(14.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .when(!show_close, |d| d.invisible())
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.close_tab(id, cx);
                    cx.notify();
                }))
                .child("×"),
        )
        .into_any_element()
    }

    fn new_tab_button(&self, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let button = div()
            .id("new-tab")
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
            }));
        if vertical {
            button.px_2().h(px(28.)).child("+ New tab   ⌘T").into_any_element()
        } else {
            button.w(px(26.)).h(px(26.)).justify_center().child("+").into_any_element()
        }
    }

    fn fold_button(&self, label: &'static str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        div()
            .id("fold-tab-strip")
            .w(px(20.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(12.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .child(label)
            .into_any_element()
    }

    /// The folded strip is a thin rail: visible, clickable, and never in the
    /// way. Clicking anywhere on it brings the tabs back.
    fn render_folded_strip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip = self.strip;
        let (arrow, vertical) = match strip.placement {
            tabstrip::Placement::Left => ("›", true),
            tabstrip::Placement::Right => ("‹", true),
            tabstrip::Placement::Top => ("⌄", false),
        };
        let rail = div()
            .id("unfold-tab-strip")
            .bg(t.surface)
            .border_color(t.border)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .child(div().text_size(px(if vertical { 11. } else { 10. })).text_color(t.muted).child(arrow));
        if vertical {
            let rail = rail.w(px(strip.width())).h_full();
            match strip.placement {
                tabstrip::Placement::Left => rail.border_r_1(),
                _ => rail.border_l_1(),
            }
            .into_any_element()
        } else {
            rail.w_full().h(px(strip.height())).border_b_1().into_any_element()
        }
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
        if let Some(snap) = self.snapshots.get(&tab).filter(|_| !has_dom && load_error.is_none()) {
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
                            .id(("retry-page", tab as usize))
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
            return self.render_start_page(cx);
        };
        let mut row = div().flex_1().h_full().flex();
        row = row.child(self.render_pane(active, cx));
        if let Some(s) = self.split.filter(|s| *s != active && self.hub.lock().tab(*s).is_some()) {
            row = row.child(div().w(px(1.)).h_full().bg(t.border)).child(self.render_pane(s, cx));
        }
        row.into_any_element()
    }

    /// The new-tab page: a quiet start field and the sections of `startpage`.
    /// A brand-new profile falls back to the welcome card inside the page.
    fn render_start_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let title = self.app_name.clone().unwrap_or_else(|| "Relay".to_string());
        let page = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(56.))
            .px_6()
            .bg(t.page)
            .overflow_hidden()
            .child(div().text_size(px(28.)).text_color(t.text).child(SharedString::from(title)))
            .child(self.start_search(cx));
        let body = if self.start_page.is_empty() { self.render_welcome(cx).into_any_element() } else { self.start_grid(cx) };
        page.child(div().id("start-body").flex_1().w_full().flex().justify_center().overflow_y_scroll().child(body)).into_any_element()
    }

    fn start_search(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        div()
            .id("start-search")
            .w(px(560.))
            .h(px(44.))
            .mt_5()
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(10.))
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .text_size(px(14.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_palette(None, false);
                cx.notify();
            }))
            .child("Search or enter an address")
            .child(div().text_size(px(11.)).child("⌘K"))
            .into_any_element()
    }

    fn start_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut left = div().flex().flex_col().gap_6().flex_1();
        let mut right = div().flex().flex_col().gap_6().flex_1();
        for (i, (section, items)) in self.start_page.sections.iter().enumerate() {
            let card = self.start_section(*section, items, cx);
            if i % 2 == 0 {
                left = left.child(card);
            } else {
                right = right.child(card);
            }
        }
        div().flex().gap_6().mt_8().w(px(680.)).child(left).child(right).into_any_element()
    }

    fn start_section(&self, section: Section, items: &[StartItem], cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_2().text_size(px(11.)).text_color(t.muted).child(section.title().to_uppercase()));
        if items.is_empty() {
            col = col.child(div().px_2().py_1().text_size(px(12.)).text_color(t.muted).child("Nothing yet"));
        }
        for (i, item) in items.iter().enumerate() {
            let url = item.url.clone();
            col = col.child(
                div()
                    .id(SharedString::from(format!("start-{}-{i}", section.as_str())))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_url(url.clone(), false);
                        cx.notify();
                    }))
                    .child(match host_of(&item.url).and_then(|host| self.favicons.get(&host)).and_then(|icon| icon.clone()) {
                        Some(icon) => img(icon).size(px(14.)).rounded(px(3.)).into_any_element(),
                        None => div().size(px(14.)).into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(t.text)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(item.title.clone())),
                    )
                    .child(
                        div()
                            .max_w(px(150.))
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(item.note.clone())),
                    ),
            );
        }
        col.into_any_element()
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
        div()
            .w(px(560.))
            .mt_8()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .rounded(px(10.))
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .child(div().text_size(px(15.)).text_color(t.text).child("A browser that agents can read"))
            .child(div().text_size(px(13.)).text_color(t.muted).child("Import your Chrome bookmarks and history, or connect Claude Code, Codex, OpenCode, or Zed to the live page."))
            .child(div().flex().gap_2().child(button("import", "Import from Chrome", Command::ImportChrome, cx)).child(button("mcp", "Copy MCP setup", Command::CopyMcpSetup, cx)))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .font_family("Menlo")
                    .child(SharedString::from(format!("Agents: {}\nSocket: {}", self.mcp_setup, self.mcp_socket))),
            )
    }

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut head = div().flex().items_center().gap_1().px_2().h(px(32.)).border_b_1().border_color(t.border);
        for (tab, label) in [(SidebarTab::Agent, "Agent"), (SidebarTab::Inspector, "Inspector")] {
            let on = self.sidebar_tab == tab;
            head = head.child(
                div()
                    .id(if tab == SidebarTab::Agent { "sidebar-agent" } else { "sidebar-inspector" })
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.sidebar_tab = tab;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        head = head.child(div().flex_1()).child(
            div()
                .id("sidebar-collapse")
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(14.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.inspector_open = false;
                    app.focus_active_page();
                    cx.notify();
                }))
                .child("×"),
        );
        let body = match self.sidebar_tab {
            SidebarTab::Agent => self.render_agent_pane(cx),
            SidebarTab::Inspector => self.render_inspector_pane(cx),
        };
        div()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.border)
            .font_family("Menlo")
            .child(head)
            .child(div().id("sidebar-body").flex_1().overflow_y_scroll().text_size(px(12.)).child(body))
    }

    fn render_agent_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let (host, policy) = self
            .active()
            .and_then(|tab| self.hub.lock().tab(tab).map(|t| t.url.clone()))
            .and_then(|url| relay_bridge::host_of(&url))
            .map(|host| {
                let policy = self.hub.agent_policy_for_host(&host);
                (host, policy)
            })
            .unwrap_or_else(|| ("no site".to_string(), relay_bridge::AgentPolicy::Allow));
        let paused = self.hub.agents_paused();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded(px(5.))
                .border_1()
                .border_color(t.border)
                .text_size(px(11.))
                .text_color(t.text)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        div()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(13.)).text_color(t.text).child("Agent surface"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child(format!("MCP: {}", if self.mcp_enabled { "on (restart to apply)" } else { "off" }))
                    .child(format!("Socket: {}", self.mcp_socket))
                    .child(format!("Status: {}", if paused { "paused" } else { "accepting tool calls" }))
                    .child(format!("eval_js: {}", if self.agent_eval { "on (Allow sites only)" } else { "off" }))
                    .child(format!("shell writes: {}", if self.shell_control { "on" } else { "off" }))
                    .child(format!("This site: {host} · {}", policy.label())),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button(if paused { "agent-resume" } else { "agent-pause" }, if paused { "Resume" } else { "Pause" })
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_agents(cx))),
                    )
                    .child(
                        button("agent-cycle-policy", "Cycle site access")
                            .on_click(cx.listener(|app, _, _, cx| app.cycle_agent_policy(cx))),
                    ),
            )
            .child(
                div()
                    .id("agent-copy-setup")
                    .px_2()
                    .py_1()
                    .rounded(px(5.))
                    .bg(t.background)
                    .border_1()
                    .border_color(t.border)
                    .text_size(px(10.))
                    .text_color(t.muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(app.mcp_setup.clone()));
                        app.toast("Copied the MCP setup command");
                        cx.notify();
                    }))
                    .child(SharedString::from(self.mcp_setup.clone())),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(t.muted)
                    .child("Page content is untrusted data. Every tool call is checked by Stop and the site policy before it touches this tab."),
            )
            .into_any_element()
    }

    fn render_inspector_pane(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
        tabs = tabs.child(div().flex_1());
        if self.panel == Panel::Network {
            let failed_only = self.network_failed_only;
            tabs = tabs.child(
                div()
                    .id("network-filter")
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(if failed_only { t.error } else { t.muted })
                    .when(failed_only, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.network_failed_only = !app.network_failed_only;
                        cx.notify();
                    }))
                    .child(if failed_only { "Failed" } else { "All" }),
            );
        } else if self.panel == Panel::Console {
            let level = self.console_min;
            let active_filter = level != ConsoleLevel::Log;
            tabs = tabs.child(
                div()
                    .id("console-filter")
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(match level {
                        ConsoleLevel::Error => t.error,
                        ConsoleLevel::Warn => t.warn,
                        _ => t.muted,
                    })
                    .when(active_filter, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.console_min = next_console_min(app.console_min);
                        cx.notify();
                    }))
                    .child(console_filter_label(level)),
            );
        }
        let body = self.render_panel_body(cx);
        div()
            .flex()
            .flex_col()
            .h_full()
            .child(tabs)
            .child(div().id("inspector-body").flex_1().overflow_y_scroll().p_1().text_size(px(12.)).child(body))
            .into_any_element()
    }

    fn build_inspector_rows(&self, tab: TabId) -> InspectorRows {
        let h = self.hub.lock();
        let Some(tab) = h.tab(tab) else { return InspectorRows::Empty("No tab") };
        match self.panel {
            Panel::Elements => {
                let rows = self.tree.rows(tab, 1500);
                if rows.is_empty() { InspectorRows::Empty("Waiting for page…") } else { InspectorRows::Elements(rows) }
            }
            Panel::Network => {
                let rows = if self.network_failed_only {
                    inspector::network_rows_filtered(tab, "", true)
                } else {
                    inspector::network_rows(tab, "")
                };
                if rows.is_empty() {
                    let empty = if self.network_failed_only { "No failed requests" } else { "No requests yet" };
                    InspectorRows::Empty(empty)
                } else {
                    InspectorRows::Network(rows.into_iter().take(500).collect())
                }
            }
            Panel::Console => {
                let rows = inspector::console_rows(tab, self.console_min);
                if rows.is_empty() {
                    InspectorRows::Empty("Console is empty")
                } else {
                    InspectorRows::Console(rows.into_iter().rev().take(500).collect())
                }
            }
            Panel::Cookies => {
                if tab.cookies.is_empty() {
                    InspectorRows::Empty("No cookies for this site")
                } else {
                    InspectorRows::Cookies(tab.cookies.clone())
                }
            }
        }
    }

    /// Build the panel rows only when the version, tab, panel, expansion, or
    /// filters changed; painting a frame reuses the cached strings.
    fn inspector_rows(&mut self) -> &InspectorRows {
        let tab = self.active();
        let valid = self.inspector_cache.as_ref().is_some_and(|c| {
            c.version == self.seen_version
                && c.tab == tab.unwrap_or(0)
                && c.panel == self.panel
                && c.tree_epoch == self.tree_epoch
                && c.console_min == self.console_min
                && c.failed_only == self.network_failed_only
        });
        if !valid {
            let rows = match tab {
                Some(tab) => self.build_inspector_rows(tab),
                None => InspectorRows::Empty("No tab"),
            };
            self.inspector_cache = Some(InspectorCache {
                version: self.seen_version,
                tab: tab.unwrap_or(0),
                panel: self.panel,
                tree_epoch: self.tree_epoch,
                console_min: self.console_min,
                failed_only: self.network_failed_only,
                rows,
            });
        }
        &self.inspector_cache.as_ref().expect("cache was just filled").rows
    }

    fn render_panel_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let Some(tab_id) = self.active() else {
            return div().p_2().text_color(t.muted).child("No tab").into_any_element();
        };
        let rows = self.inspector_rows();
        let mut col = div().flex().flex_col();
        match rows {
            InspectorRows::Empty(message) => return div().p_2().text_color(t.muted).child(*message).into_any_element(),
            InspectorRows::Elements(rows) => {
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
                                app.tree_epoch = app.tree_epoch.wrapping_add(1);
                                cx.notify();
                            }))
                            .child(SharedString::from(format!("{marker}{}", r.label))),
                    );
                }
            }
            InspectorRows::Network(rows) => {
                for r in rows {
                    col = col.child(
                        div()
                            .flex()
                            .gap_2()
                            .h(px(18.))
                            .px_1()
                            .text_color(if r.failed { t.error } else { t.text })
                            .child(div().w(px(36.)).child(SharedString::from(r.method.clone())))
                            .child(div().w(px(32.)).child(SharedString::from(r.status.clone())))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(r.url.clone())))
                            .child(div().text_color(t.muted).child(SharedString::from(r.time.clone()))),
                    );
                }
            }
            InspectorRows::Console(rows) => {
                for r in rows {
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
                            .child(SharedString::from(r.text.clone()))
                            .when(!r.source.is_empty(), |d| d.child(div().text_color(t.muted).child(SharedString::from(r.source.clone())))),
                    );
                }
            }
            InspectorRows::Cookies(rows) => {
                for c in rows {
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

    fn render_library(&self, lib: &Library, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().id("library-list").flex().flex_col().py_1().max_h(px(400.)).overflow_y_scroll();
        if lib.rows.is_empty() {
            list = list.child(div().px_4().py_6().text_size(px(13.)).text_color(t.muted).child(lib.mode.empty()));
        }
        for (i, row) in lib.rows.iter().enumerate() {
            let on = i == lib.selected;
            list = list.child(
                div()
                    .id(("lib", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(36.))
                    .px_3()
                    .mx_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_library_row(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .overflow_hidden()
                            .child(div().text_size(px(13.)).text_color(t.text).whitespace_nowrap().text_ellipsis().child(SharedString::from(row.title.clone())))
                            .child(div().text_size(px(11.)).text_color(t.muted).whitespace_nowrap().text_ellipsis().child(SharedString::from(row.note.clone()))),
                    )
                    .when(lib.mode == Mode::Bookmarks, |d| {
                        d.child(
                            div()
                                .id(("lib-move", i))
                                .px_1()
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .text_size(px(10.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    let note = app.library.as_ref().and_then(|l| l.rows.get(i)).map(|r| r.note.clone()).unwrap_or_default();
                                    if let Some(lib) = app.library.as_mut() {
                                        lib.folder_input = Some(if note == "Bookmark" { String::new() } else { note });
                                    }
                                    cx.notify();
                                }))
                                .child("move"),
                        )
                    })
                    .child(
                        div()
                            .id(("lib-del", i))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(14.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.error))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.delete_library_row(i, cx);
                            }))
                            .child("×"),
                    ),
            );
        }
        let title = if lib.query.is_empty() { format!("Search {}", lib.mode.title().to_lowercase()) } else { lib.query.clone() };
        let panel = div()
            .id("library")
            .w(px(680.))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_library()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(40.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().text_size(px(14.)).text_color(t.text).child(lib.mode.title()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(lib.mode == Mode::Bookmarks, |d| {
                                d.child(
                                    div()
                                        .id("lib-add")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(11.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.bookmark_current_in_library(cx);
                                        }))
                                        .child("+ Bookmark this page"),
                                )
                            })
                            .child(div().text_size(px(11.)).text_color(t.muted).child(lib.mode.hint())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(42.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(14.))
                    .text_color(if lib.query.is_empty() { t.muted } else { t.text })
                    .child(SharedString::from(title))
                    .child(div().w(px(1.5)).h(px(18.)).ml_0p5().bg(t.accent)),
            );
        let panel = if let Some(input) = &lib.folder_input {
            panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(38.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(12.))
                    .text_color(t.text)
                    .child("Move to folder:")
                    .child(div().flex_1().child(SharedString::from(if input.is_empty() { "top level".to_string() } else { input.clone() })))
                    .child(div().text_color(t.muted).child("↩ saves · esc cancels")),
            )
        } else {
            panel
        };
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(panel.child(list))
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
                        app.close_palette();
                        app.run_item(item_c.clone(), cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.)).child(SharedString::from(item.title())))
                    .child(div().text_size(px(12.)).text_color(t.muted).child(SharedString::from(item.detail()))),
            );
        }
        let cursor_el = || div().w(px(1.5)).h(px(18.)).bg(t.accent);
        let selection = p.selection();
        let mut query_row = div().flex().items_center().whitespace_nowrap();
        if p.query.is_empty() {
            query_row = query_row.child(cursor_el()).child("Search or enter address…");
        } else {
            for (start, ch) in p.query.char_indices() {
                if p.cursor == start {
                    query_row = query_row.child(cursor_el());
                }
                let end = start + ch.len_utf8();
                let selected = selection.is_some_and(|(s, e)| start >= s && start < e);
                query_row = query_row.child(
                    div()
                        .id(("pal-char", start))
                        .when(selected, |d| d.bg(t.active))
                        .cursor_pointer()
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if let Some(p) = app.palette.as_mut() {
                                p.set_cursor(end, false);
                            }
                            cx.notify();
                        }))
                        .child(SharedString::from(ch.to_string())),
                );
            }
            if p.cursor == p.query.len() {
                query_row = query_row.child(cursor_el());
            }
        }
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
                        .child(query_row),
                )
                .child(list),
        )
    }

    fn apply_shell_setting(&mut self, key: &str, value: &str) {
        match key {
            "search" => self.search = value.to_string(),
            "blocking" => {
                self.blocking = setting_enabled(Some(value));
                if self.blocking {
                    self.web.borrow().load_content_rules(&rules_json(self.store.as_ref()));
                } else {
                    self.web.borrow().clear_content_rules();
                }
            }
            "sleep_minutes" => {
                self.sleep_minutes = value.parse::<u64>().unwrap_or(15).min(1440);
                self.sleep_policy.idle_after = Duration::from_secs(self.sleep_minutes.saturating_mul(60));
            }
            "tabs_position" => {
                if let Some(placement) = tabstrip::Placement::parse(value) {
                    self.strip.placement = placement;
                }
            }
            "tabs_folded" => self.strip.folded = setting_enabled(Some(value)),
            "restore_session" => self.session_enabled = setting_enabled(Some(value)),
            _ => {}
        }
    }

    fn reveal_download(&mut self, path: &str, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = path;
            self.toast("Reveal in Finder needs macOS");
        }
        cx.notify();
    }

    fn activate_selected_download(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.hub.lock().downloads().into_iter().nth(self.downloads_selected) else { return };
        match d.state {
            relay_bridge::DownloadState::Downloading => {
                self.web.borrow_mut().apply(BrowserRequest::CancelDownload { tab: d.tab, id: d.id });
                self.toast("Download cancelled");
            }
            relay_bridge::DownloadState::Failed(_) => {
                self.web.borrow_mut().apply(BrowserRequest::Navigate { tab: d.tab, url: d.url.clone() });
                self.toast("Retrying download");
            }
            relay_bridge::DownloadState::Done => {
                if let Some(path) = &d.path {
                    self.reveal_download(path, cx);
                }
            }
        }
        cx.notify();
    }

    fn open_downloads(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.settings_open = false;
        self.library = None;
        self.shortcuts = None;
        self.connect_open = false;
        self.find = None;
        self.end_hints();
        self.downloads_open = true;
        self.downloads_selected = 0;
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_downloads(&mut self) {
        self.downloads_open = false;
        self.focus_active_page();
    }

    fn open_connect(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.settings_open = false;
        self.library = None;
        self.shortcuts = None;
        self.downloads_open = false;
        self.find = None;
        self.end_hints();
        self.connect_open = true;
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_connect(&mut self) {
        self.connect_open = false;
        self.focus_active_page();
    }

    fn open_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.settings_open = false;
        self.library = None;
        self.connect_open = false;
        self.downloads_open = false;
        self.find = None;
        self.end_hints();
        self.shortcuts = Some(ShortcutEditor { recording: None });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_shortcuts(&mut self) {
        if self.shortcuts.as_ref().is_some_and(|s| s.recording.is_some()) {
            // Restore the map that was active before recording.
            self.restore_keymap();
        }
        self.shortcuts = None;
        self.focus_active_page();
    }

    fn restore_keymap(&mut self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("keybindings", &self.keymap.to_setting());
        }
    }

    fn apply_keymap(&mut self, cx: &mut Context<Self>) {
        cx.clear_key_bindings();
        cx.bind_keys(self.keymap.bindings());
        cx.set_menus(app_menus_named(self.app_name.as_deref()));
        self.restore_keymap();
    }

    fn start_recording(&mut self, id: ActionId, cx: &mut Context<Self>) {
        if let Some(editor) = self.shortcuts.as_mut() {
            editor.recording = Some(id);
            // No action may fire while the user presses the new shortcut.
            cx.clear_key_bindings();
            cx.set_menus(vec![]);
            cx.notify();
        }
    }

    fn finish_recording(&mut self, id: ActionId, key: &str, cx: &mut Context<Self>) {
        match self.keymap.set(id, key) {
            Ok(()) => {
                if let Some(editor) = self.shortcuts.as_mut() {
                    editor.recording = None;
                }
                self.apply_keymap(cx);
                self.toast(format!("{}: {}", id.label(), keymap::display_key(key)));
            }
            Err(e) => self.toast(e),
        }
        cx.notify();
    }

    fn set_theme_choice(&mut self, choice: ThemeChoice) {
        self.theme_choice = choice;
        if let Some(store) = &self.store {
            let _ = store.set_setting("theme", choice.as_str());
        }
    }

    fn set_accent_choice(&mut self, choice: AccentChoice) {
        self.accent_choice = choice;
        if let Some(store) = &self.store {
            let _ = store.set_setting("accent", choice.as_str());
        }
    }

    fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        let next = self.theme_choice.next();
        self.set_theme_choice(next);
        self.toast(format!("Appearance: {}", next.label()));
        cx.notify();
    }

    fn cycle_accent(&mut self, cx: &mut Context<Self>) {
        let next = self.accent_choice.next();
        self.set_accent_choice(next);
        self.toast(format!("Accent: {}", next.label()));
        cx.notify();
    }

    fn render_downloads(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let downloads = self.hub.lock().downloads();
        let mut rows = div().flex().flex_col();
        if downloads.is_empty() {
            rows = rows.child(div().px_4().py_6().text_size(px(13.)).text_color(t.muted).child("No downloads yet"));
        }
        for (idx, d) in downloads.into_iter().enumerate() {
            let selected = idx == self.downloads_selected;
            let state = match &d.state {
                relay_bridge::DownloadState::Downloading => "Downloading…".to_string(),
                relay_bridge::DownloadState::Done => "Done".to_string(),
                relay_bridge::DownloadState::Failed(e) => format!("Failed: {e}"),
            };
            let path = d.path.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("download-{}", d.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(40.))
                    .px_4()
                    .py_1()
                    .border_b_1()
                    .border_color(t.border)
                    .when(selected, |el| el.bg(t.hover))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .overflow_hidden()
                            .child(div().text_size(px(12.)).text_color(t.text).whitespace_nowrap().text_ellipsis().child(SharedString::from(d.filename)))
                            .child(div().text_size(px(10.)).text_color(t.muted).whitespace_nowrap().text_ellipsis().child(SharedString::from(format!("{state} · {}", d.url)))),
                    )
                    .child(
                        div()
                            .when(matches!(d.state, relay_bridge::DownloadState::Failed(_)), |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-retry-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.web.borrow_mut().apply(BrowserRequest::Navigate { tab: d.tab, url: d.url.clone() });
                                            app.toast("Retrying download");
                                            cx.notify();
                                        }))
                                        .child("Retry"),
                                )
                            })
                            .when(d.state == relay_bridge::DownloadState::Downloading, |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-cancel-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.web.borrow_mut().apply(BrowserRequest::CancelDownload { tab: d.tab, id: d.id });
                                            app.toast("Download cancelled");
                                            cx.notify();
                                        }))
                                        .child("Cancel"),
                                )
                            })
                            .when(path.is_some(), |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-reveal-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            if let Some(path) = &path {
                                                app.reveal_download(path, cx);
                                            } else {
                                                cx.notify();
                                            }
                                        }))
                                        .child("Reveal"),
                                )
                            }),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(
            div()
                .id("downloads")
                .w(px(680.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_downloads()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Downloads"))
                        .child(
                            div()
                                .id("downloads-clear")
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(t.border)
                                .text_size(px(11.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    let n = app.hub.update(|h| h.clear_finished_downloads());
                                    app.toast(format!("Cleared {n} finished downloads"));
                                    cx.notify();
                                }))
                                .child("Clear finished"),
                        ),
                )
                .child(rows),
        )
    }

    fn render_connect(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut rows = div().flex().flex_col();
        for client in mcp_config::Client::ALL {
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("connect-{}", client.id())))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .min_h(px(46.))
                    .px_4()
                    .py_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().text_size(px(13.)).text_color(t.text).child(client.name()))
                            .child(div().text_size(px(10.)).text_color(t.muted).child(client.target())),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("connect-copy-{}", client.id())))
                            .px_2()
                            .py_1()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(t.border)
                            .text_size(px(11.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let command = mcp_config::relay_command();
                                cx.write_to_clipboard(ClipboardItem::new_string(client.snippet(&command)));
                                app.toast(format!("Copied the {} config", client.name()));
                                cx.notify();
                            }))
                            .child("Copy config"),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(
            div()
                .id("connect")
                .w(px(620.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_connect()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Connect an agent"))
                        .child(div().text_size(px(11.)).text_color(t.muted).child("Copy the text; Relay changes no files")),
                )
                .child(rows),
        )
    }

    fn render_shortcuts(&self, editor: &ShortcutEditor, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().id("shortcuts-list").flex().flex_col().max_h(px(460.)).overflow_y_scroll();
        for (i, id) in ActionId::ALL.into_iter().enumerate() {
            let recording = editor.recording == Some(id);
            list = list.child(
                div()
                    .id(("shortcut", i))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .h(px(32.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(12.))
                    .when(recording, |d| d.bg(t.active))
                    .child(div().text_color(t.text).child(id.label()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_color(t.muted).child(SharedString::from(keymap::display_key(self.keymap.key(id)))))
                            .child(
                                div()
                                    .id(("shortcut-change", i))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if recording { t.accent } else { t.border })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.start_recording(id, cx);
                                    }))
                                    .child(if recording { "Press keys…" } else { "Change" }),
                            ),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(60.)).bg(t.scrim).child(
            div()
                .id("shortcuts")
                .w(px(640.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_shortcuts()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Keyboard shortcuts"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .id("shortcuts-reset")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.keymap.reset_all();
                                            app.apply_keymap(cx);
                                            app.toast("Shortcuts reset to defaults");
                                            cx.notify();
                                        }))
                                        .child("Reset all"),
                                )
                                .child(div().text_size(px(11.)).text_color(t.muted).child(if editor.recording.is_some() { "Press a shortcut · Esc cancels" } else { "Esc closes" })),
                        ),
                )
                .child(list),
        )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let pill = |id: &'static str, label: SharedString, danger: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded(px(6.))
                .border_1()
                .border_color(if danger { t.error } else { t.border })
                .bg(t.background)
                .text_size(px(12.))
                .text_color(if danger { t.error } else { t.text })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        let row = |title: &'static str, subtitle: Option<&'static str>, control: gpui::AnyElement| {
            let mut left = div().flex().flex_col().gap_0p5().child(div().text_size(px(13.)).text_color(t.text).child(title));
            if let Some(sub) = subtitle {
                left = left.child(div().text_size(px(11.)).text_color(t.muted).child(sub));
            }
            div().flex().items_center().justify_between().px_4().py_2p5().border_b_1().border_color(t.border).child(left).child(control)
        };
        let sleep_label: SharedString = if self.sleep_minutes == 0 { "Off".into() } else { format!("{} min", self.sleep_minutes).into() };
        let clear_label: SharedString = if self.confirm_clear_history { "Confirm clear history".into() } else { "Clear history".into() };
        let search_btn = pill("settings-search", palette::search_label(&self.search).into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                let idx = palette::SEARCH_PRESETS
                    .iter()
                    .position(|(_, url)| *url == app.search)
                    .map(|i| (i + 1) % palette::SEARCH_PRESETS.len())
                    .unwrap_or(0);
                app.search = palette::SEARCH_PRESETS[idx].1.to_string();
                if let Some(store) = &app.store {
                    let _ = store.set_setting("search", &app.search);
                }
                cx.notify();
            }))
            .into_any_element();
        let blocking_btn = pill("settings-blocking", if self.blocking { "On" } else { "Off" }.into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.blocking = !app.blocking;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("blocking", if app.blocking { "on" } else { "off" });
                }
                if app.blocking {
                    app.web.borrow().load_content_rules(&rules_json(app.store.as_ref()));
                } else {
                    app.web.borrow().clear_content_rules();
                }
                cx.notify();
            }))
            .into_any_element();
        let sleep_btn = pill("settings-sleep", sleep_label, false)
            .on_click(cx.listener(|app, _, _, cx| {
                const PRESETS: [u64; 5] = [5, 15, 30, 60, 0];
                let idx = PRESETS.iter().position(|&m| m == app.sleep_minutes).map(|i| (i + 1) % PRESETS.len()).unwrap_or(0);
                app.sleep_minutes = PRESETS[idx];
                app.sleep_policy.idle_after = Duration::from_secs(app.sleep_minutes * 60);
                if let Some(store) = &app.store {
                    let _ = store.set_setting("sleep_minutes", &app.sleep_minutes.to_string());
                }
                cx.notify();
            }))
            .into_any_element();
        let mcp_btn = pill("settings-mcp", if self.mcp_enabled { "On" } else { "Off" }.into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.mcp_enabled = !app.mcp_enabled;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("mcp", if app.mcp_enabled { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let eval_btn = pill("settings-agent-eval", if self.agent_eval { "On" } else { "Off" }.into(), self.agent_eval)
            .on_click(cx.listener(|app, _, _, cx| {
                app.agent_eval = !app.agent_eval;
                app.hub.set_agent_eval(app.agent_eval);
                if let Some(store) = &app.store {
                    let _ = store.set_setting("agent_eval", if app.agent_eval { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let connect_btn = pill("settings-connect", "Set up…".into(), false)
            .on_click(cx.listener(|app, _, _, cx| app.open_connect(cx)))
            .into_any_element();
        let shell_btn = pill("settings-shell-control", if self.shell_control { "On" } else { "Off" }.into(), self.shell_control)
            .on_click(cx.listener(|app, _, _, cx| {
                app.shell_control = !app.shell_control;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("shell_control", if app.shell_control { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let strip_position = pill("settings-tabs-position", self.strip.placement.label().into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.rotate_tab_strip(cx);
            }))
            .into_any_element();
        let strip_folded = pill(
            "settings-tabs-folded",
            if self.strip.folded { "Folded" } else { "Open" }.into(),
            false,
        )
        .on_click(cx.listener(|app, _, _, cx| {
            app.toggle_tab_strip(cx);
        }))
        .into_any_element();
        let strip_control = div().flex().items_center().gap_1().child(strip_position).child(strip_folded).into_any_element();
        let clear_btn = pill("settings-clear-history", clear_label, self.confirm_clear_history)
            .on_click(cx.listener(|app, _, _, cx| {
                if !app.confirm_clear_history {
                    app.confirm_clear_history = true;
                } else {
                    app.confirm_clear_history = false;
                    match app.store.as_ref().map(|s| s.clear_history().and_then(|n| s.clear_session().map(|_| n))) {
                        Some(Ok(n)) => {
                            app.saved_session.clear();
                            app.refresh_start_page();
                            app.toast(format!("Cleared {n} history entries and the saved session"))
                        }
                        Some(Err(e)) => app.toast(format!("Clear history failed: {e}")),
                        None => app.toast("Profile store unavailable"),
                    }
                }
                cx.notify();
            }))
            .into_any_element();
        let mut theme_control = div().flex().items_center().gap_1();
        for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            let id = match choice {
                ThemeChoice::System => "settings-theme-system",
                ThemeChoice::Light => "settings-theme-light",
                ThemeChoice::Dark => "settings-theme-dark",
            };
            theme_control = theme_control.child(
                pill(id, choice.label().into(), false)
                    .when(choice == self.theme_choice, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_theme_choice(choice);
                        cx.notify();
                    })),
            );
        }
        let mut accent_control = div().flex().items_center().gap_1();
        for choice in AccentChoice::ALL {
            let id = match choice {
                AccentChoice::Blue => "settings-accent-blue",
                AccentChoice::Purple => "settings-accent-purple",
                AccentChoice::Green => "settings-accent-green",
                AccentChoice::Orange => "settings-accent-orange",
            };
            accent_control = accent_control.child(
                pill(id, choice.label().into(), false)
                    .when(choice == self.accent_choice, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_accent_choice(choice);
                        cx.notify();
                    })),
            );
        }
        let theme_control = theme_control.into_any_element();
        let accent_control = accent_control.into_any_element();
        let shortcuts_btn = pill("settings-shortcuts", "Edit…".into(), false)
            .on_click(cx.listener(|app, _, _, cx| app.open_shortcuts(cx)))
            .into_any_element();
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(90.)).bg(t.scrim).child(
            div()
                .id("settings")
                .w(px(520.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_settings();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Settings"))
                        .child(div().text_size(px(12.)).text_color(t.muted).child("Esc")),
                )
                .child(row("Keyboard shortcuts", Some("Click Change, then press the new keys"), shortcuts_btn))
                .child(row("Appearance", Some("System follows macOS"), theme_control))
                .child(row("Accent", None, accent_control))
                .child(row("Search engine", None, search_btn))
                .child(row("Ad & tracker blocking", None, blocking_btn))
                .child(row("Tab sleep timeout", None, sleep_btn))
                .child(row("Tab strip", Some("⌘S folds · ⇧⌘S moves"), strip_control))
                .child(row("Local MCP server", Some("(restart to apply)"), mcp_btn))
                .child(row("Agent JavaScript (eval_js)", Some("On only for Allow sites; agents can read page data"), eval_btn))
                .child(row("Agent shell changes", Some("Allow plan/apply for theme, accent, and start sections"), shell_btn))
                .child(row("Connect an agent", Some("Claude Code, Codex, OpenCode, Cursor, Zed"), connect_btn))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .py_2p5()
                        .child(div().text_size(px(13.)).text_color(t.text).child("Browsing history"))
                        .child(clear_btn),
                ),
        )
    }

    /// What the window title should say: the active page, or Relay itself.
    /// A web app window is just its page; the bundle already carries its name.
    fn document_title(&self) -> String {
        let h = self.hub.lock();
        match h.active().and_then(|a| h.tab(a)) {
            Some(t) if !t.title.is_empty() => {
                if self.app_mode { t.title.clone() } else { format!("{} — Relay", t.title) }
            }
            Some(t) => {
                if self.app_mode { palette::short_url(&t.url) } else { format!("{} — Relay", palette::short_url(&t.url)) }
            }
            None => self.app_name.clone().unwrap_or_else(|| "Relay".to_string()),
        }
    }

    fn render_find(&self, f: &FindBar) -> impl IntoElement {
        let t = self.theme;
        let (text, color) = if f.query.is_empty() {
            ("Find in page…".to_string(), t.muted)
        } else {
            (f.query.clone(), t.text)
        };
        let status = match f.found {
            Some(false) => "No matches",
            Some(true) => "↩ next · ⇧↩ previous",
            None => "esc to close",
        };
        div().absolute().top(px(10.)).right(px(16.)).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .w(px(340.))
                .h(px(34.))
                .px_3()
                .rounded(px(8.))
                .bg(t.surface)
                .border_1()
                .border_color(if f.found == Some(false) { t.warn } else { t.border })
                .shadow_lg()
                .text_size(px(13.))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(color).child(SharedString::from(text)))
                .child(div().text_size(px(11.)).text_color(if f.found == Some(false) { t.warn } else { t.muted }).child(status)),
        )
    }

    fn render_agent_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(30.))
            .px_3()
            .bg(t.agent_bg)
            .border_t_1()
            .border_color(t.agent)
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().child("Agents paused — page automation is stopped"))
            .child(
                div()
                    .id("resume-agents")
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(t.agent)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.hub.set_agents_paused(false);
                        app.toast("Agents resumed");
                        cx.notify();
                    }))
                    .child("Resume"),
            )
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
        if self.mcp_enabled {
            right.push_str(" · agents: relay mcp");
        } else {
            right.push_str(" · mcp: off");
        }
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
        self.theme = Theme::for_choice(window.appearance(), self.theme_choice, self.accent_choice);
        let title = if self.title_cache.0 == self.seen_version {
            self.title_cache.1.clone()
        } else {
            let title = self.document_title();
            self.title_cache = (self.seen_version, title.clone());
            title
        };
        if title != self.window_title {
            self.window_title = title.clone();
            window.set_window_title(&title);
        }
        // Panes re-register their bounds every frame; drop tabs not shown.
        let visible = self.visible_tabs();
        self.placements.borrow_mut().retain(|(id, _)| visible.contains(id));
        let overlay_open = self.palette.is_some() || self.settings_open || self.library.is_some() || self.shortcuts.is_some() || self.connect_open || self.downloads_open;
        if overlay_open {
            // Keep native views from covering the palette or settings overlay.
            self.web.borrow().layout(&[]);
            self.placements.borrow_mut().clear();
        }
        let t = self.theme;
        let content = if overlay_open {
            div().flex_1().h_full().bg(t.page).into_any_element()
        } else {
            self.render_content(cx).into_any_element()
        };
        // A web app window starts chromeless; a popup tab brings the strip
        // back rather than hiding a tab the user cannot reach.
        let show_strip = !self.app_mode || self.hub.lock().tabs().count() > 1;
        let mut main = div().flex_1().overflow_hidden();
        if show_strip {
            main = match self.strip.placement {
                tabstrip::Placement::Top => main.flex().flex_col().child(self.render_tab_bar(cx)).child(content),
                tabstrip::Placement::Left => main.flex().child(self.render_tab_bar(cx)).child(content),
                tabstrip::Placement::Right => main.flex().child(content).child(self.render_tab_bar(cx)),
            };
        } else {
            main = main.child(content);
        }
        if self.inspector_open {
            main = main.flex().child(self.render_inspector(cx));
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
            .on_action(cx.listener(|app, _: &NewTab, _, cx| {
                app.run_command(Command::NewTab, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &CloseTab, _, cx| {
                app.run_command(Command::CloseTab, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ReopenClosedTab, _, cx| {
                app.run_command(Command::ReopenClosedTab, cx);
                cx.notify();
            }))
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
            .on_action(cx.listener(|app, _: &ToggleSettings, _, cx| {
                app.toggle_settings();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &LinkHints, _, cx| {
                app.run_command(Command::LinkHints, cx);
                cx.notify()
            }))
            .on_action(cx.listener(|app, _: &CopyCleanUrl, _, cx| {
                app.run_command(Command::CopyCleanUrl, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &CopyMarkdown, _, cx| {
                app.run_command(Command::CopyMarkdown, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ToggleBookmark, _, cx| {
                app.run_command(Command::ToggleBookmark, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &TogglePin, _, cx| {
                app.run_command(Command::TogglePin, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &Find, _, cx| {
                app.run_command(Command::Find, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ZoomIn, _, cx| {
                app.run_command(Command::ZoomIn, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ZoomOut, _, cx| {
                app.run_command(Command::ZoomOut, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ZoomReset, _, cx| {
                app.run_command(Command::ZoomReset, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &Quit, _, cx| {
                app.save_session();
                cx.quit()
            }))
            .on_action(cx.listener(|app, _: &ToggleTabStrip, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .on_action(cx.listener(|app, _: &NextTabStripPosition, _, cx| {
                app.rotate_tab_strip(cx);
            }))
            .on_action(cx.listener(|app, _: &InstallWebApp, _, cx| {
                app.install_web_app(cx);
            }))
            .on_action(cx.listener(|app, _: &RemoveWebApp, _, cx| {
                app.remove_web_app(cx);
            }))
            .on_action(cx.listener(|app, _: &Cut, _, cx| app.apply_edit(EditAction::Cut, cx)))
            .on_action(cx.listener(|app, _: &Copy, _, cx| app.apply_edit(EditAction::Copy, cx)))
            .on_action(cx.listener(|app, _: &Paste, _, cx| app.apply_edit(EditAction::Paste, cx)))
            .on_action(cx.listener(|app, _: &SelectAll, _, cx| app.apply_edit(EditAction::SelectAll, cx)))
            .on_action(cx.listener(|app, _: &Undo, _, cx| app.apply_edit(EditAction::Undo, cx)))
            .on_action(cx.listener(|app, _: &Redo, _, cx| app.apply_edit(EditAction::Redo, cx)))
            .on_action(cx.listener(|app, _: &Minimize, _, cx| {
                app.web.borrow().minimize();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &Help, _, cx| {
                app.open_help();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ShowHistory, _, cx| {
                app.open_library(Mode::History, cx);
            }))
            .on_action(cx.listener(|app, _: &ShowBookmarks, _, cx| {
                app.open_library(Mode::Bookmarks, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleAgents, _, cx| {
                app.toggle_agents(cx);
            }))
            .on_action(cx.listener(|app, _: &CycleAgentPolicy, _, cx| {
                app.cycle_agent_policy(cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleReader, _, cx| {
                app.toggle_reader(cx);
            }))
            .on_action(cx.listener(|app, _: &TogglePictureInPicture, _, cx| {
                app.toggle_picture_in_picture(cx);
            }))
            .on_action(cx.listener(|app, _: &ShowDownloads, _, cx| {
                app.open_downloads(cx);
            }));
        macro_rules! select {
            ($($a:ident => $i:expr),*) => {
                $(root = root.on_action(cx.listener(|app, _: &$a, _, cx| { app.select_index($i); cx.notify() }));)*
            };
        }
        select!(SelectTab1 => 0, SelectTab2 => 1, SelectTab3 => 2, SelectTab4 => 3, SelectTab5 => 4, SelectTab6 => 5, SelectTab7 => 6, SelectTab8 => 7, SelectTab9 => 8);
        root = root.child(main);
        if self.hub.agents_paused() {
            root = root.child(self.render_agent_banner(cx));
        }
        if !self.app_mode || self.toast.is_some() || self.hints.is_some() {
            root = root.child(self.render_status());
        }
        if let Some(f) = &self.find {
            root = root.child(self.render_find(f));
        }
        if let Some(p) = &self.palette {
            root = root.child(self.render_palette(p, cx));
        }
        if self.settings_open {
            root = root.child(self.render_settings(cx));
        }
        if let Some(lib) = &self.library {
            root = root.child(self.render_library(lib, cx));
        }
        if let Some(editor) = &self.shortcuts {
            root = root.child(self.render_shortcuts(editor, cx));
        }
        if self.connect_open {
            root = root.child(self.render_connect(cx));
        }
        if self.downloads_open {
            root = root.child(self.render_downloads(cx));
        }
        root
    }
}

/// The user's open tabs in display order, as the store persists them.
/// Agent tabs are not part of the user's session. Awake tabs restore at the
/// top (their scroll is only known once they sleep).
fn session_snapshot(hub: &Hub, slept_scroll_y: impl Fn(TabId) -> Option<f64>) -> Vec<SessionTab> {
    let active = hub.active();
    hub.tabs()
        .filter(|t| t.owner == Owner::User && is_http_url(&t.url))
        .enumerate()
        .map(|(i, t)| SessionTab {
            url: t.url.clone(),
            title: t.title.clone(),
            pinned: t.pinned,
            scroll_y: if t.sleeping { slept_scroll_y(t.id).unwrap_or(0.0) } else { 0.0 },
            active: Some(t.id) == active,
            position: i as i64,
        })
        .collect()
}

/// Recreate saved tabs in the hub. Returns each new tab with its URL and
/// scroll, plus the tab to activate (the saved active one, else the first).
fn restore_into(hub: &mut Hub, saved: &[SessionTab]) -> (Vec<(TabId, String, f64)>, Option<TabId>) {
    let mut restored = Vec::with_capacity(saved.len());
    let mut active = None;
    for s in saved.iter().filter(|s| is_http_url(&s.url)) {
        let id = hub.create_tab(&s.url, Owner::User);
        if let Some(t) = hub.tab_mut(id) {
            t.title = s.title.clone();
            t.pinned = s.pinned;
        }
        if s.active {
            active = Some(id);
        }
        restored.push((id, s.url.clone(), s.scroll_y));
    }
    let active = active.or_else(|| restored.first().map(|r| r.0));
    if let Some(id) = active {
        hub.set_active(id);
    }
    (restored, active)
}

pub(crate) fn setting_enabled(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "false" | "0"))
}

fn next_console_min(level: ConsoleLevel) -> ConsoleLevel {
    match level {
        ConsoleLevel::Log | ConsoleLevel::Info => ConsoleLevel::Warn,
        ConsoleLevel::Warn => ConsoleLevel::Error,
        ConsoleLevel::Error => ConsoleLevel::Debug,
        ConsoleLevel::Debug => ConsoleLevel::Log,
    }
}

fn console_filter_label(level: ConsoleLevel) -> &'static str {
    match level {
        ConsoleLevel::Log | ConsoleLevel::Info => "Log+",
        ConsoleLevel::Warn => "Warn+",
        ConsoleLevel::Error => "Errors",
        ConsoleLevel::Debug => "All",
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Built-in list plus the user's `relay_home()/filters.txt` (EasyList syntax).
/// The content-blocker JSON: built-in + cookie-banner + `filters.txt` rules,
/// then an exception for every site the user turned blocking off on.
fn rules_json(store: Option<&Store>) -> String {
    let mut rules = relay_webkit::content_rules::compile(&blocklist());
    let off: Vec<String> = store
        .and_then(|s| s.all_site_prefs().ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, p)| p.blocking_off)
        .map(|(host, _)| host)
        .collect();
    rules.allow_sites(&off);
    rules.to_json()
}

fn blocklist() -> String {
    let mut s = relay_webkit::content_rules::BUILTIN_FILTERS.to_string();
    s.push_str(relay_webkit::content_rules::CMP_FILTERS);
    if let Ok(extra) = std::fs::read_to_string(relay_bridge::paths::relay_home().join("filters.txt")) {
        s.push('\n');
        s.push_str(&extra);
    }
    s
}

/// Whether a tab finished a page load the shell has not handled yet. A tab
/// whose page never said `hello` (`load_seq == 0`: just created, or restored
/// and woken but still on WebKit's blank document) has not loaded anything;
/// running `after_load` then would restore scroll onto the blank page.
fn page_loaded_since(recorded_seq: Option<u64>, load_seq: u64) -> bool {
    load_seq > 0 && recorded_seq != Some(load_seq)
}

/// What compat mode sends: Safari's user agent shape (WebKit's default omits
/// the `Version/… Safari/…` tokens some sites sniff for).
const SAFARI_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";

#[derive(Debug, PartialEq)]
enum CompatChange {
    None,
    /// Switch to the Safari agent and reload once (this load used the default).
    Enable,
    /// Back to the default agent for the next requests; no reload needed.
    Disable,
}

fn compat_change(view_has_compat: bool, site_wants_compat: bool) -> CompatChange {
    match (view_has_compat, site_wants_compat) {
        (false, true) => CompatChange::Enable,
        (true, false) => CompatChange::Disable,
        _ => CompatChange::None,
    }
}

/// Browser-style zoom presets; `direction` 1 / -1 steps, 0 resets.
const ZOOM_STEPS: [f64; 13] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

fn zoom_step(current: f64, direction: i8) -> f64 {
    let current = if current.is_finite() && current > 0.0 { current } else { 1.0 };
    match direction {
        0 => 1.0,
        d if d > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > current + 1e-6).unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]),
        _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 1e-6).unwrap_or(ZOOM_STEPS[0]),
    }
}

/// "820 B", "14 KB" for toasts.
fn byte_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// What the saved session keeps (matches `Store::save_session`).
/// Decode the bridge's untrusted `favicon` reply. Only PNG/JPEG/WebP/GIF
/// under 256 KB is accepted; the data URL prefix and the mime must agree.
fn favicon_payload(value: &serde_json::Value) -> Option<(Vec<u8>, &'static str)> {
    if value.get("available")?.as_bool()? != true {
        return None;
    }
    let data = value.get("data")?.as_str()?;
    let mime = value.get("mime")?.as_str()?;
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => return None,
    };
    let prefix = format!("data:{mime};base64,");
    let payload = data.strip_prefix(&prefix)?;
    if payload.len() > 256 * 1024 * 4 / 3 {
        return None;
    }
    let bytes = webapp::base64_decode(payload)?;
    (bytes.len() <= 256 * 1024).then_some((bytes, ext))
}

fn favicon_decode(bytes: &[u8], ext: &str) -> Image {
    let format = match ext {
        "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        _ => ImageFormat::Png,
    };
    Image::from_bytes(format, bytes.to_vec())
}

#[cfg_attr(not(test), allow(dead_code))]
fn favicon_image(value: &serde_json::Value) -> Option<Image> {
    let (bytes, ext) = favicon_payload(value)?;
    Some(favicon_decode(&bytes, ext))
}

fn favicon_file_name(host: &str) -> String {
    host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect::<String>().trim_matches('.').to_string()
}

/// Write the icon 0600 so the start page shows it on later launches.
fn cache_favicon_file(host: &str, bytes: &[u8], ext: &str) {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let name = favicon_file_name(host);
    if name.is_empty() {
        return;
    }
    let dir = relay_bridge::paths::relay_home().join("favicons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let path = dir.join(format!("{name}.{ext}"));
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(path) {
        let _ = file.write_all(bytes);
    }
}

fn cached_favicons() -> HashMap<String, Option<Arc<Image>>> {
    let dir = relay_bridge::paths::relay_home().join("favicons");
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten().take(512) {
        let path = entry.path();
        let (Some(ext), Some(stem)) = (path.extension().and_then(|e| e.to_str()), path.file_stem().and_then(|s| s.to_str())) else { continue };
        let ext = ext.to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "webp" | "gif") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() > 256 * 1024 {
            continue;
        }
        out.insert(stem.to_string(), Some(Arc::new(favicon_decode(&bytes, &ext))));
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Action;
    use std::collections::HashSet;

    fn saved(url: &str, pinned: bool, active: bool, scroll_y: f64) -> SessionTab {
        SessionTab { url: url.into(), title: format!("T {url}"), pinned, scroll_y, active, position: 0 }
    }

    #[test]
    fn restore_recreates_tabs_in_order_with_pins_and_activates_the_saved_tab() {
        let hub = SharedHub::new();
        let saved = vec![
            saved("https://a.example/", true, false, 0.0),
            saved("about:blank", false, false, 0.0),
            saved("https://b.example/", false, true, 340.0),
        ];
        let (restored, active) = hub.update(|h| restore_into(h, &saved));
        assert_eq!(restored.len(), 2, "non-web URLs are not restored");
        assert_eq!(restored[1].1, "https://b.example/");
        assert_eq!(restored[1].2, 340.0);
        assert_eq!(active, Some(restored[1].0));
        let h = hub.lock();
        assert_eq!(h.active(), active);
        let tabs: Vec<_> = h.tabs().map(|t| (t.url.clone(), t.pinned, t.title.clone())).collect();
        assert_eq!(tabs[0], ("https://a.example/".into(), true, "T https://a.example/".into()));
        assert_eq!(tabs[1].0, "https://b.example/");
    }

    #[test]
    fn restore_without_a_saved_active_tab_activates_the_first() {
        let hub = SharedHub::new();
        let two = [saved("https://a.example/", false, false, 0.0), saved("https://b.example/", false, false, 0.0)];
        let (restored, active) = hub.update(|h| restore_into(h, &two));
        assert_eq!(active, Some(restored[0].0));
        let (none, no_active) = SharedHub::new().update(|h| restore_into(h, &[]));
        assert!(none.is_empty() && no_active.is_none());
    }

    #[test]
    fn snapshot_keeps_user_web_tabs_in_order_and_round_trips() {
        let hub = SharedHub::new();
        let a = hub.update(|h| {
            let a = h.create_tab("https://a.example/", Owner::User);
            h.create_tab("https://agent.example/", Owner::Agent);
            let b = h.create_tab("https://b.example/", Owner::User);
            h.tab_mut(b).unwrap().pinned = true;
            h.tab_mut(a).unwrap().sleeping = true;
            h.set_active(b);
            a
        });
        let snap = session_snapshot(&hub.lock(), |t| (t == a).then_some(512.0));
        assert_eq!(snap.len(), 2, "agent tabs are not saved");
        assert!(snap.iter().all(|s| s.url != "https://agent.example/"));
        assert_eq!((snap[0].scroll_y, snap[0].active, snap[0].position), (512.0, false, 0));
        assert_eq!((snap[1].pinned, snap[1].active, snap[1].position), (true, true, 1));

        let fresh = SharedHub::new();
        let (_, active) = fresh.update(|h| restore_into(h, &snap));
        let again = session_snapshot(&fresh.lock(), |_| None);
        let key = |v: &[SessionTab]| v.iter().map(|s| (s.url.clone(), s.pinned, s.active)).collect::<Vec<_>>();
        assert_eq!(key(&again), key(&snap));
        assert_eq!(fresh.lock().active(), active);
    }

    #[test]
    fn only_a_real_page_load_counts_as_loaded() {
        assert!(!page_loaded_since(None, 0), "created or restored-and-woken tab on the blank document");
        assert!(page_loaded_since(None, 1), "first hello");
        assert!(!page_loaded_since(Some(1), 1), "already handled");
        assert!(page_loaded_since(Some(1), 2), "next navigation");
    }

    #[test]
    fn title_changes_do_not_count_as_visits() {
        let rec = RecordedVisit { load_seq: 7, title: "First".into() };
        assert_eq!(visit_decision(7, "First", Some(&rec)), VisitDecision::Ignore);
        assert_eq!(visit_decision(7, "Second", Some(&rec)), VisitDecision::TitleOnly);
        assert_eq!(visit_decision(7, "Third", Some(&rec)), VisitDecision::TitleOnly, "one visit per load, not per title");
        assert_eq!(visit_decision(8, "Second", Some(&rec)), VisitDecision::Record, "a new load records again");
        assert_eq!(visit_decision(0, "Second", None), VisitDecision::Ignore);
        assert_eq!(visit_decision(1, "Second", None), VisitDecision::Record);
    }

    #[test]
    fn compat_reloads_only_when_a_compat_site_used_the_default_agent() {
        assert_eq!(compat_change(false, true), CompatChange::Enable);
        assert_eq!(compat_change(true, true), CompatChange::None, "after the one reload, no loop");
        assert_eq!(compat_change(true, false), CompatChange::Disable);
        assert_eq!(compat_change(false, false), CompatChange::None);
        assert!(SAFARI_USER_AGENT.contains("Version/") && SAFARI_USER_AGENT.ends_with("Safari/605.1.15"));
    }

    #[test]
    fn zoom_steps_through_presets_and_clamps() {
        assert_eq!(zoom_step(1.0, 1), 1.1);
        assert_eq!(zoom_step(1.0, -1), 0.9);
        assert_eq!(zoom_step(1.3, 1), 1.5, "off-preset values move to the next preset");
        assert_eq!(zoom_step(1.3, -1), 1.25);
        assert_eq!(zoom_step(3.0, 1), 3.0);
        assert_eq!(zoom_step(0.5, -1), 0.5);
        assert_eq!(zoom_step(2.5, 0), 1.0);
        assert_eq!(zoom_step(f64::NAN, 1), 1.1);
    }

    #[test]
    fn byte_size_reads_naturally() {
        assert_eq!(byte_size(820), "820 B");
        assert_eq!(byte_size(1024), "1 KB");
        assert_eq!(byte_size(14_000), "14 KB");
    }

    #[test]
    fn favicon_reply_is_validated_before_decoding() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lU2l4wAAAABJRU5ErkJggg==";
        assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": format!("data:image/png;base64,{png}")})).is_some());
        assert!(favicon_image(&serde_json::json!({"available": false, "mime": "image/png", "data": format!("data:image/png;base64,{png}")})).is_none());
        assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/svg+xml", "data": "data:image/svg+xml;base64,PHN2Zz4="})).is_none());
        assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": "data:image/jpeg;base64,AAAA"})).is_none());
        assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": "not-a-data-url"})).is_none());
        let huge = format!("data:image/png;base64,{}", "A".repeat(400 * 1024));
        assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": huge})).is_none());
        assert!(!favicon_file_name("evil/../x").contains('/'));
        assert!(favicon_file_name("../../etc").chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'));
    }

    #[test]
    fn key_bindings_cover_documented_shortcuts() {
        let bindings = key_bindings();
        assert_eq!(bindings.len(), 49);
        let bound: HashSet<&'static str> = bindings.iter().map(|b| b.action().name()).collect();
        let required: [&'static str; 49] = [
            OpenPalette.name(),
            OpenLocation.name(),
            NewTab.name(),
            CloseTab.name(),
            ReopenClosedTab.name(),
            Reload.name(),
            Back.name(),
            Forward.name(),
            NextTab.name(),
            PrevTab.name(),
            ToggleInspector.name(),
            WebInspector.name(),
            ToggleSplit.name(),
            ToggleSettings.name(),
            LinkHints.name(),
            CopyCleanUrl.name(),
            CopyMarkdown.name(),
            ToggleBookmark.name(),
            TogglePin.name(),
            Find.name(),
            ZoomIn.name(),
            ZoomOut.name(),
            ZoomReset.name(),
            Quit.name(),
            SelectTab1.name(),
            SelectTab2.name(),
            SelectTab3.name(),
            SelectTab4.name(),
            SelectTab5.name(),
            SelectTab6.name(),
            SelectTab7.name(),
            SelectTab8.name(),
            SelectTab9.name(),
            ToggleTabStrip.name(),
            NextTabStripPosition.name(),
            Cut.name(),
            Copy.name(),
            Paste.name(),
            SelectAll.name(),
            Undo.name(),
            Redo.name(),
            Minimize.name(),
            ShowHistory.name(),
            ShowBookmarks.name(),
            ToggleAgents.name(),
            CycleAgentPolicy.name(),
            ToggleReader.name(),
            TogglePictureInPicture.name(),
            ShowDownloads.name(),
        ];
        for name in required {
            assert!(bound.contains(name), "missing key binding for {name}");
        }
    }

    #[test]
    fn the_menu_bar_is_complete_and_has_no_duplicate_actions() {
        let menus = app_menus();
        let names: Vec<&'static str> = menus
            .iter()
            .flat_map(|m| m.items.iter())
            .filter_map(|i| match i {
                MenuItem::Action { action, .. } => Some(action.name()),
                _ => None,
            })
            .collect();
        for required in [
            NewTab.name(),
            CloseTab.name(),
            ToggleTabStrip.name(),
            NextTabStripPosition.name(),
            InstallWebApp.name(),
            RemoveWebApp.name(),
            Cut.name(),
            Copy.name(),
            Paste.name(),
            SelectAll.name(),
            Undo.name(),
            Redo.name(),
            Find.name(),
            Minimize.name(),
            Quit.name(),
        ] {
            assert!(names.contains(&required), "the menu bar is missing {required}");
        }
        for binding in key_bindings() {
            assert!(
                names.contains(&binding.action().name()),
                "{} has a key binding but no menu item, so it will not fire while WKWebView has focus",
                binding.action().name()
            );
        }
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "a menu action appears twice: {names:?}");
        assert!(menus.iter().any(|m| m.name == "File"), "File menu");
        assert!(menus.iter().any(|m| m.name == "Edit"), "Edit menu");
        assert_eq!(app_menus_named(Some("Relay Shop"))[0].name, "Relay Shop");
        assert_eq!(app_menus()[0].name, "Relay");
    }
}
