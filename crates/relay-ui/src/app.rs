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
    actions, canvas, div, fill, img, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, DispatchPhase,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, GlobalElementId, Hsla, Image, ImageFormat,
    InspectorElementId, KeyBinding, KeyDownEvent, LayoutId, Menu, MenuItem, ModifiersChangedEvent, MouseButton, MouseDownEvent, Pixels,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use relay_bridge::{host_of, is_http_url, is_openable_url, BrowserRequest, ConsoleLevel, Hub, NodeId, Op, Owner, SharedHub, TabId, TabView};
use relay_extensions::store::{SessionTab, Store, WebAppRecord};
use relay_webkit::downloads::FOLDER_LABEL;
use relay_webkit::sleep::SleepPolicy;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
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
        ToggleFloatVideo,
        FindNext,
        FindPrevious,
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
        NewPrivateTab,
        Print,
        ExportPdf,
        ToggleBottomBar,
        TabsOnLeft,
        TabsOnTop,
        TabsOnRight,
        FoldAllGroups,
        UnfoldAllGroups,
        Spaces,
    ]
);


mod chrome;
mod inspector_view;
mod menus;
mod overlays;
mod palette_view;
#[cfg(test)]
mod tests;

pub use menus::{app_menus, key_bindings};
use menus::app_menus_named;
use palette_view::PaletteTextMetrics;
#[cfg(test)]
use palette_view::palette_text_runs;


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

impl SidebarTab {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agents",
            Self::Inspector => "inspector",
        }
    }
}

struct ShortcutEditor {
    recording: Option<ActionId>,
    /// Finds a shortcut by name or by the keys it currently uses.
    query: String,
}

/// The small modal that names a tab group.
struct GroupPrompt {
    tab: TabId,
    value: String,
}

/// The small modal that names or picks a space.
struct SpacePrompt {
    value: String,
}

struct FindBar {
    tab: TabId,
    query: String,
    found: Option<bool>,
    case_sensitive: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsTab {
    #[default]
    General,
    Appearance,
    Tabs,
    Browsing,
    Passwords,
    Privacy,
    Extensions,
    Agents,
    Shortcuts,
    About,
}

impl SettingsTab {
    pub(super) const ALL: [SettingsTab; 10] = [
        SettingsTab::General,
        SettingsTab::Appearance,
        SettingsTab::Tabs,
        SettingsTab::Browsing,
        SettingsTab::Passwords,
        SettingsTab::Privacy,
        SettingsTab::Extensions,
        SettingsTab::Agents,
        SettingsTab::Shortcuts,
        SettingsTab::About,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Appearance => "Appearance",
            SettingsTab::Tabs => "Tabs",
            SettingsTab::Browsing => "Browsing",
            SettingsTab::Passwords => "Passwords",
            SettingsTab::Privacy => "Privacy",
            SettingsTab::Extensions => "Extensions",
            SettingsTab::Agents => "Agents & MCP",
            SettingsTab::Shortcuts => "Shortcuts",
            SettingsTab::About => "About",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsDropdown {
    SearchEngine,
    SleepTimeout,
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
    /// `None` = inheritance has not been applied; `Some(None)` = System.
    last_web_dark: Option<Option<bool>>,
    /// Decoded tab icons by host. `None` means the last attempt failed; a new
    /// load retries once instead of retrying every frame.
    favicons: HashMap<String, Option<Arc<Image>>>,
    /// Host → `load_seq` whose icon request has been answered or timed out.
    favicon_tried: HashMap<String, u64>,
    favicon_pending: HashSet<String>,
    bookmarked_urls: HashSet<String>,
    cmd_held: bool,
    show_tab_numbers: bool,
    shortcuts: Option<ShortcutEditor>,
    keymap: Keymap,
    palette: Option<Palette>,
    /// True when the palette text element installed an IME input handler this
    /// frame. Printable keys then go through that handler instead of
    /// `key_char`, so composition is not swallowed and text is not inserted
    /// twice.
    palette_input_active: bool,
    /// Last shaped query line and bounds, used by `EntityInputHandler` for the
    /// IME candidate window and hit testing.
    palette_metrics: Rc<RefCell<Option<PaletteTextMetrics>>>,
    /// ⌘T opens the palette for a new tab; ⌘K/⌘L target the current tab.
    palette_new_tab: bool,
    /// ⌘⇧N opens the palette for a new ephemeral private tab.
    palette_private_tab: bool,
    /// Tab currently showing its inline right-click context menu in the tab strip.
    tab_menu: Option<TabId>,
    /// Inline name prompt opened from a tab's context menu.
    group_prompt: Option<GroupPrompt>,
    space_prompt: Option<SpacePrompt>,
    /// Group headings folded by the user; persisted as `collapsed_groups`.
    collapsed_groups: HashSet<String>,
    /// Remembered camera/mic decisions: `(host, kind)` → allowed.
    media_permissions: HashMap<(String, i64), bool>,
    /// Displayed download folder and whether each file asks first.
    downloads_dir: String,
    downloads_ask: bool,
    settings_open: bool,
    settings_tab: SettingsTab,
    settings_dropdown: Option<SettingsDropdown>,
    ext_busy: bool,
    /// Web Store URL or 32-char id typed in Settings → Extensions.
    ext_input: String,
    /// Load failures from WKWebExtension, shown in Settings → Extensions.
    ext_errors: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    blocking: bool,
    /// Hide consent-platform banners with the built-in CMP selectors.
    hide_cookie_banners: bool,
    /// Offer to fill a saved login when a sign-in field gets focus.
    login_fill_enabled: bool,
    /// Offer to save a login after a successful sign-in.
    login_save_enabled: bool,
    /// The status bar at the window's bottom edge; hide it for a quieter page
    /// (⌘⌥B or Settings → Appearance).
    show_bottom_bar: bool,
    /// Non-secret autofill offer: `(tab, url, form_idx, vec![(username, path)])`.
    login_fill_prompt: Option<(TabId, String, i64, Vec<(String, String)>)>,
    /// Fill a lone saved login as soon as a password field gets native focus.
    autofill_logins: bool,
    /// Draw site favicons in tabs; off shows deterministic letter badges.
    tab_glyphs: bool,
    /// 24 px tab rows instead of 28.
    compact_tabs: bool,
    /// A row of saved pages under the tab strip (off by default).
    show_bookmarks_bar: bool,
    /// Always show a one-line address bar above the page.
    show_address_bar: bool,
    /// Bookmarks bar shows only site icons.
    bookmarks_icons_only: bool,
    /// Bookmarks bar appears only on the new-tab page.
    bookmarks_new_tab_only: bool,
    /// Keep every tab's close button visible instead of hover/active only.
    always_show_close: bool,
    /// Cached bookmarks for that bar; refreshed with the start page.
    bookmarks_bar: Vec<(String, String)>,
    /// 100% unless the user picks another default for sites with no zoom pref.
    default_zoom: f64,
    /// Non-secret save-login offer: `(tab, url, username)`. Password stays inside Engine.
    login_save_prompt: Option<(TabId, String, String)>,
    /// `(origin_display, username)` declined in this session; never re-prompted.
    declined_login_saves: HashSet<(String, String)>,
    agent_eval: bool,
    shell_control: bool,
    agent_widget: Option<(String, String)>,
    sleep_minutes: u64,
    mcp_enabled: bool,
    confirm_clear_history: bool,
    confirm_clear_site_data: bool,
    sources: Sources,
    inspector_open: bool,
    sidebar_tab: SidebarTab,
    connect_open: bool,
    connect_detected: Vec<(mcp_config::Client, bool)>,
    connect_copied: Option<(mcp_config::Client, Instant)>,
    downloads_open: bool,
    downloads_selected: usize,
    download_poll_at: std::time::Instant,
    reader_tabs: HashSet<TabId>,
    /// Per-tab floating-video state, so the shortcut toggles both ways.
    float_tabs: std::rc::Rc<std::cell::RefCell<HashMap<TabId, bool>>>,
    /// History/Bookmarks modal; `None` when closed.
    library: Option<Library>,
    panel: Panel,
    tree: Tree,
    tree_epoch: u64,
    inspector_cache: Option<InspectorCache>,
    console_min: ConsoleLevel,
    network_failed_only: bool,
    selected_element: Option<(TabId, u64, NodeId)>,
    selected_network: Option<(TabId, u64, String)>,
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
        let mut web = WebHost::new(hub.clone(), window);
        let search = store.as_ref().and_then(|s| s.setting("search").ok().flatten()).unwrap_or_else(|| palette::DEFAULT_SEARCH.to_string());
        let blocking = setting_enabled(store.as_ref().and_then(|s| s.setting("blocking").ok().flatten()).as_deref());
        let hide_cookie_banners = setting_enabled(store.as_ref().and_then(|s| s.setting("cookie_banners").ok().flatten()).as_deref());
        let agent_eval = setting_enabled(store.as_ref().and_then(|s| s.setting("agent_eval").ok().flatten()).as_deref());
        let shell_control = setting_enabled(store.as_ref().and_then(|s| s.setting("shell_control").ok().flatten()).as_deref());
        let agent_widget = store.as_ref().and_then(|s| {
            let title = s.setting("agent_widget_title").ok().flatten()?;
            let body = s.setting("agent_widget_body").ok().flatten().unwrap_or_default();
            (!title.is_empty()).then_some((title, body))
        });
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
        // System passes `None` so WebKit follows the Mac itself; Light/Dark
        // force the matching site appearance.
        let web_dark = match theme_choice {
            ThemeChoice::System => None,
            ThemeChoice::Light => Some(false),
            ThemeChoice::Dark => Some(true),
        };
        web.set_appearance(web_dark);
        let keymap = Keymap::from_setting(store.as_ref().and_then(|s| s.setting("keybindings").ok().flatten()).as_deref());
        let show_tab_numbers = setting_enabled(store.as_ref().and_then(|s| s.setting("tab_numbers").ok().flatten()).as_deref());
        // `login_offers` is the older combined switch; new profiles use the
        // two independent rows (fill, save).
        let legacy_offers = store.as_ref().and_then(|s| s.setting("login_offers").ok().flatten());
        let login_fill_enabled = setting_enabled(
            store.as_ref().and_then(|s| s.setting("login_fill").ok().flatten()).as_deref().or(legacy_offers.as_deref()),
        );
        let login_save_enabled = setting_enabled(
            store.as_ref().and_then(|s| s.setting("login_save").ok().flatten()).as_deref().or(legacy_offers.as_deref()),
        );
        // Explicit click is the default; automatic fill is an opt-in.
        let autofill_logins = store
            .as_ref()
            .and_then(|s| s.setting("autofill_logins").ok().flatten())
            .as_deref()
            .is_some_and(|v| setting_enabled(Some(v)));
        let tab_glyphs = setting_enabled(store.as_ref().and_then(|s| s.setting("tab_glyphs").ok().flatten()).as_deref());
        let compact_tabs = !setting_enabled(store.as_ref().and_then(|s| s.setting("compact_tabs").ok().flatten()).as_deref());
        // These three are opt-in: absence must mean off, not the `on` that
        // `setting_enabled(None)` returns for ordinary toggles.
        let setting_on = |key: &str| setting_opt_in(store.as_ref().and_then(|s| s.setting(key).ok().flatten()).as_deref());
        let show_bookmarks_bar = setting_on("bookmarks_bar");
        let show_address_bar = setting_on("address_bar");
        let bookmarks_icons_only = setting_on("bookmarks_icons_only");
        let bookmarks_new_tab_only = setting_on("bookmarks_new_tab_only");
        let collapsed_groups = store
            .as_ref()
            .and_then(|s| s.setting("collapsed_groups").ok().flatten())
            .map(|raw| collapsed_group_names(Some(&raw)))
            .unwrap_or_default();
        let media_permissions = store
            .as_ref()
            .and_then(|s| s.setting("media_permissions").ok().flatten())
            .map(|raw| parse_media_permissions(&raw))
            .unwrap_or_default();
        web.set_media_permissions(media_permissions.iter().map(|((host, kind), granted)| (host.clone(), *kind, *granted)).collect());
        let active_space = store
            .as_ref()
            .and_then(|s| s.setting("active_space").ok().flatten())
            .map(|s| Hub::normalize_space_name(&s).unwrap_or_default())
            .unwrap_or_default();
        hub.update(|h| h.set_current_space(&active_space));
        let downloads_dir = download_dir_setting(store.as_ref().and_then(|s| s.setting("downloads_dir").ok().flatten()).as_deref());
        let downloads_ask = setting_opt_in(store.as_ref().and_then(|s| s.setting("downloads_ask").ok().flatten()).as_deref());
        web.set_downloads(downloads_dir.clone(), downloads_ask);
        let always_show_close = setting_on("always_show_close");
        let default_zoom = store
            .as_ref()
            .and_then(|s| s.setting("default_zoom").ok().flatten())
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|z| (0.5..=3.0).contains(z))
            .unwrap_or(1.0);
        let show_bottom_bar = setting_enabled(store.as_ref().and_then(|s| s.setting("bottom_bar").ok().flatten()).as_deref());
        let bookmarked_urls: HashSet<String> = store
            .as_ref()
            .and_then(|s| s.bookmarks().ok())
            .map(|list| list.into_iter().map(|b| b.url).collect())
            .unwrap_or_default();
        web.set_login_capture(login_save_enabled);
        let mut app = Self {
            hub,
            web: Rc::new(RefCell::new(web)),
            requests,
            store,
            focus,
            theme: Theme::for_choice(window.appearance(), theme_choice, accent_choice),
            theme_choice,
            accent_choice,
            last_web_dark: Some(web_dark),
            favicons: cached_favicons(),
            favicon_tried: HashMap::new(),
            favicon_pending: HashSet::new(),
            bookmarked_urls,
            cmd_held: false,
            show_tab_numbers,
            shortcuts: None,
            keymap,
            palette: None,
            palette_input_active: false,
            palette_metrics: Rc::default(),
            palette_new_tab: false,
            palette_private_tab: false,
            tab_menu: None,
            group_prompt: None,
            space_prompt: None,
            collapsed_groups,
            media_permissions,
            downloads_dir: downloads_dir.to_string_lossy().into_owned(),
            downloads_ask,
            settings_open: false,
            settings_tab: SettingsTab::General,
            settings_dropdown: None,
            ext_busy: false,
            ext_input: String::new(),
            ext_errors: std::rc::Rc::default(),
            blocking,
            hide_cookie_banners,
            login_fill_enabled,
            login_save_enabled,
            autofill_logins,
            tab_glyphs,
            compact_tabs,
            show_bookmarks_bar,
            show_address_bar,
            bookmarks_icons_only,
            bookmarks_new_tab_only,
            always_show_close,
            bookmarks_bar: vec![],
            default_zoom,
            show_bottom_bar,
            login_fill_prompt: None,
            login_save_prompt: None,
            declined_login_saves: HashSet::new(),
            agent_eval,
            shell_control,
            agent_widget,
            sleep_minutes,
            mcp_enabled,
            confirm_clear_history: false,
            confirm_clear_site_data: false,
            sources: Sources::default(),
            inspector_open: false,
            sidebar_tab: SidebarTab::Inspector,
            connect_open: false,
            connect_detected: vec![],
            connect_copied: None,
            downloads_open: false,
            downloads_selected: 0,
            download_poll_at: std::time::Instant::now(),
            reader_tabs: HashSet::new(),
            float_tabs: std::rc::Rc::default(),
            library: None,
            panel: Panel::Elements,
            tree: Tree::default(),
            tree_epoch: 0,
            inspector_cache: None,
            console_min: ConsoleLevel::Log,
            network_failed_only: false,
            selected_element: None,
            selected_network: None,
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
        // Closing the only window ends the process, as the owner expects from
        // a single-window browser. `quit` is deferred: the platform is still
        // inside its close decision and quitting re-enters window teardown.
        window.on_window_should_close(cx, |_window, cx| {
            if cx.windows().len() <= 1 {
                cx.defer(|cx| cx.quit());
            }
            true
        });
        app.start_pump(cx);
        app.refresh_start_page();
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
        let errors = self.ext_errors.clone();
        for (id, name, path) in store.enabled_extensions().unwrap_or_default() {
            let errors = errors.clone();
            let label = name.clone();
            self.web.borrow().load_extension(std::path::Path::new(&path), &id, move |r| {
                if let Err(e) = r {
                    let line = format!("{label}: {e}");
                    eprintln!("relay: extension {line}");
                    errors.borrow_mut().push(line);
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
                WebEvent::LoginFocus { tab, url, form_idx } => {
                    if self.login_fill_enabled {
                        // Only offer logins Relay wrote itself. Safari's or
                        // another manager's Keychain items are not ours to
                        // fill or delete.
                        let managed: HashSet<(String, String)> = self
                            .store
                            .as_ref()
                            .and_then(|s| {
                                let origin = relay_extensions::login::Origin::parse(&url).ok()?;
                                s.saved_logins(origin.host()).ok()
                            })
                            .unwrap_or_default()
                            .into_iter()
                            .collect();
                        let candidates: Vec<(String, String)> =
                            self.web.borrow().list_login_candidates(&url).into_iter().filter(|c| managed.contains(c)).collect();
                        if !candidates.is_empty() {
                            // One saved login is the common case: fill it now
                            // (the focus was a real native gesture) instead of
                            // asking again. Several logins still get the picker.
                            if self.autofill_logins && candidates.len() == 1 {
                                let (username, path) = candidates[0].clone();
                                if self.web.borrow().fill_login(tab, &url, form_idx, &username, &path).is_ok() {
                                    self.login_fill_prompt = None;
                                    continue;
                                }
                            }
                            self.login_fill_prompt = Some((tab, url, form_idx, candidates));
                        }
                    }
                }
                WebEvent::LoginFilled { tab, ok } => {
                    if self.login_fill_prompt.as_ref().is_some_and(|(t, _, _, _)| *t == tab) {
                        self.login_fill_prompt = None;
                    }
                    self.toast(if ok { "Password filled" } else { "Could not fill this form" });
                }
                WebEvent::SiteDataCleared => {
                    self.confirm_clear_site_data = false;
                    self.toast("Cookies and site data cleared");
                }
                WebEvent::MediaPermission { host, kind, granted } => {
                    self.media_permissions.insert((host, kind), granted);
                    self.persist_media_permissions();
                }
                WebEvent::DefaultBrowserSet { ok, error } => {
                    self.toast(if ok {
                        "Relay is now the default browser".to_string()
                    } else {
                        format!("Could not set the default browser: {}", error.unwrap_or_else(|| "macOS refused".into()))
                    });
                }
                WebEvent::LoginSaveOffer { tab, url, username } => {
                    let origin_key = relay_extensions::login::Origin::parse(&url).ok().map(|o| o.to_string());
                    let allowed = self.login_save_enabled
                        && origin_key
                            .as_ref()
                            .is_some_and(|ok| !self.declined_login_saves.contains(&(ok.clone(), username.clone())));
                    if allowed {
                        self.login_save_prompt = Some((tab, url, username));
                    } else {
                        self.web.borrow().decline_save_login(tab);
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
                        if !t.private {
                            if let (Some(store), Owner::User) = (&self.store, t.owner) {
                                let _ = store.record_visit(&t.url, &t.title, now_unix());
                                visited = true;
                            }
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
            // A new document invalidates any fill/save offer from the old one.
            if self.login_fill_prompt.as_ref().is_some_and(|(t, _, _, _)| *t == tab) {
                self.login_fill_prompt = None;
            }
            if self.login_save_prompt.as_ref().is_some_and(|(t, _, _)| *t == tab) {
                self.login_save_prompt = None;
                self.web.borrow().decline_save_login(tab);
            }
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
                    self.apply_shell_setting(&key, &value, cx);
                    shell_changed = true;
                }
                relay_bridge::ShellChange::Sidebar { open, tab } => {
                    self.inspector_open = open;
                    self.sidebar_tab = if tab == "agents" { SidebarTab::Agent } else { SidebarTab::Inspector };
                    shell_changed = true;
                }
                relay_bridge::ShellChange::Widget { title, body } => {
                    self.agent_widget = if title.is_empty() { None } else { Some((title, body)) };
                    shell_changed = true;
                }
            }
        }
        if shell_changed {
            self.title_cache = (u64::MAX, String::new());
            cx.notify();
        }
        if let Some(action) = self.web.borrow().take_media_key() {
            if let Some(tab) = self.active() {
                self.eval_op(tab, Op::Media { action: action.to_string() });
            }
        }
        if self.download_poll_at.elapsed() > Duration::from_millis(500) {
            self.download_poll_at = std::time::Instant::now();
            self.web.borrow().poll_downloads();
        }
        self.request_favicons(cx);
    }

    /// Fetch at most one icon per host from the page itself, after the page
    /// has a DOM to read. A failed attempt is retried on the next load, never
    /// on the next frame. Cross-origin icons without CORS fail closed to the
    /// letter/dot fallback.
    fn request_favicons(&mut self, cx: &mut Context<Self>) {
        let mut fetch: Vec<(TabId, String, u64)> = vec![];
        {
            let h = self.hub.lock();
            for t in h.tabs() {
                // Private tabs leave no icon on disk, so do not ask for one.
                if t.private || t.dom.root().is_none() || t.load_error.is_some() {
                    continue;
                }
                let Some(host) = host_of(&t.url) else { continue };
                let has_icon = matches!(self.favicons.get(&host), Some(Some(_)));
                if favicon_due(has_icon, t.load_seq, self.favicon_tried.get(&host).copied(), self.favicon_pending.contains(&host)) {
                    fetch.push((t.id, host, t.load_seq));
                }
            }
        }
        for (tab, host, seq) in fetch {
            let Ok((script, rx)) = self.hub.begin_command(tab, Op::Favicon) else { continue };
            self.favicon_pending.insert(host.clone());
            self.web.borrow_mut().apply(BrowserRequest::Eval { tab, script });
            cx.spawn(async move |this, cx| {
                let reply = futures_timeout(rx, Duration::from_secs(4), cx).await;
                let _ = this.update(cx, |app, cx| {
                    app.finish_favicon(host, seq, reply);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn finish_favicon(&mut self, host: String, seq: u64, reply: Option<serde_json::Value>) {
        self.favicon_pending.remove(&host);
        self.favicon_tried.insert(host.clone(), seq);
        let payload = reply.as_ref().and_then(favicon_payload);
        if let Some((bytes, ext)) = &payload {
            cache_favicon_file(&host, bytes, ext);
        }
        match payload.as_ref().and_then(|(bytes, ext)| favicon_decode(bytes, ext).map(Arc::new)) {
            Some(image) => {
                self.favicons.insert(host, Some(image));
            }
            None => {
                // A timeout or a page without an icon: keep a previously
                // decoded icon, otherwise show the monogram until a reload.
                self.favicons.entry(host).or_insert(None);
            }
        }
    }

    /// Which start-page sections are on, in their configured order.
    pub(super) fn start_sections(&self) -> Vec<Section> {
        let raw = self.store.as_ref().and_then(|s| s.setting("start_sections").ok().flatten());
        crate::startpage::parse_sections(raw.as_deref())
    }

    /// Show or hide one start-page section. `none` is written when the last
    /// one is switched off, so the page stays deliberately blank.
    pub(super) fn toggle_start_section(&mut self, section: Section, cx: &mut Context<Self>) {
        let mut sections = self.start_sections();
        if let Some(pos) = sections.iter().position(|s| *s == section) {
            sections.remove(pos);
        } else {
            sections.push(section);
            let order = Section::ALL;
            sections.sort_by_key(|s| order.iter().position(|x| x == s).unwrap_or(usize::MAX));
        }
        let value = if sections.is_empty() {
            "none".to_string()
        } else {
            sections.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")
        };
        if let Some(store) = &self.store {
            let _ = store.set_setting("start_sections", &value);
        }
        self.refresh_start_page();
        self.toast(format!("{}: {}", section.title(), if sections.contains(&section) { "shown" } else { "hidden" }));
        cx.notify();
    }

    /// Rebuild the new-tab cache after a profile change. Cheap: a few
    /// indexed SQLite reads, only on a visit/bookmark/app change.
    fn refresh_start_page(&mut self) {
        let Some(store) = &self.store else {
            self.start_page = StartPage::welcome();
            self.bookmarked_urls.clear();
            return;
        };
        let sections = store.setting("start_sections").ok().flatten();
        self.start_page = StartPage::build(store, sections.as_deref()).unwrap_or_default();
        let list = store.bookmarks().unwrap_or_default();
        self.bookmarked_urls = list.iter().map(|b| b.url.clone()).collect();
        self.bookmarks_bar = list.into_iter().take(16).map(|b| (b.title, b.url)).collect();
    }

    pub(super) fn active_is_bookmarked(&self) -> bool {
        let Some(active) = self.active() else { return false };
        self.hub
            .lock()
            .tab(active)
            .is_some_and(|t| self.bookmarked_urls.contains(&t.url))
    }

    pub(super) fn remove_bookmark_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            if self.bookmarked_urls.contains(url) {
                let _ = store.toggle_bookmark(url, "");
                self.refresh_start_page();
                self.toast("Bookmark removed");
                cx.notify();
            }
        }
    }

    pub(super) fn clear_loose_tabs(&mut self, cx: &mut Context<Self>) {
        let loose: Vec<TabId> = {
            let h = self.hub.lock();
            h.tabs()
                .filter(|t| t.owner == Owner::User && !t.pinned)
                .map(|t| t.id)
                .collect()
        };
        let count = loose.len();
        for id in loose {
            self.close_tab(id, cx);
        }
        if count > 0 {
            self.toast(if count == 1 { "Cleared 1 tab".to_string() } else { format!("Cleared {count} tabs") });
        }
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

    fn open_private_url(&mut self, url: String) {
        let tab = self.hub.update(|h| {
            let id = h.create_private_tab(&url);
            h.set_active(id);
            id
        });
        self.web.borrow_mut().apply(BrowserRequest::OpenPrivate { tab, url });
        self.focus_pending = Some(tab);
        self.try_focus_pending();
        self.toast("Private tab (no history, no cookies saved, hidden from agents)");
    }

    pub(super) fn duplicate_tab(&mut self, tab: TabId) {
        let info = self.hub.lock().tab(tab).map(|t| (t.url.clone(), t.private));
        if let Some((url, private)) = info {
            if private {
                self.open_private_url(url);
            } else {
                self.open_url(url, true);
            }
        }
        self.tab_menu = None;
    }

    pub(super) fn sleep_tab_now(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.tab_menu = None;
        if self.active() == Some(tab) {
            self.toast("Switch to another tab before sleeping this one");
            cx.notify();
            return;
        }
        self.web.borrow_mut().sleep(tab, (0.0, 0.0));
        self.toast("Tab put to sleep");
        cx.notify();
    }

    pub(super) fn close_other_tabs(&mut self, keep: TabId, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let others: Vec<TabId> = {
            let h = self.hub.lock();
            h.tabs()
                .filter(|t| t.id != keep && !t.pinned && t.owner == Owner::User)
                .map(|t| t.id)
                .collect()
        };
        let count = others.len();
        for id in others {
            self.close_tab(id, cx);
        }
        if count > 0 {
            self.toast(if count == 1 { "Closed 1 other tab".to_string() } else { format!("Closed {count} other tabs") });
        }
    }

    pub(super) fn toggle_pin_for(&mut self, tab: TabId) {
        self.tab_menu = None;
        if let Some(pinned) = self.hub.update(|h| h.set_pinned(tab, !h.tab(tab).is_some_and(|t| t.pinned))) {
            self.toast(if pinned { "Tab pinned to Favorites" } else { "Tab unpinned" });
        }
    }

    fn activate(&mut self, tab: TabId) {
        self.tab_menu = None;
        if let Some(space) = self.hub.lock().tab_space(tab).map(str::to_string) {
            if self.hub.lock().current_space() != space {
                self.hub.update(|h| h.set_current_space(&space));
                self.persist_active_space();
            }
        }
        if self.find.as_ref().is_some_and(|f| f.tab != tab) {
            self.find = None;
        }
        if self.login_fill_prompt.as_ref().is_some_and(|(t, _, _, _)| *t != tab) {
            self.login_fill_prompt = None;
        }
        if let Some((t, _, _)) = self.login_save_prompt.as_ref() {
            if *t != tab {
                let t = *t;
                self.login_save_prompt = None;
                self.web.borrow().decline_save_login(t);
            }
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
        if self.tab_menu == Some(tab) {
            self.tab_menu = None;
        }
        if self.hub.lock().tab(tab).is_some_and(|t| t.pinned) {
            self.put_pinned_away(tab, cx);
            return;
        }
        let last = self.hub.lock().tabs().count() <= 1;
        if let Some(t) = self.hub.update(|h| h.close_tab(tab)) {
            if !t.private && t.url.starts_with("http") {
                // Reopen history is a convenience, not a log; older entries
                // fall off so a long session cannot grow it without bound.
                const MAX_CLOSED: usize = 50;
                if self.closed.len() >= MAX_CLOSED {
                    self.closed.remove(0);
                }
                self.closed.push(t.url);
            }
        }
        if self.split == Some(tab) {
            self.split = None;
        }
        if self.hovered_tab == Some(tab) {
            self.hovered_tab = None;
        }
        if self.login_fill_prompt.as_ref().is_some_and(|(t, _, _, _)| *t == tab) {
            self.login_fill_prompt = None;
        }
        if self.login_save_prompt.as_ref().is_some_and(|(t, _, _)| *t == tab) {
            self.login_save_prompt = None;
            self.web.borrow().decline_save_login(tab);
        }
        self.snapshots.remove(&tab);
        self.compat_tabs.remove(&tab);
        self.web.borrow_mut().apply(BrowserRequest::Close(tab));
        let current = self.hub.lock().current_space().to_string();
        let next = self.active().filter(|next| self.hub.lock().tab_space(*next) == Some(current.as_str()));
        if let Some(next) = next {
            self.activate(next);
        } else {
            self.hub.update(|h| h.clear_active());
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
        // A per-site zoom wins; 1.0 means "never changed here", so the
        // owner's default applies instead of silently resetting the page.
        let zoom = if (prefs.zoom - 1.0).abs() < 1e-6 { self.default_zoom } else { prefs.zoom };
        self.web.borrow().set_zoom(tab, zoom);
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

    /// ⌘⌥B: hide or show the bottom status bar. A toast still reveals it for
    /// a moment, because the native page covers the shell.
    fn toggle_bottom_bar(&mut self) {
        self.show_bottom_bar = !self.show_bottom_bar;
        if let Some(store) = &self.store {
            let _ = store.set_setting("bottom_bar", if self.show_bottom_bar { "on" } else { "off" });
        }
        self.toast(if self.show_bottom_bar { "Bottom bar shown" } else { "Bottom bar hidden" });
    }

    pub(super) fn set_tab_strip_placement(&mut self, placement: tabstrip::Placement, cx: &mut Context<Self>) {
        if self.strip.placement == placement {
            return;
        }
        self.strip.placement = placement;
        self.persist_strip();
        self.toast(format!("Tab strip: {}", placement.label()));
        cx.notify();
    }

    pub(super) fn remove_extension(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            match store.remove_extension(id) {
                Ok(true) => {
                    if relay_extensions::crx::is_extension_id(id) {
                        let dir = relay_bridge::paths::relay_home().join("extensions").join(id);
                        let _ = std::fs::remove_dir_all(dir);
                    }
                    self.toast("Extension removed (takes effect on next launch)");
                }
                Ok(false) => self.toast("Extension not found"),
                Err(e) => self.toast(format!("Remove failed: {e}")),
            }
        }
        cx.notify();
    }

    pub(super) fn install_extension_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let raw = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        self.install_extension(&raw, cx);
    }

    pub(super) fn install_extension_from_input(&mut self, cx: &mut Context<Self>) {
        let raw = std::mem::take(&mut self.ext_input);
        self.install_extension(&raw, cx);
    }

    fn install_extension(&mut self, raw: &str, cx: &mut Context<Self>) {
        if self.ext_busy {
            self.toast("Extension install already in progress…");
            return;
        }
        let Some(id) = relay_extensions::crx::extract_extension_id(raw) else {
            self.toast("Enter a Chrome Web Store URL or 32-char extension ID");
            cx.notify();
            return;
        };
        self.ext_busy = true;
        self.toast(format!("Installing extension {id}…"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id_bg = id.clone();
            let res: Result<(String, String, std::path::PathBuf), String> = cx
                .background_executor()
                .spawn(async move {
                    let bytes = relay_extensions::crx::download(&id_bg).map_err(|e| e.to_string())?;
                    let dir = relay_extensions::crx::unpack(&bytes, &id_bg, &relay_bridge::paths::relay_home().join("extensions"))
                        .map_err(|e| e.to_string())?;
                    relay_extensions::manifest::install_shims(&dir).map_err(|e| e.to_string())?;
                    let m = relay_extensions::manifest::Manifest::read(&dir).map_err(|e| e.to_string())?;
                    let name = m.display_name(&dir);
                    Ok((name, m.version, dir))
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |app, cx| {
                    app.ext_busy = false;
                    match res {
                        Ok((name, version, dir)) => {
                            if let Some(store) = &app.store {
                                let _ = store.record_extension(&id, &name, &version, &dir);
                            }
                            let errors = app.ext_errors.clone();
                            let label = name.clone();
                            app.web.borrow().load_extension(&dir, &id, move |r| {
                                if let Err(e) = r {
                                    errors.borrow_mut().push(format!("{label}: {e}"));
                                }
                            });
                            app.toast(format!("Installed {name} {version}"));
                        }
                        Err(e) => app.toast(format!("Extension install failed: {e}")),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn import_chrome_extensions(&mut self, cx: &mut Context<Self>) {
        if self.ext_busy {
            self.toast("Extension import already in progress…");
            return;
        }
        let Some(profile) = relay_extensions::chrome_import::default_profile() else {
            self.toast("No Chrome-family profile found on this Mac");
            cx.notify();
            return;
        };
        let found = match relay_extensions::chrome_import::read_extensions(&profile) {
            Ok(list) if !list.is_empty() => list,
            Ok(_) => {
                self.toast("No Chrome extensions found to import");
                cx.notify();
                return;
            }
            Err(e) => {
                self.toast(format!("Could not read Chrome extensions: {e}"));
                cx.notify();
                return;
            }
        };
        let existing: HashSet<String> = self
            .store
            .as_ref()
            .and_then(|s| s.enabled_extensions().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        let to_install: Vec<String> = found
            .into_iter()
            .map(|e| e.id)
            .filter(|id| relay_extensions::crx::is_extension_id(id) && !existing.contains(id))
            .collect();
        if to_install.is_empty() {
            self.toast("All Chrome extensions are already imported");
            cx.notify();
            return;
        }
        self.ext_busy = true;
        self.toast(format!("Importing {} Chrome extension(s)…", to_install.len()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let results: Vec<Result<(String, String, String, std::path::PathBuf), String>> = cx
                .background_executor()
                .spawn(async move {
                    let root = relay_bridge::paths::relay_home().join("extensions");
                    to_install
                        .into_iter()
                        .map(|id| {
                            let bytes = relay_extensions::crx::download(&id).map_err(|e| e.to_string())?;
                            let dir = relay_extensions::crx::unpack(&bytes, &id, &root).map_err(|e| e.to_string())?;
                            relay_extensions::manifest::install_shims(&dir).map_err(|e| e.to_string())?;
                            let m = relay_extensions::manifest::Manifest::read(&dir).map_err(|e| e.to_string())?;
                            let name = m.display_name(&dir);
                            Ok((id, name, m.version, dir))
                        })
                        .collect()
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |app, cx| {
                    app.ext_busy = false;
                    let mut ok_count = 0usize;
                    let mut fail_count = 0usize;
                    for r in results {
                        match r {
                            Ok((id, name, version, dir)) => {
                                if let Some(store) = &app.store {
                                    let _ = store.record_extension(&id, &name, &version, &dir);
                                }
                                let errors = app.ext_errors.clone();
                                let label = name.clone();
                                app.web.borrow().load_extension(&dir, &id, move |r| {
                                    if let Err(e) = r {
                                        errors.borrow_mut().push(format!("{label}: {e}"));
                                    }
                                });
                                ok_count += 1;
                            }
                            Err(_) => fail_count += 1,
                        }
                    }
                    let msg = if fail_count == 0 {
                        format!("Imported {ok_count} Chrome extension(s)")
                    } else {
                        format!("Imported {ok_count} extension(s) ({fail_count} unavailable on Web Store)")
                    };
                    app.toast(msg);
                    cx.notify();
                });
            });
        })
        .detach();
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
            let reply = futures_timeout(rx, Duration::from_secs(15), cx).await;
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

    fn open_group_prompt(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let current = self.hub.lock().tab(tab).and_then(|t| t.group.clone()).unwrap_or_default();
        self.group_prompt = Some(GroupPrompt { tab, value: current });
        self.tab_menu = None;
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_group_prompt(&mut self) {
        self.group_prompt = None;
        self.focus_active_page();
    }

    fn open_space_prompt(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.tab_menu = None;
        self.group_prompt = None;
        self.space_prompt = Some(SpacePrompt { value: self.hub.lock().current_space().to_string() });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_space_prompt(&mut self) {
        self.space_prompt = None;
        self.focus_active_page();
    }

    fn save_space_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.space_prompt.take() else { return };
        let name = Hub::normalize_space_name(&prompt.value).unwrap_or_default();
        self.switch_space(name, cx);
    }

    /// Switch to a named space (empty is Default). Activates its first tab, or
    /// clears the active tab so the start page shows when the space is empty.
    fn switch_space(&mut self, name: String, cx: &mut Context<Self>) {
        let first = self.hub.update(|h| {
            h.set_current_space(&name);
            h.space_tab_ids().first().copied()
        });
        self.persist_active_space();
        if let Some(tab) = first {
            self.activate(tab);
        } else {
            self.hub.update(|h| h.clear_active());
        }
        let label = if name.is_empty() { "Default".to_string() } else { name };
        self.toast(format!("Space: {label}"));
        cx.notify();
    }

    fn save_group_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.group_prompt.take() else { return };
        let group = relay_bridge::hub::normalize_group_name(&prompt.value);
        self.hub.update(|h| h.set_tab_group(prompt.tab, group.as_deref()));
        match &group {
            Some(name) => self.toast(format!("Moved tab to {name}")),
            None => self.toast("Removed tab from its group"),
        }
        self.focus_active_page();
        cx.notify();
    }

    fn remove_tab_from_group(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.hub.update(|h| h.set_tab_group(tab, None));
        self.toast("Removed tab from its group");
        cx.notify();
    }

    fn set_group_collapsed(&mut self, name: &str, collapsed: bool, cx: &mut Context<Self>) {
        if collapsed {
            self.collapsed_groups.insert(name.to_string());
        } else {
            self.collapsed_groups.remove(name);
        }
        self.persist_collapsed_groups();
        cx.notify();
    }

    fn toggle_group_collapsed(&mut self, name: &str, cx: &mut Context<Self>) {
        let collapsed = !self.collapsed_groups.contains(name);
        self.set_group_collapsed(name, collapsed, cx);
    }

    fn fold_all_groups(&mut self, folded: bool, cx: &mut Context<Self>) {
        if folded {
            for name in self.hub.lock().tab_groups() {
                self.collapsed_groups.insert(name);
            }
        } else {
            self.collapsed_groups.clear();
        }
        self.persist_collapsed_groups();
        cx.notify();
    }

    fn persist_active_space(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("active_space", self.hub.lock().current_space());
        }
    }

    fn persist_downloads(&self) {
        let ask = self.downloads_ask;
        let dir = std::path::PathBuf::from(&self.downloads_dir);
        self.web.borrow_mut().set_downloads(dir, ask);
    }

    fn persist_media_permissions(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("media_permissions", &encode_media_permissions(&self.media_permissions));
        }
    }

    fn forget_media_permissions(&mut self, cx: &mut Context<Self>) {
        self.media_permissions.clear();
        self.web.borrow_mut().clear_media_permissions();
        if let Some(store) = &self.store {
            let _ = store.set_setting("media_permissions", "");
        }
        self.toast("Forgot camera and microphone choices");
        cx.notify();
    }

    fn persist_collapsed_groups(&self) {
        if let Some(store) = &self.store {
            let mut names: Vec<&String> = self.collapsed_groups.iter().collect();
            names.sort();
            let value = names.into_iter().map(String::as_str).collect::<Vec<_>>().join(",");
            let _ = store.set_setting("collapsed_groups", &value);
        }
    }

    /// Mute / unmute every media element in a tab. JS reports the page's own
    /// current state, so the native side needs no duplicate media model.
    /// Float the active page's first video in a fixed corner overlay.
    fn toggle_float_video(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else { return };
        let on = self
            .float_tabs
            .borrow()
            .get(&tab)
            .copied()
            .map(|on| !on)
            .unwrap_or(true);
        self.float_tabs.borrow_mut().insert(tab, on);
        self.eval_op(tab, Op::FloatVideo { on });
        self.toast(if on { "Video floated" } else { "Video restored" });
        cx.notify();
    }

    fn toggle_mute(&mut self, tab: TabId) {
        self.eval_op(tab, Op::Mute { muted: None });
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
            Ok(true) => self.refresh_start_page(),
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
        self.refresh_start_page();
        self.refresh_library();
        cx.notify();
    }

    fn open_find(&mut self) {
        let Some(tab) = self.active() else { return };
        self.palette = None;
        self.library = None;
        self.connect_open = false;
        self.end_hints();
        let (query, case_sensitive) = self.find.take().map(|f| (f.query, f.case_sensitive)).unwrap_or_default();
        self.find = Some(FindBar { tab, query, found: None, case_sensitive });
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
        self.web.borrow().find(f.tab, &f.query, backwards, f.case_sensitive);
    }

    fn find_step(&mut self, backwards: bool) {
        if self.find.is_none() {
            self.open_find();
            return;
        }
        self.run_find(backwards);
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
            (h.space_tab_ids(), h.active())
        };
        if ids.is_empty() {
            return;
        }
        let pos = active.and_then(|a| ids.iter().position(|t| *t == a)).unwrap_or(0) as isize;
        let next = ids[((pos + delta).rem_euclid(ids.len() as isize)) as usize];
        self.activate(next);
    }

    fn select_index(&mut self, i: usize) {
        let ids = self.hub.lock().space_tab_ids();
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
        self.palette_private_tab = false;
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
        let mut p = Palette { custom_bangs: self.custom_bangs.clone(), ..Default::default() };
        p.set_query(prefill.unwrap_or_default());
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
        self.settings_dropdown = None;
        self.confirm_clear_history = false;
        self.focus_active_page();
    }

    fn run_item(&mut self, item: Item, cx: &mut Context<Self>) {
        let private = std::mem::take(&mut self.palette_private_tab);
        let new_tab = self.palette_new_tab || self.active().is_none();
        match item {
            Item::Open(url) | Item::Search { url, .. } | Item::Bang { url, .. } | Item::History { url, .. } | Item::Bookmark { url, .. } => {
                if private {
                    self.open_private_url(url);
                } else {
                    self.open_url(url, new_tab);
                }
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
                self.hub.set_sidebar(self.inspector_open, self.sidebar_tab.as_str());
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
            Command::NewPrivateTab => {
                if self.app_mode {
                    self.toast("App windows are single-tab");
                } else {
                    self.open_palette(None, true);
                    self.palette_private_tab = true;
                }
            }
            Command::Print => {
                if let Some(t) = active {
                    self.web.borrow_mut().apply(BrowserRequest::Print(t));
                } else {
                    self.toast("No page to print");
                }
            }
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
            (h.space_tab_ids(), h.active())
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
            self.toast("No Chrome-family profile found");
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
        if self.space_prompt.is_some() {
            match k.key.as_str() {
                "escape" => self.close_space_prompt(),
                "enter" => self.save_space_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.space_prompt.as_mut() {
                        if k.modifiers.secondary() || k.modifiers.alt {
                            prompt.value.clear();
                        } else {
                            prompt.value.pop();
                        }
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(prompt) = self.space_prompt.as_mut() {
                            if prompt.value.chars().count() < 24 {
                                prompt.value.push_str(c);
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.group_prompt.is_some() {
            match k.key.as_str() {
                "escape" => self.close_group_prompt(),
                "enter" => self.save_group_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.group_prompt.as_mut() {
                        if k.modifiers.secondary() || k.modifiers.alt {
                            prompt.value.clear();
                        } else {
                            prompt.value.pop();
                        }
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(prompt) = self.group_prompt.as_mut() {
                            if prompt.value.chars().count() < 40 {
                                prompt.value.push_str(c);
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
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
            } else {
                match k.key.as_str() {
                    "escape" => {
                        let has_query = self.shortcuts.as_ref().is_some_and(|s| !s.query.is_empty());
                        if has_query {
                            if let Some(editor) = self.shortcuts.as_mut() {
                                editor.query.clear();
                            }
                        } else {
                            self.close_shortcuts(cx);
                        }
                    }
                    "backspace" => {
                        if let Some(editor) = self.shortcuts.as_mut() {
                            if k.modifiers.secondary() || k.modifiers.alt {
                                editor.query.clear();
                            } else {
                                editor.query.pop();
                            }
                        }
                    }
                    _ => {
                        if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.modified()) {
                            if let Some(editor) = self.shortcuts.as_mut() {
                                editor.query.push_str(&c.to_lowercase());
                            }
                        }
                    }
                }
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
            let len = self.hub.lock().downloads_in_panel_order().len();
            self.downloads_selected = self.downloads_selected.min(len.saturating_sub(1));
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
            match k.key.as_str() {
                "escape" => self.close_settings(),
                _ if self.settings_tab == SettingsTab::Extensions => match k.key.as_str() {
                    "enter" => self.install_extension_from_input(cx),
                    "backspace" => {
                        if k.modifiers.secondary() || k.modifiers.alt {
                            self.ext_input.clear();
                        } else {
                            self.ext_input.pop();
                        }
                    }
                    _ => {
                        if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                            if self.ext_input.chars().count() < 300 {
                                self.ext_input.push_str(c);
                            }
                        }
                    }
                },
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
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
        let input_active = self.palette_input_active;
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
            "right" => {
                if p.cursor == p.query.len() && p.selection().is_none() && p.accept_completion() {
                    p.refresh(&self.sources, &self.search);
                } else {
                    p.move_cursor_ext(1, k.modifiers.shift, k.modifiers.alt);
                }
            }
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
                // With the platform input handler installed, printable keys
                // must reach NSTextInputClient so composition works; inserting
                // key_char here would both swallow the IME and double-insert
                // the committed text.
                if input_active {
                    return;
                }
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

    fn apply_shell_setting(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        match key {
            "search" => self.search = value.to_string(),
            "cookie_banners" => {
                self.hide_cookie_banners = setting_enabled(Some(value));
                if self.blocking {
                    self.web.borrow().load_content_rules(&rules_json(self.store.as_ref()));
                }
            }
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
            "bottom_bar" => self.show_bottom_bar = setting_enabled(Some(value)),
            "autofill_logins" => self.autofill_logins = setting_enabled(Some(value)),
            "tab_glyphs" => self.tab_glyphs = setting_enabled(Some(value)),
            "compact_tabs" => self.compact_tabs = setting_enabled(Some(value)),
            "bookmarks_bar" => self.show_bookmarks_bar = setting_enabled(Some(value)),
            "address_bar" => self.show_address_bar = setting_enabled(Some(value)),
            "bookmarks_icons_only" => self.bookmarks_icons_only = setting_enabled(Some(value)),
            "bookmarks_new_tab_only" => self.bookmarks_new_tab_only = setting_enabled(Some(value)),
            "always_show_close" => self.always_show_close = setting_enabled(Some(value)),
            "default_zoom" => {
                if let Ok(z) = value.parse::<f64>() {
                    if (0.5..=3.0).contains(&z) {
                        self.default_zoom = z;
                    }
                }
            }
            "login_offers" => {
                let on = setting_enabled(Some(value));
                self.login_fill_enabled = on;
                self.login_save_enabled = on;
                self.web.borrow_mut().set_login_capture(on);
            }
            "login_fill" => self.login_fill_enabled = setting_enabled(Some(value)),
            "login_save" => {
                self.login_save_enabled = setting_enabled(Some(value));
                self.web.borrow_mut().set_login_capture(self.login_save_enabled);
            }
            "restore_session" => self.session_enabled = setting_enabled(Some(value)),
            "keybindings" => {
                self.keymap = Keymap::from_setting(Some(value));
                // Rebinding menus and GPUI keys is what makes the shortcut
                // fire now, not after a restart.
                self.apply_keymap(cx);
            }
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
        let Some(d) = self.hub.lock().downloads_in_panel_order().into_iter().nth(self.downloads_selected) else { return };
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
        let mut clients: Vec<(mcp_config::Client, bool)> = mcp_config::Client::ALL.iter().map(|&c| (c, c.is_installed())).collect();
        clients.sort_by_key(|(_, detected)| !*detected);
        self.connect_detected = clients;
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
        self.shortcuts = Some(ShortcutEditor { recording: None, query: String::new() });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts.as_ref().is_some_and(|s| s.recording.is_some()) {
            // Recording cleared the bindings and menus; restore both, not
            // only the saved setting, or every shortcut stays dead.
            self.apply_keymap(cx);
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
        let (bindings, menus) = live_keymap(&self.keymap, self.app_name.as_deref());
        cx.clear_key_bindings();
        cx.bind_keys(bindings);
        cx.set_menus(menus);
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
        let result = self.keymap.set(id, key);
        if let Some(editor) = self.shortcuts.as_mut() {
            editor.recording = None;
        }
        // Always restore the bindings and the menu bar, also after a refused
        // key: a failed rebind must never leave the app with no shortcuts.
        self.apply_keymap(cx);
        match result {
            Ok(()) => self.toast(format!("{}: {}", id.label(), keymap::shortcut_label(key))),
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

}


impl Render for RelayApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.palette_input_active = false;
        if self.palette.is_some() && !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let web_dark = match self.theme_choice {
            ThemeChoice::System => None,
            ThemeChoice::Light => Some(false),
            ThemeChoice::Dark => Some(true),
        };
        if self.last_web_dark != Some(web_dark) {
            self.last_web_dark = Some(web_dark);
            self.web.borrow_mut().set_appearance(web_dark);
        }
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
            let mut column = div().flex_1().h_full().flex().flex_col().overflow_hidden();
            if self.show_address_bar && !self.app_mode {
                column = column.child(self.render_address_bar(cx));
            }
            if bookmarks_bar_shown(self.show_bookmarks_bar, self.bookmarks_new_tab_only, self.active().is_some()) && !self.app_mode {
                column = column.child(self.render_bookmarks_bar(cx));
            }
            column.child(self.render_content(cx)).into_any_element()
        };
        // A web app window starts chromeless; a popup tab brings the strip
        // back rather than hiding a tab the user cannot reach.
        let show_strip = !self.app_mode || self.hub.lock().tabs().count() > 1;
        let mut main = div().flex_1().overflow_hidden();
        if show_strip && !self.strip.folded {
            main = match self.strip.placement {
                tabstrip::Placement::Top => main.flex().flex_col().child(self.render_tab_bar(cx)).child(content),
                tabstrip::Placement::Left => main.flex().child(self.render_tab_bar(cx)).child(content),
                tabstrip::Placement::Right => main.flex().child(content).child(self.render_tab_bar(cx)),
            };
        } else {
            main = main.flex().child(content);
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
            .on_modifiers_changed(cx.listener(|app, event: &ModifiersChangedEvent, _, cx| {
                let held = event.modifiers.platform;
                if app.cmd_held != held {
                    app.cmd_held = held;
                    cx.notify();
                }
            }))
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
            .on_action(cx.listener(|app, _: &FindNext, _, cx| {
                app.find_step(false);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &FindPrevious, _, cx| {
                app.find_step(true);
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
            .on_action(cx.listener(|app, _: &ToggleFloatVideo, _, cx| {
                app.toggle_float_video(cx);
            }))
            .on_action(cx.listener(|app, _: &ShowDownloads, _, cx| {
                app.open_downloads(cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleBottomBar, _, cx| {
                app.toggle_bottom_bar();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &TabsOnLeft, _, cx| {
                app.set_tab_strip_placement(tabstrip::Placement::Left, cx);
            }))
            .on_action(cx.listener(|app, _: &TabsOnTop, _, cx| {
                app.set_tab_strip_placement(tabstrip::Placement::Top, cx);
            }))
            .on_action(cx.listener(|app, _: &TabsOnRight, _, cx| {
                app.set_tab_strip_placement(tabstrip::Placement::Right, cx);
            }))
            .on_action(cx.listener(|app, _: &FoldAllGroups, _, cx| {
                app.fold_all_groups(true, cx);
            }))
            .on_action(cx.listener(|app, _: &UnfoldAllGroups, _, cx| {
                app.fold_all_groups(false, cx);
            }))
            .on_action(cx.listener(|app, _: &Spaces, _, cx| {
                app.open_space_prompt(cx);
            }))
            .on_action(cx.listener(|app, _: &NewPrivateTab, _, cx| {
                app.run_command(Command::NewPrivateTab, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &Print, _, cx| {
                app.run_command(Command::Print, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ExportPdf, _, cx| {
                if let Some(tab) = app.active() {
                    app.web.borrow().export_pdf(tab);
                } else {
                    app.toast("Open a page to export it");
                }
                cx.notify();
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
        } else if self.challenge_handoff().is_some() {
            root = root.child(self.render_challenge_banner(cx));
        }
        if let Some(banner) = self.render_login_fill_banner(cx) {
            root = root.child(banner);
        }
        if let Some(banner) = self.render_login_save_banner(cx) {
            root = root.child(banner);
        }
        // A hidden bar still appears for a toast or link hints: those messages
        // have nowhere else to go while the native page covers the shell.
        if (!self.app_mode && self.show_bottom_bar) || self.toast.is_some() || self.hints.is_some() {
            root = root.child(self.render_status(cx));
        }
        // The folded-strip pill is a small overlay over the page, not part of
        // the layout: show it on a page too, or hiding the bottom bar leaves
        // ⌘S as the only way back.
        if show_strip && self.strip.folded && !overlay_open && !self.app_mode {
            root = root.child(self.render_folded_strip(cx));
        }
        if self.strip.placement == tabstrip::Placement::Top && self.tab_menu.is_some() && !overlay_open {
            if let Some(menu) = self.render_floating_tab_menu(cx) {
                root = root.child(menu);
            }
        }
        if let Some(f) = &self.find {
            root = root.child(self.render_find(f, cx));
        }
        if let Some(p) = &self.palette {
            root = root.child(self.render_palette(p, cx));
        }
        if let Some(prompt) = &self.group_prompt {
            root = root.child(self.render_group_prompt(prompt, cx));
        }
        if let Some(prompt) = &self.space_prompt {
            root = root.child(self.render_space_prompt(prompt, cx));
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
        .filter(|t| t.owner == Owner::User && !t.private && is_http_url(&t.url))
        .enumerate()
        .map(|(i, t)| SessionTab {
            url: t.url.clone(),
            title: t.title.clone(),
            pinned: t.pinned,
            scroll_y: if t.sleeping { slept_scroll_y(t.id).unwrap_or(0.0) } else { 0.0 },
            active: Some(t.id) == active,
            position: i as i64,
            group: t.group.clone().unwrap_or_default(),
            space: t.space.clone(),
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
            t.group = relay_bridge::hub::normalize_group_name(&s.group);
            t.space = Hub::normalize_space_name(&s.space).unwrap_or_default();
        }
        if s.active {
            active = Some(id);
        }
        restored.push((id, s.url.clone(), s.scroll_y));
    }
    let active = active.or_else(|| restored.first().map(|r| r.0));
    if let Some(id) = active {
        hub.set_active(id);
        if let Some(space) = hub.tab_space(id).map(str::to_string) {
            hub.set_current_space(&space);
        }
    }
    (restored, active)
}

/// Which Downloads-panel section a row belongs to. The hub returns rows
/// already in this order, so the panel inserts a heading only when the group
/// changes and keyboard selection follows the same sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadGroup {
    Active,
    Finished,
}

impl DownloadGroup {
    fn of(state: &relay_bridge::DownloadState) -> Self {
        match state {
            relay_bridge::DownloadState::Downloading => Self::Active,
            _ => Self::Finished,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Finished => "Finished",
        }
    }
}

/// Show a destination path the way the row prints it: the home directory
/// becomes `~`. The row ellipsizes anything longer, so the panel's width does
/// not depend on the path.
fn shorten_home(path: &str, home: &str) -> String {
    if !home.is_empty() && path == home {
        return "~".into();
    }
    if !home.is_empty() && path.strip_prefix(home).is_some_and(|rest| rest.starts_with('/')) {
        return format!("~{}", &path[home.len()..]);
    }
    path.to_string()
}

fn display_path(path: &str) -> String {
    shorten_home(path, &std::env::var("HOME").unwrap_or_default())
}

fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Parse the comma-separated `collapsed_groups` setting. Names cannot contain
/// commas (the hub normalizer maps them to spaces), so splitting is stable.
/// Expand `~/...` and fall back to `~/Downloads/Relay` for an empty/invalid
/// setting.
pub(crate) fn download_dir_setting(raw: Option<&str>) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
    let default = relay_webkit::downloads::folder();
    let Some(raw) = raw.map(str::trim).filter(|v| !v.is_empty()) else { return default };
    if raw == "~" {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = std::path::PathBuf::from(raw);
    if path.is_absolute() { path } else { default }
}

/// `host|kind|1;host|kind|0`. Hosts cannot contain `|`; kind is WebKit's
/// capture enum (0 camera, 1 microphone, 2 both).
pub(crate) fn parse_media_permissions(raw: &str) -> HashMap<(String, i64), bool> {
    let mut out = HashMap::new();
    for entry in raw.split(';') {
        let mut parts = entry.split('|');
        let (Some(host), Some(kind), Some(value)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(kind), Some(granted)) = (kind.parse::<i64>(), match value { "1" => Some(true), "0" => Some(false), _ => None }) else {
            continue;
        };
        if !host.is_empty() {
            out.insert((host.to_string(), kind), granted);
        }
    }
    out
}

pub(crate) fn encode_media_permissions(map: &HashMap<(String, i64), bool>) -> String {
    let mut entries: Vec<((&String, i64), bool)> = map.iter().map(|(k, v)| ((&k.0, k.1), *v)).collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
        .into_iter()
        .map(|((host, kind), granted)| format!("{host}|{kind}|{}", i32::from(granted)))
        .collect::<Vec<_>>()
        .join(";")
}

pub(crate) fn collapsed_group_names(raw: Option<&str>) -> HashSet<String> {
    raw.unwrap_or("").split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect()
}

/// Opt-in switches are off when the key is absent, unlike toggles that have
/// historically defaulted on.
/// The bookmarks bar is shown when enabled, and optionally only with no
/// active tab (the new-tab page).
pub(crate) fn bookmarks_bar_shown(enabled: bool, new_tab_only: bool, has_tab: bool) -> bool {
    enabled && (!new_tab_only || !has_tab)
}

pub(crate) fn setting_opt_in(value: Option<&str>) -> bool {
    value.is_some_and(|v| setting_enabled(Some(v)))
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

fn shell_grant_action(granted: bool) -> &'static str {
    if granted { "Revoke" } else { "Grant" }
}

/// The binding and menu set GPUI installs for a keymap. One owner keeps the
/// Settings editor and an agent-applied `keybindings` change in step.
fn live_keymap(keymap: &Keymap, app_name: Option<&str>) -> (Vec<KeyBinding>, Vec<Menu>) {
    (keymap.bindings(), app_menus_named(app_name))
}

fn shell_write_summary(global: bool, granted: usize) -> String {
    if global {
        "on (all clients)".into()
    } else if granted == 0 {
        "off".into()
    } else {
        format!("{granted} client{} granted", if granted == 1 { "" } else { "s" })
    }
}

/// Built-in list plus the user's `relay_home()/filters.txt` (EasyList syntax).
/// The content-blocker JSON: built-in + cookie-banner + `filters.txt` rules,
/// then an exception for every site the user turned blocking off on.
fn rules_json(store: Option<&Store>) -> String {
    let hide_banners = setting_enabled(store.and_then(|s| s.setting("cookie_banners").ok().flatten()).as_deref());
    let mut rules = relay_webkit::content_rules::compile(&blocklist(hide_banners));
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

fn blocklist(hide_cookie_banners: bool) -> String {
    let mut s = relay_webkit::content_rules::BUILTIN_FILTERS.to_string();
    if hide_cookie_banners {
        s.push_str(relay_webkit::content_rules::CMP_FILTERS);
    }
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

/// Whether a host still needs an icon request: not cached, not in flight,
/// and the page has not already answered for this load.
fn favicon_due(has_icon: bool, load_seq: u64, tried_seq: Option<u64>, pending: bool) -> bool {
    !has_icon && !pending && tried_seq.is_none_or(|tried| load_seq > tried)
}

/// Decode the bridge's untrusted `favicon` reply. PNG/JPEG/WebP/GIF/SVG and
/// the PNG payload of an ICO under 256 KB are accepted; the data URL prefix
/// and the mime must agree, and the bytes must carry the format's signature
/// before they reach the renderer.
fn favicon_payload(value: &serde_json::Value) -> Option<(Vec<u8>, &'static str)> {
    if value.get("available")?.as_bool()? != true {
        return None;
    }
    let data = value.get("data")?.as_str()?;
    let mime = value.get("mime")?.as_str()?;
    let mut ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        _ => return None,
    };
    let prefix = format!("data:{mime};base64,");
    let payload = data.strip_prefix(&prefix)?;
    if payload.len() > 256 * 1024 * 4 / 3 {
        return None;
    }
    let mut bytes = webapp::base64_decode(payload)?;
    if bytes.is_empty() || bytes.len() > 256 * 1024 || !favicon_signature(&bytes, ext) {
        return None;
    }
    if ext == "ico" {
        bytes = ico_png(&bytes)?;
        ext = "png";
    }
    if !favicon_pixels_ok(&bytes, ext) {
        return None;
    }
    (bytes.len() <= 256 * 1024).then_some((bytes, ext))
}

/// Reject decompression bombs before the renderer allocates pixels: the
/// compressed cap alone allows a 256 KB PNG to decode into gigabytes.
fn favicon_pixels_ok(bytes: &[u8], ext: &str) -> bool {
    let Some((w, h)) = favicon_dimensions(bytes, ext) else {
        // SVG without explicit dimensions is sized by the view box and safe.
        return ext == "svg" && bytes.len() <= 256 * 1024;
    };
    w > 0 && h > 0 && w <= 1024 && h <= 1024 && u64::from(w) * u64::from(h) <= 2 * 1024 * 1024
}

fn favicon_dimensions(bytes: &[u8], ext: &str) -> Option<(u32, u32)> {
    match ext {
        "png" if bytes.len() >= 24 => {
            Some((u32::from_be_bytes(bytes[16..20].try_into().ok()?), u32::from_be_bytes(bytes[20..24].try_into().ok()?)))
        }
        "gif" if bytes.len() >= 10 => Some((u16::from_le_bytes([bytes[6], bytes[7]]) as u32, u16::from_le_bytes([bytes[8], bytes[9]]) as u32)),
        "jpg" => {
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xff {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                    let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                    return Some((w, h));
                }
                let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                if len < 2 {
                    return None;
                }
                i = i.checked_add(2 + len)?;
            }
            None
        }
        "webp" if bytes.len() >= 30 => match &bytes[12..16] {
            b"VP8X" => {
                let w = 1 + u32::from_le_bytes([0, bytes[24], bytes[25], bytes[26]]);
                let h = 1 + u32::from_le_bytes([0, bytes[27], bytes[28], bytes[29]]);
                Some((w, h))
            }
            b"VP8L" => {
                let b = &bytes[21..25];
                let w = 1 + ((u32::from(b[1]) & 0x3f) << 8 | u32::from(b[0]));
                let h = 1 + ((u32::from(b[3]) & 0x0f) << 10 | u32::from(b[2]) << 2 | (u32::from(b[1]) & 0xc0) >> 6);
                Some((w, h))
            }
            b"VP8 " => {
                let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]])) & 0x3fff;
                let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]])) & 0x3fff;
                Some((w, h))
            }
            _ => None,
        },
        "svg" => {
            let text = std::str::from_utf8(&bytes[..bytes.len().min(4096)]).ok()?.to_ascii_lowercase();
            let attr = |name: &str| -> Option<u32> {
                let start = text.find(name)? + name.len();
                let rest = &text[start..];
                let value = rest.split('"').nth(1).or_else(|| rest.split('\'').nth(1))?;
                value.trim().trim_end_matches("px").parse().ok()
            };
            match (attr("width="), attr("height=")) {
                (Some(w), Some(h)) => Some((w, h)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn favicon_signature(bytes: &[u8], ext: &str) -> bool {
    match ext {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "svg" => std::str::from_utf8(&bytes[..bytes.len().min(4096)])
            .is_ok_and(|text| text.trim_start_matches(['\u{feff}', ' ', '\n', '\r', '\t']).to_ascii_lowercase().contains("<svg")),
        "ico" => bytes.len() >= 6 && bytes[0] == 0 && bytes[1] == 0 && bytes[2] == 1 && bytes[3] == 0,
        _ => false,
    }
}

/// Pull the largest PNG-compressed image out of an ICO container. Old BMP
/// entries are skipped rather than converted; most current sites ship PNG.
fn ico_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if !favicon_signature(bytes, "ico") {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || count > 64 || bytes.len() < 6 + count * 16 {
        return None;
    }
    let mut best: Option<(u32, usize, usize)> = None;
    for i in 0..count {
        let entry = 6 + i * 16;
        let width = match bytes[entry] {
            0 => 256,
            w => u32::from(w),
        };
        let height = match bytes[entry + 1] {
            0 => 256,
            h => u32::from(h),
        };
        let size = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(bytes[entry + 12..entry + 16].try_into().ok()?) as usize;
        let end = offset.checked_add(size)?;
        if size < 8 || end > bytes.len() || !bytes[offset..end].starts_with(b"\x89PNG\r\n\x1a\n") {
            continue;
        }
        let area = width.saturating_mul(height);
        if best.is_none_or(|(best_area, _, _)| area > best_area) {
            best = Some((area, offset, size));
        }
    }
    best.map(|(_, offset, size)| bytes[offset..offset + size].to_vec())
}

fn favicon_decode(bytes: &[u8], ext: &str) -> Option<Image> {
    if !favicon_signature(bytes, ext) {
        return None;
    }
    let format = match ext {
        "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        "svg" => ImageFormat::Svg,
        _ => ImageFormat::Png,
    };
    Some(Image::from_bytes(format, bytes.to_vec()))
}

#[cfg_attr(not(test), allow(dead_code))]
fn favicon_image(value: &serde_json::Value) -> Option<Image> {
    let (bytes, ext) = favicon_payload(value)?;
    favicon_decode(&bytes, ext)
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
        if !matches!(ext.as_str(), "png" | "jpg" | "webp" | "gif" | "svg") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() > 256 * 1024 {
            continue;
        }
        if let Some(image) = favicon_decode(&bytes, &ext) {
            out.insert(stem.to_string(), Some(Arc::new(image)));
        }
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

