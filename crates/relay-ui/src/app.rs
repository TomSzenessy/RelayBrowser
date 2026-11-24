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
use crate::theme::{self, AccentChoice, Theme, ThemeChoice};
use crate::web::{EditAction, WebEvent, WebHost};
use crate::webapp;
use gpui::{
    actions, canvas, div, fill, img, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, DispatchPhase,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, GlobalElementId, Hsla, Image, ImageFormat,
    ease_out_quint, Animation, AnimationExt, InspectorElementId, KeyBinding, KeyDownEvent, LayoutId, Menu, MenuItem,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, ObjectFit, PathPromptOptions, Pixels,
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
        ToggleFloatPanel,
        AskAi,
        ShowTabSwitcher,
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

/// The small modal that names/colours a tab group (for one tab, or an entire group when `tab` is None).
struct GroupPrompt {
    tab: Option<TabId>,
    original_group: Option<String>,
    value: String,
    color: Option<theme::GroupColor>,
}

/// The small modal that names, picks, or styles a space.
struct SpacePrompt {
    value: String,
    confirm_delete: bool,
    look: theme::SpaceLook,
}

/// Quick MRU / strip-order tab switcher modal.
struct TabSwitcher {
    tabs: Vec<(TabId, String, String)>,
    selected: usize,
}

/// Placement of the compact URL/status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusBarPosition {
    Top,
    Bottom,
    #[default]
    Hidden,
}

impl StatusBarPosition {
    pub(crate) const ALL: [StatusBarPosition; 3] = [Self::Top, Self::Bottom, Self::Hidden];

    pub(crate) fn parse(raw: Option<&str>, legacy_bottom_bar: Option<&str>) -> Self {
        if let Some(value) = raw {
            match value.trim().to_ascii_lowercase().as_str() {
                "top" => return Self::Top,
                "bottom" => return Self::Bottom,
                "hidden" | "off" | "none" => return Self::Hidden,
                _ => {}
            }
        }
        if let Some(legacy) = legacy_bottom_bar {
            return if setting_enabled(Some(legacy)) { Self::Bottom } else { Self::Hidden };
        }
        Self::Hidden
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Hidden => "hidden",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Hidden => "Hidden",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Top,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Hidden,
        }
    }
}

/// What closing the last unpinned tab does in normal browser mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LastTabClose {
    #[default]
    StartPage,
    Quit,
}

impl LastTabClose {
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("quit") => Self::Quit,
            _ => Self::StartPage,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StartPage => "start_page",
            Self::Quit => "quit",
        }
    }
}

/// Order used by Ctrl+Tab / Ctrl+Shift+Tab and the visual switcher (⌘⌥T).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TabSwitchOrder {
    #[default]
    Mru,
    Strip,
}

impl TabSwitchOrder {
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("strip") => Self::Strip,
            _ => Self::Mru,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mru => "mru",
            Self::Strip => "strip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseConfirmKind {
    Quit,
    CloseOthers(TabId),
    ClearLoose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CloseConfirm {
    pub(super) kind: CloseConfirmKind,
    pub(super) count: usize,
}

struct PendingAi {
    prompt: String,
    history: Vec<(bool, String)>,
}

struct AiChat {
    messages: Vec<(bool, String)>,
    input: String,
    busy: bool,
    error: Option<String>,
    /// Set while visible page text is being collected for the next question.
    pending: Option<PendingAi>,
    pending_at: Option<Instant>,
}

/// The modal that names a profile or picks an existing one.
struct ProfilePrompt {
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
    Ai,
    About,
}

impl SettingsTab {
    pub(super) const ALL: [SettingsTab; 11] = [
        SettingsTab::General,
        SettingsTab::Appearance,
        SettingsTab::Tabs,
        SettingsTab::Browsing,
        SettingsTab::Passwords,
        SettingsTab::Privacy,
        SettingsTab::Extensions,
        SettingsTab::Agents,
        SettingsTab::Shortcuts,
        SettingsTab::Ai,
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
            SettingsTab::Ai => "AI",
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
    /// Blank-tab background preset or validated local wallpaper path.
    start_bg: theme::StartBackground,
    /// Decoded wallpaper image when `start_bg` is `StartBackground::Image`.
    start_bg_image: Option<Arc<Image>>,
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
    /// User-chosen colours per group name; persisted as `group_colors` JSON.
    group_colors: std::collections::BTreeMap<String, theme::GroupColor>,
    /// User-chosen colours/gradients/icons per space; persisted as `space_looks` JSON.
    space_looks: std::collections::BTreeMap<String, theme::SpaceLook>,
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
    /// Placement of the compact status bar (Top, Bottom, or Hidden; ⌘⌥B cycles).
    status_bar_position: StatusBarPosition,
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
    /// Vertical strip shows only site icons in a narrow rail.
    tab_icons_only: bool,
    /// Show enabled extensions as a compact toolbar.
    extension_toolbar: bool,
    /// Cached action icons from WebKit, keyed by extension id.
    ext_icons: HashMap<String, Arc<Image>>,
    /// Cached action badge text and unread state, keyed by enabled extension id.
    ext_badges: HashMap<String, relay_extensions::ExtensionBadge>,
    /// Optional shared folder for a merge-on-launch profile sync.
    sync_folder: String,
    /// Advertise the active page for Handoff (opt-in).
    handoff: bool,
    ai_endpoint: String,
    ai_model: String,
    ai_key_draft: String,
    ai_focus: u8,
    ai_has_key: bool,
    /// Send visible page text as AI context (opt-out privacy switch).
    ai_page_context: bool,
    ai_chat: Option<AiChat>,
    /// Whether the docked Agent sidebar's chat composer currently owns typing.
    agent_input_focused: bool,
    /// Whether the MCP & permissions drawer in the Agent sidebar is expanded.
    agent_mcp_expanded: bool,
    tab_switcher: Option<TabSwitcher>,
    ai_stream: Option<std::sync::mpsc::Receiver<relay_extensions::ai::StreamEvent>>,
    ai_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_handoff: Option<String>,
    last_sync: Instant,
    last_ext_icons: Instant,
    /// Host whose Site Card is open.
    site_card: Option<String>,
    /// Recently activated tabs, newest first; Ctrl+Tab walks this order.
    mru: Vec<TabId>,
    /// Current named profile (empty is Default).
    profile_name: String,
    profile_prompt: Option<ProfilePrompt>,
    /// Bookmarks bar shows only site icons.
    bookmarks_icons_only: bool,
    /// Bookmarks bar appears only on the new-tab page.
    bookmarks_new_tab_only: bool,
    /// Keep every tab's close button visible instead of hover/active only.
    always_show_close: bool,
    /// What closing the last unpinned tab does (`start_page` default vs `quit`).
    last_tab_close: LastTabClose,
    /// Ask before closing multiple tabs at once (Quit, Close Others, Clear).
    confirm_close_multiple: bool,
    /// Switcher and Ctrl+Tab order (`mru` default vs `strip`).
    tab_switch_order: TabSwitchOrder,
    /// Active multi-tab close confirmation modal, if any.
    close_confirm: Option<CloseConfirm>,
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
        web.set_corner_style(if app_mode { 0.0 } else { 10.0 }, true, true);
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
        // Opt-in switches: absence must mean off, not the `on` that
        // `setting_enabled(None)` returns for ordinary toggles.
        let setting_on = |key: &str| setting_opt_in(store.as_ref().and_then(|s| s.setting(key).ok().flatten()).as_deref());
        let compact_tabs = setting_on("compact_tabs");
        let show_bookmarks_bar = setting_on("bookmarks_bar");
        let show_address_bar = setting_on("address_bar");
        let tab_icons_only = setting_on("tab_icons_only");
        let handoff = setting_on("handoff");
        let ai_endpoint = store
            .as_ref()
            .and_then(|s| s.setting("ai_endpoint").ok().flatten())
            .unwrap_or_else(|| relay_extensions::ai::DEFAULT_ENDPOINT.to_string());
        let ai_model = store
            .as_ref()
            .and_then(|s| s.setting("ai_model").ok().flatten())
            .unwrap_or_else(|| relay_extensions::ai::DEFAULT_MODEL.to_string());
        let ai_has_key = ai_key(store.as_ref()).is_some();
        let ai_page_context = setting_enabled(store.as_ref().and_then(|s| s.setting("ai_page_context").ok().flatten()).as_deref());
        let extension_toolbar = setting_enabled(store.as_ref().and_then(|s| s.setting("extension_toolbar").ok().flatten()).as_deref());
        let sync_folder = store.as_ref().and_then(|s| s.setting("sync_folder").ok().flatten()).unwrap_or_default();
        let bookmarks_icons_only = setting_on("bookmarks_icons_only");
        let bookmarks_new_tab_only = setting_on("bookmarks_new_tab_only");
        let collapsed_groups = store
            .as_ref()
            .and_then(|s| s.setting("collapsed_groups").ok().flatten())
            .map(|raw| collapsed_group_names(Some(&raw)))
            .unwrap_or_default();
        let group_colors = parse_group_colors(store.as_ref().and_then(|s| s.setting("group_colors").ok().flatten()).as_deref());
        let space_looks = parse_space_looks(store.as_ref().and_then(|s| s.setting("space_looks").ok().flatten()).as_deref());
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
        let last_tab_close = LastTabClose::parse(store.as_ref().and_then(|s| s.setting("last_tab_close").ok().flatten()).as_deref());
        let confirm_close_multiple = setting_on("confirm_close_multiple");
        let tab_switch_order = TabSwitchOrder::parse(store.as_ref().and_then(|s| s.setting("tab_switch_order").ok().flatten()).as_deref());
        let (start_bg, start_bg_image) = load_start_background(store.as_ref().and_then(|s| s.setting("start_bg").ok().flatten()).as_deref());
        let default_zoom = store
            .as_ref()
            .and_then(|s| s.setting("default_zoom").ok().flatten())
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|z| (0.5..=3.0).contains(z))
            .unwrap_or(1.0);
        let status_bar_position = StatusBarPosition::parse(
            store.as_ref().and_then(|s| s.setting("status_bar_position").ok().flatten()).as_deref(),
            store.as_ref().and_then(|s| s.setting("bottom_bar").ok().flatten()).as_deref(),
        );
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
            start_bg,
            start_bg_image,
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
            group_colors,
            space_looks,
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
            tab_icons_only,
            extension_toolbar,
            ext_icons: HashMap::new(),
            ext_badges: HashMap::new(),
            sync_folder,
            handoff,
            ai_endpoint,
            ai_model,
            ai_key_draft: String::new(),
            ai_focus: 0,
            ai_has_key,
            ai_page_context,
            ai_chat: Some(AiChat { messages: Vec::new(), input: String::new(), busy: false, error: None, pending: None, pending_at: None }),
            agent_input_focused: false,
            agent_mcp_expanded: false,
            tab_switcher: None,
            ai_stream: None,
            ai_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_handoff: None,
            last_sync: Instant::now(),
            last_ext_icons: Instant::now(),
            site_card: None,
            mru: Vec::new(),
            profile_name: std::env::var("RELAY_PROFILE").unwrap_or_default(),
            profile_prompt: None,
            bookmarks_icons_only,
            bookmarks_new_tab_only,
            always_show_close,
            last_tab_close,
            confirm_close_multiple,
            tab_switch_order,
            close_confirm: None,
            bookmarks_bar: vec![],
            default_zoom,
            status_bar_position,
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
            app.sync_profile_folder(true);
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

    fn refresh_extension_icons(&mut self) {
        let Some(store) = &self.store else { return };
        let ids: Vec<String> = store.enabled_extensions().unwrap_or_default().into_iter().map(|(id, _, _)| id).collect();
        let mut badges = HashMap::new();
        for id in &ids {
            if let Some(badge) = self.web.borrow().extension_badge(id) {
                badges.insert(id.clone(), badge);
            }
        }
        self.ext_badges = badges;
        if ids.len() == self.ext_icons.len() && ids.iter().all(|id| self.ext_icons.contains_key(id)) {
            return;
        }
        let mut icons = HashMap::new();
        for id in ids {
            if let Some(bytes) = self.web.borrow().extension_icon(&id) {
                if let Some(image) = favicon_decode(&bytes, "png") {
                    icons.insert(id, Arc::new(image));
                }
            }
        }
        self.ext_icons = icons;
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
        if self.agent_input_focused && !self.web.borrow().shell_is_focused() {
            self.agent_input_focused = false;
            changed = true;
        }
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
        let handoff = self.web.borrow().take_handoff();
        if let Some(url) = handoff {
            self.open_url(url, true);
            self.toast("Opened Handoff link");
        }
        self.drain_ai_stream(cx);
        self.maybe_send_ai(cx);
        self.web.borrow_mut().poll_float_panel();
        let target = self
            .active()
            .and_then(|id| self.hub.lock().tab(id).map(|t| (t.url.clone(), t.title.clone(), t.private)))
            .filter(|(url, _, private)| handoff_target(url, *private, self.handoff, self.app_mode).is_some())
            .map(|(url, title, _)| (url, title));
        let key = target.as_ref().map(|(url, title)| format!("{url}\n{title}"));
        if key != self.last_handoff {
            match &target {
                Some((url, title)) => self.web.borrow_mut().set_handoff(Some(url), title),
                None => self.web.borrow_mut().set_handoff(None, ""),
            }
            self.last_handoff = key;
        }
        if !self.sync_folder.is_empty() && self.last_sync.elapsed() > Duration::from_secs(60) {
            self.last_sync = Instant::now();
            self.sync_profile_folder(true);
        }
        if self.extension_toolbar && self.last_ext_icons.elapsed() > Duration::from_secs(2) {
            self.last_ext_icons = Instant::now();
            self.refresh_extension_icons();
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
                    app.finish_favicon(host, seq, reply, cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn finish_favicon(&mut self, host: String, seq: u64, reply: Option<serde_json::Value>, cx: &mut Context<Self>) {
        self.favicon_pending.remove(&host);
        self.favicon_tried.insert(host.clone(), seq);
        let payload = reply.as_ref().and_then(favicon_payload);
        if let Some((bytes, ext)) = &payload {
            cache_favicon_file(&host, bytes, ext);
        }
        if let Some(image) = payload.as_ref().and_then(|(bytes, ext)| favicon_decode(bytes, ext).map(Arc::new)) {
            self.favicons.insert(host, Some(image));
            return;
        }
        // When in-page fetch fails (typically cross-origin CDN favicons without
        // CORS headers), fall back to a cookie-free native fetch of the page's
        // declared icon URLs (strictly http(s), bounded 256 KB, SSRF-filtered).
        let candidates = favicon_candidate_urls(reply.as_ref(), &host);
        if !candidates.is_empty() {
            self.favicon_pending.insert(host.clone());
            let host_bg = host.clone();
            let fetched = cx.background_executor().spawn(async move {
                fetch_favicon_candidates(&host_bg, &candidates)
            });
            cx.spawn(async move |this, cx| {
                let payload = fetched.await;
                let _ = this.update(cx, |app, cx| {
                    app.favicon_pending.remove(&host);
                    if let Some((bytes, ext)) = payload {
                        cache_favicon_file(&host, &bytes, ext);
                        if let Some(image) = favicon_decode(&bytes, ext).map(Arc::new) {
                            app.favicons.insert(host, Some(image));
                            cx.notify();
                            return;
                        }
                    }
                    app.favicons.entry(host).or_insert(None);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        // A timeout or a page without an icon: keep a previously
        // decoded icon, otherwise show the monogram until a reload.
        self.favicons.entry(host).or_insert(None);
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
        if should_confirm_multi_close(self.confirm_close_multiple, loose.len()) {
            self.close_confirm = Some(CloseConfirm { kind: CloseConfirmKind::ClearLoose, count: loose.len() });
            self.web.borrow().focus_shell();
            cx.notify();
            return;
        }
        self.execute_clear_loose_tabs(cx);
    }

    fn execute_clear_loose_tabs(&mut self, cx: &mut Context<Self>) {
        let loose: Vec<TabId> = {
            let h = self.hub.lock();
            h.tabs()
                .filter(|t| t.owner == Owner::User && !t.pinned)
                .map(|t| t.id)
                .collect()
        };
        let count = loose.len();
        for id in loose {
            self.close_tab_inner(id, false, cx);
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
        self.agent_input_focused = false;
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
        self.agent_input_focused = false;
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
        if should_confirm_multi_close(self.confirm_close_multiple, others.len()) {
            self.close_confirm = Some(CloseConfirm { kind: CloseConfirmKind::CloseOthers(keep), count: others.len() });
            self.web.borrow().focus_shell();
            cx.notify();
            return;
        }
        self.execute_close_other_tabs(keep, cx);
    }

    fn execute_close_other_tabs(&mut self, keep: TabId, cx: &mut Context<Self>) {
        let others: Vec<TabId> = {
            let h = self.hub.lock();
            h.tabs()
                .filter(|t| t.id != keep && !t.pinned && t.owner == Owner::User)
                .map(|t| t.id)
                .collect()
        };
        let count = others.len();
        for id in others {
            self.close_tab_inner(id, false, cx);
        }
        if count > 0 {
            self.toast(if count == 1 { "Closed 1 other tab".to_string() } else { format!("Closed {count} other tabs") });
        }
    }

    fn request_quit(&mut self, cx: &mut Context<Self>) {
        let count = self.hub.lock().tabs().filter(|t| t.owner == Owner::User).count();
        if !self.app_mode && should_confirm_multi_close(self.confirm_close_multiple, count) {
            self.close_confirm = Some(CloseConfirm { kind: CloseConfirmKind::Quit, count });
            self.web.borrow().focus_shell();
            cx.notify();
            return;
        }
        self.save_session();
        cx.quit();
    }

    pub(super) fn cancel_close_confirm(&mut self) {
        self.close_confirm = None;
        self.focus_active_page();
    }

    pub(super) fn confirm_close_action(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.close_confirm.take() else { return };
        match confirm.kind {
            CloseConfirmKind::Quit => {
                self.save_session();
                cx.quit();
            }
            CloseConfirmKind::CloseOthers(keep) => {
                self.execute_close_other_tabs(keep, cx);
                self.focus_active_page();
            }
            CloseConfirmKind::ClearLoose => {
                self.execute_clear_loose_tabs(cx);
                self.focus_active_page();
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_pin_for(&mut self, tab: TabId) {
        self.tab_menu = None;
        if let Some(pinned) = self.hub.update(|h| h.set_pinned(tab, !h.tab(tab).is_some_and(|t| t.pinned))) {
            self.toast(if pinned { "Tab pinned to Favorites" } else { "Tab unpinned" });
        }
    }

    fn activate(&mut self, tab: TabId) {
        self.tab_menu = None;
        self.agent_input_focused = false;
        if self.site_card.is_some() {
            let host = self.hub.lock().tab(tab).map(|t| crate::clean_url::host_of(&t.url)).unwrap_or_default();
            if self.site_card.as_deref() != Some(host.as_str()) {
                self.site_card = None;
            }
        }
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
        self.mru.retain(|id| *id != tab);
        self.mru.insert(0, tab);
        self.mru.truncate(50);
        self.hub.update(|h| h.set_active(tab));
        self.web.borrow_mut().wake(tab);
        self.focus_pending = Some(tab);
        self.try_focus_pending();
    }

    /// Whether any shell-owned modal is currently drawn over the page area.
    /// Native WKWebViews are hidden while true so they cannot cover overlays
    /// or steal keyboard focus back from modal dialogs.
    fn has_modal_overlay(&self) -> bool {
        self.palette.is_some()
            || self.settings_open
            || self.library.is_some()
            || self.shortcuts.is_some()
            || self.connect_open
            || self.downloads_open
            || self.group_prompt.is_some()
            || self.space_prompt.is_some()
            || self.tab_switcher.is_some()
            || self.profile_prompt.is_some()
            || self.site_card.is_some()
            || self.close_confirm.is_some()
    }

    /// Focus the page only when the native view exists and no overlay or
    /// docked Agent chat composer owns the keyboard.
    fn try_focus_pending(&mut self) {
        let Some(tab) = self.focus_pending else { return };
        if self.active() != Some(tab)
            || self.has_modal_overlay()
            || self.hints.is_some()
            || (self.inspector_open && self.sidebar_tab == SidebarTab::Agent && self.agent_input_focused)
        {
            self.focus_pending = None;
            return;
        }
        if self.web.borrow().focus_page(tab) {
            self.focus_pending = None;
        }
    }

    fn focus_active_page(&mut self) {
        self.agent_input_focused = false;
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
    /// are put to sleep; the last tab in a web app (or when `last_tab_close`
    /// is set to `quit`) quits Relay.
    fn close_tab(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.close_tab_inner(tab, true, cx);
    }

    fn close_tab_inner(&mut self, tab: TabId, allow_last_quit: bool, cx: &mut Context<Self>) {
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
        self.mru.retain(|id| *id != tab);
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
        if allow_last_quit && should_quit_on_last_tab(self.app_mode, self.last_tab_close, last) {
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
    fn set_status_bar_position(&mut self, pos: StatusBarPosition) {
        self.status_bar_position = pos;
        if let Some(store) = &self.store {
            let _ = store.set_setting("status_bar_position", pos.as_str());
            let _ = store.set_setting("bottom_bar", if pos == StatusBarPosition::Hidden { "off" } else { "on" });
        }
    }

    fn toggle_bottom_bar(&mut self) {
        let next = self.status_bar_position.next();
        self.set_status_bar_position(next);
        self.toast(format!("Status bar: {}", next.label()));
    }

    pub(super) fn toggle_agent_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.inspector_open && self.sidebar_tab == SidebarTab::Agent {
            self.inspector_open = false;
            self.agent_input_focused = false;
            self.hub.set_sidebar(false, self.sidebar_tab.as_str());
            self.focus_active_page();
        } else {
            self.inspector_open = true;
            self.sidebar_tab = SidebarTab::Agent;
            self.hub.set_sidebar(true, self.sidebar_tab.as_str());
            if self.ai_chat.is_none() {
                self.ai_chat = Some(AiChat { messages: Vec::new(), input: String::new(), busy: false, error: None, pending: None, pending_at: None });
            }
        }
        cx.notify();
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
        if self.agent_input_focused && !self.web.borrow().shell_is_focused() {
            self.agent_input_focused = false;
        }
        if agent_composer_captures_keys(
            self.inspector_open,
            self.sidebar_tab,
            self.agent_input_focused,
            self.has_modal_overlay(),
            self.find.is_some(),
            self.hints.is_some(),
        ) {
            if let Some(chat) = self.ai_chat.as_mut() {
                match action {
                    EditAction::Paste => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                            let clean: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
                            let remain = 2000usize.saturating_sub(chat.input.chars().count());
                            chat.input.extend(clean.chars().take(remain));
                        }
                    }
                    EditAction::Copy | EditAction::Cut => {
                        if !chat.input.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(chat.input.clone()));
                        }
                        if action == EditAction::Cut {
                            chat.input.clear();
                        }
                    }
                    EditAction::SelectAll | EditAction::Undo | EditAction::Redo => {}
                }
            }
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
        let current = self.hub.lock().tab(tab).and_then(|t| t.group.clone());
        let color = current.as_deref().and_then(|g| self.group_colors.get(g).copied());
        self.group_prompt = Some(GroupPrompt {
            tab: Some(tab),
            original_group: current.clone(),
            value: current.unwrap_or_default(),
            color,
        });
        self.tab_menu = None;
        self.web.borrow().focus_shell();
        cx.notify();
    }

    pub(super) fn open_group_edit_prompt(&mut self, name: &str, cx: &mut Context<Self>) {
        let color = self.group_colors.get(name).copied();
        self.group_prompt = Some(GroupPrompt {
            tab: None,
            original_group: Some(name.to_string()),
            value: name.to_string(),
            color,
        });
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
        let current = self.hub.lock().current_space().to_string();
        let look = self.space_looks.get(&current).cloned().unwrap_or_default();
        self.space_prompt = Some(SpacePrompt { value: current, confirm_delete: false, look });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn import_safari(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let bookmarks = relay_extensions::safari::default_bookmarks();
        let history = relay_extensions::safari::default_history();
        if bookmarks.is_none() && history.is_none() {
            self.toast("Direct Safari access needs Full Disk Access — or use Import export file…");
            cx.notify();
            return;
        }
        let empty = std::path::Path::new("/nonexistent");
        match relay_extensions::safari::import(
            store,
            bookmarks.as_deref().unwrap_or(empty),
            history.as_deref().unwrap_or(empty),
            false,
        ) {
            Ok(c) => self.toast(format!("Imported {} Safari bookmarks · {} history rows", c.bookmarks, c.history)),
            Err(e) => self.toast(format!("Safari import failed: {e}")),
        }
        cx.notify();
    }

    fn import_safari_export_file(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Safari's exported browsing-data file (.zip, Bookmarks.html, or History.json)".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            if let Some(path) = picked {
                let _ = this.update(cx, |app, cx| {
                    let Some(store) = &app.store else { return };
                    match relay_extensions::safari::import_export_file(store, &path) {
                        Ok(c) => {
                            app.refresh_start_page();
                            app.toast(format!("Imported {} Safari bookmarks and {} history items", c.bookmarks, c.history));
                        }
                        Err(e) => app.toast(format!("Safari export import failed: {e}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn open_extension_popup(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.web.borrow_mut().open_extension_popup(id) {
            if let Some(badge) = self.ext_badges.get_mut(id) {
                badge.unread = false;
            }
            self.refresh_extension_icons();
            self.toast("Opened extension popup");
        } else {
            self.toast("This extension has no action popup");
        }
        cx.notify();
    }

    fn open_extension_options(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(url) = self.web.borrow().extension_options_url(id) else {
            self.toast("This extension has no options page");
            cx.notify();
            return;
        };
        self.open_url(url, true);
        cx.notify();
    }

    fn drain_ai_stream(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = self.ai_stream.take() else { return };
        let mut keep = true;
        loop {
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    keep = false;
                    if let Some(chat) = self.ai_chat.as_mut() {
                        chat.busy = false;
                    }
                    cx.notify();
                    break;
                }
            };
            let mut done = false;
            if let Some(chat) = self.ai_chat.as_mut() {
                match event {
                    relay_extensions::ai::StreamEvent::Delta(delta) => {
                        if chat.messages.last().is_some_and(|(user, _)| !*user) {
                            if let Some((_, text)) = chat.messages.last_mut() {
                                text.push_str(&delta);
                            }
                        } else {
                            chat.messages.push((false, delta));
                        }
                    }
                    relay_extensions::ai::StreamEvent::Done(Ok(answer)) => {
                        if chat.messages.last().is_some_and(|(user, _)| !*user) {
                            if let Some((_, text)) = chat.messages.last_mut() {
                                *text = answer;
                            }
                        } else {
                            chat.messages.push((false, answer));
                        }
                        chat.busy = false;
                        done = true;
                    }
                    relay_extensions::ai::StreamEvent::Done(Err(error)) => {
                        chat.busy = false;
                        chat.error = Some(error);
                        done = true;
                    }
                }
            }
            cx.notify();
            if done {
                keep = false;
                break;
            }
        }
        if keep {
            self.ai_stream = Some(rx);
        }
    }

    fn set_ai_preset(&mut self, endpoint: &str, model: &str, cx: &mut Context<Self>) {
        self.ai_endpoint = endpoint.to_string();
        self.ai_model = model.to_string();
        self.ai_focus = 2;
        cx.notify();
    }

    fn save_ai_settings(&mut self, cx: &mut Context<Self>) {
        let Some(endpoint) = relay_extensions::ai::normalize_endpoint(&self.ai_endpoint) else {
            self.toast("AI endpoint must be an http(s) URL");
            cx.notify();
            return;
        };
        self.ai_endpoint = endpoint;
        self.ai_model = self.ai_model.trim().to_string();
        if self.ai_model.is_empty() {
            self.ai_model = relay_extensions::ai::DEFAULT_MODEL.to_string();
        }
        if let Some(store) = &self.store {
            let _ = store.set_setting("ai_endpoint", &self.ai_endpoint);
            let _ = store.set_setting("ai_model", &self.ai_model);
        }
        if !self.ai_key_draft.trim().is_empty() {
            #[cfg(target_os = "macos")]
            match {
                let profile = std::env::var("RELAY_PROFILE").ok();
                relay_extensions::mac::save_ai_key(profile.as_deref(), &self.ai_key_draft)
            } {
                Ok(()) => {
                    self.ai_has_key = true;
                    self.ai_key_draft.clear();
                    self.toast("AI key saved to the login Keychain");
                }
                Err(failure) => self.toast(format!("Could not save the AI key: {failure:?}")),
            }
            #[cfg(not(target_os = "macos"))]
            {
                self.toast("Saving an AI key needs macOS; set OPENAI_API_KEY");
            }
        } else {
            self.toast("AI settings saved");
        }
        cx.notify();
    }

    fn open_tab_switcher(&mut self, cx: &mut Context<Self>) {
        let (space_ids, active) = {
            let hub = self.hub.lock();
            (hub.space_tab_ids(), hub.active())
        };
        let ids = tab_switch_sequence(self.tab_switch_order, &self.mru, &space_ids);
        let tabs: Vec<(TabId, String, String)> = ids
            .into_iter()
            .filter_map(|id| self.hub.lock().tab(id).map(|t| (id, t.title.clone(), t.url.clone())))
            .collect();
        if tabs.is_empty() {
            self.toast("No tabs in this space");
            cx.notify();
            return;
        }
        let selected = tabs.iter().position(|(id, _, _)| Some(*id) == active).map_or(0, |i| if i + 1 < tabs.len() { i + 1 } else { 0 });
        self.tab_switcher = Some(TabSwitcher { tabs, selected });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_tab_switcher(&mut self) {
        self.tab_switcher = None;
        self.focus_active_page();
    }

    fn activate_tab_switcher(&mut self, cx: &mut Context<Self>) {
        let Some(switcher) = self.tab_switcher.take() else { return };
        if let Some((id, _, _)) = switcher.tabs.get(switcher.selected) {
            self.activate(*id);
        }
        cx.notify();
    }

    fn open_ai_chat(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.inspector_open = true;
        self.sidebar_tab = SidebarTab::Agent;
        self.hub.set_sidebar(true, self.sidebar_tab.as_str());
        self.ai_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        if self.ai_chat.is_none() {
            self.ai_chat = Some(AiChat { messages: Vec::new(), input: String::new(), busy: false, error: None, pending: None, pending_at: None });
        }
        self.agent_input_focused = self.ai_has_key;
        if self.agent_input_focused {
            self.web.borrow().focus_shell();
        }
        cx.notify();
    }

    pub(super) fn focus_agent_input(&mut self, cx: &mut Context<Self>) {
        if self.ai_chat.is_none() {
            self.ai_chat = Some(AiChat { messages: Vec::new(), input: String::new(), busy: false, error: None, pending: None, pending_at: None });
        }
        self.agent_input_focused = true;
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn unfocus_agent_input(&mut self) {
        self.agent_input_focused = false;
        self.focus_active_page();
    }

    fn new_ai_chat(&mut self, cx: &mut Context<Self>) {
        self.ai_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.ai_stream = None;
        self.ai_chat = Some(AiChat { messages: Vec::new(), input: String::new(), busy: false, error: None, pending: None, pending_at: None });
        cx.notify();
    }

    fn copy_ai_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.toast("Copied AI answer");
        cx.notify();
    }

    fn summarize_page(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.ai_chat.as_mut() {
            chat.input = "Summarize this page.".to_string();
        }
        self.send_ai(cx);
    }

    fn stop_ai(&mut self, cx: &mut Context<Self>) {
        self.ai_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.ai_stream = None;
        if let Some(chat) = self.ai_chat.as_mut() {
            chat.busy = false;
            chat.error = Some("AI request stopped".into());
            chat.pending = None;
            chat.pending_at = None;
        }
        cx.notify();
    }

    fn send_ai(&mut self, cx: &mut Context<Self>) {
        let prompt = self.ai_chat.as_ref().map(|c| c.input.trim().to_string()).unwrap_or_default();
        if prompt.is_empty() || self.ai_chat.as_ref().is_some_and(|c| c.busy) {
            return;
        }
        let Some(tab) = self.active() else { return };
        let (url, title) = self.hub.lock().tab(tab).map(|t| (t.url.clone(), t.title.clone())).unwrap_or_default();
        if self.ai_chat.as_ref().is_some_and(|c| c.pending.is_some()) {
            return;
        }
        let page = self.hub.lock().page_text(tab, &url);
        let Some(chat) = self.ai_chat.as_mut() else { return };
        let history = chat.messages.clone();
        chat.input.clear();
        chat.error = None;
        chat.messages.push((true, prompt.clone()));
        chat.busy = true;
        if !self.ai_page_context {
            self.dispatch_ai(prompt, String::new(), url, title, history, cx);
            return;
        }
        match page {
            Some(page) => self.dispatch_ai(prompt, page, url, title, history, cx),
            None => {
                if let Some(chat) = self.ai_chat.as_mut() {
                    chat.pending = Some(PendingAi { prompt, history });
                    chat.pending_at = Some(Instant::now());
                }
                self.eval_op(tab, Op::PageText);
                cx.notify();
            }
        }
    }

    fn dispatch_ai(
        &mut self,
        prompt: String,
        page: String,
        url: String,
        title: String,
        history: Vec<(bool, String)>,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = ai_key(self.store.as_ref()) else {
            if let Some(chat) = self.ai_chat.as_mut() {
                chat.busy = false;
                chat.error = Some("No API key; set one in Settings → AI".into());
            }
            cx.notify();
            return;
        };
        let Some(config) = ai_config(&self.ai_endpoint, &self.ai_model) else {
            if let Some(chat) = self.ai_chat.as_mut() {
                chat.busy = false;
                chat.error = Some("AI endpoint must be an http(s) URL".into());
            }
            cx.notify();
            return;
        };
        if let Some(chat) = self.ai_chat.as_mut() {
            chat.busy = true;
        }
        cx.notify();
        self.ai_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        let cancel = self.ai_cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_stream = Some(rx);
        cx.background_executor()
            .spawn(async move {
                relay_extensions::ai::stream(&config, &key, &url, &title, &page, &prompt, &history, cancel, tx);
            })
            .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(60)).await;
                let still_streaming = this
                    .update(cx, |app, cx| {
                        app.drain_ai_stream(cx);
                        app.ai_stream.is_some()
                    })
                    .unwrap_or(false);
                if !still_streaming {
                    break;
                }
            }
        })
        .detach();
    }

    fn maybe_send_ai(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.ai_chat.as_ref().and_then(|c| c.pending.as_ref()).map(|p| PendingAi { prompt: p.prompt.clone(), history: p.history.clone() }) else { return };
        let Some(tab) = self.active() else { return };
        let (url, title) = self.hub.lock().tab(tab).map(|t| (t.url.clone(), t.title.clone())).unwrap_or_default();
        let page = self.hub.lock().page_text(tab, &url);
        let timeout = self.ai_chat.as_ref().and_then(|c| c.pending_at).is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(2));
        if page.is_none() && !timeout {
            return;
        }
        if let Some(chat) = self.ai_chat.as_mut() {
            chat.pending = None;
            chat.pending_at = None;
        }
        self.dispatch_ai(pending.prompt, page.unwrap_or_default(), url, title, pending.history, cx);
    }

    fn open_profile_prompt(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        self.profile_prompt = Some(ProfilePrompt { value: String::new() });
        self.web.borrow().focus_shell();
        cx.notify();
    }

    fn close_profile_prompt(&mut self) {
        self.profile_prompt = None;
        self.focus_active_page();
    }

    fn switch_profile(&mut self, name: &str, cx: &mut Context<Self>) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.toast(format!("Could not relaunch: {e}"));
                cx.notify();
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        if name.is_empty() {
            // Default lives at the base home, not inside profiles/.
            if let Some(base) = std::env::var_os("RELAY_BASE_HOME") {
                cmd.env("RELAY_HOME", base);
            } else {
                cmd.env_remove("RELAY_HOME");
            }
            cmd.env_remove("RELAY_PROFILE").env_remove("RELAY_PROFILE_ID");
        } else {
            cmd.arg("--profile").arg(name);
        }
        match cmd.spawn() {
            Ok(_) => cx.quit(),
            Err(e) => {
                self.toast(format!("Could not open profile: {e}"));
                cx.notify();
            }
        }
    }

    fn save_profile_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.profile_prompt.take() else { return };
        let name = profile_name(&prompt.value);
        if name.is_empty() {
            self.toast("Enter a profile name");
            cx.notify();
            return;
        }
        self.switch_profile(&name, cx);
    }

    fn open_site_card(&mut self, host: String) {
        self.site_card = Some(host);
        self.web.borrow().focus_shell();
    }

    fn close_site_card(&mut self) {
        self.site_card = None;
        self.focus_active_page();
    }

    fn forget_host_permissions(&mut self, host: &str, cx: &mut Context<Self>) {
        self.media_permissions.retain(|(h, _), _| h != host);
        self.persist_media_permissions();
        self.toast(format!("Forgot permissions for {host}"));
        cx.notify();
    }

    fn close_space_prompt(&mut self) {
        self.space_prompt = None;
        self.focus_active_page();
    }

    fn persist_space_looks(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("space_looks", &serialize_space_looks(&self.space_looks));
        }
    }

    pub(super) fn current_space_look(&self) -> theme::SpaceLook {
        let current = self.hub.lock().current_space().to_string();
        self.space_looks.get(&current).cloned().unwrap_or_default()
    }

    fn save_space_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.space_prompt.take() else { return };
        let name = Hub::normalize_space_name(&prompt.value).unwrap_or_default();
        if prompt.look.is_default() {
            self.space_looks.remove(&name);
        } else {
            self.space_looks.insert(name.clone(), prompt.look);
        }
        self.persist_space_looks();
        self.switch_space(name, cx);
    }

    fn rename_current_space(&mut self, name: String, cx: &mut Context<Self>) {
        let old = self.hub.lock().current_space().to_string();
        let new = Hub::normalize_space_name(&name).unwrap_or_default();
        if old.is_empty() {
            self.toast("Default is the unnamed space; switch to a new name instead");
            cx.notify();
            return;
        }
        if new.is_empty() || new == old {
            self.toast("Type a different space name first");
            cx.notify();
            return;
        }
        let chosen_look = self.space_prompt.as_ref().map(|p| p.look.clone());
        let moved = self.hub.update(|h| h.rename_space(&old, &new));
        let old_look = self.space_looks.remove(&old);
        if let Some(look) = chosen_look.filter(|l| !l.is_default()).or(old_look) {
            self.space_looks.entry(new.clone()).or_insert(look);
        }
        self.persist_space_looks();
        self.space_prompt = None;
        self.persist_active_space();
        let first = self.hub.lock().first_tab_in_space(&new);
        if let Some(tab) = first {
            self.activate(tab);
        }
        self.toast(if moved == 0 { format!("Renamed to {new}") } else { format!("Renamed to {new} ({moved} tabs)") });
        cx.notify();
    }

    fn delete_current_space(&mut self, cx: &mut Context<Self>) {
        let name = self.hub.lock().current_space().to_string();
        if name.is_empty() {
            self.toast("Default space cannot be deleted");
            cx.notify();
            return;
        }
        self.space_looks.remove(&name);
        self.persist_space_looks();
        let ids = self.hub.update(|h| h.take_space_tabs(&name));
        for tab in &ids {
            self.web.borrow_mut().apply(BrowserRequest::Close(*tab));
            self.snapshots.remove(tab);
            self.compat_tabs.remove(tab);
            if self.split == Some(*tab) {
                self.split = None;
            }
        }
        self.space_prompt = None;
        self.hub.update(|h| h.set_current_space(""));
        self.persist_active_space();
        let first = self.hub.lock().first_tab_in_space("");
        if let Some(tab) = first {
            self.activate(tab);
        } else {
            self.hub.update(|h| h.clear_active());
        }
        self.toast(format!("Deleted space {name} ({} tabs)", ids.len()));
        cx.notify();
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

    fn persist_group_colors(&self) {
        if let Some(store) = &self.store {
            let _ = store.set_setting("group_colors", &serialize_group_colors(&self.group_colors));
        }
    }

    fn save_group_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.group_prompt.take() else { return };
        let group = relay_bridge::hub::normalize_group_name(&prompt.value);
        match prompt.tab {
            Some(tab) => {
                self.hub.update(|h| h.set_tab_group(tab, group.as_deref()));
                let old_still_used = prompt
                    .original_group
                    .as_deref()
                    .is_some_and(|old| self.hub.lock().tab_groups().iter().any(|g| g == old));
                apply_group_color_change(
                    &mut self.group_colors,
                    prompt.original_group.as_deref(),
                    group.as_deref(),
                    prompt.color,
                    old_still_used,
                );
                self.persist_group_colors();
                match &group {
                    Some(name) => self.toast(format!("Moved tab to {name}")),
                    None => self.toast("Removed tab from its group"),
                }
            }
            None => {
                if let Some(old) = prompt.original_group.as_deref() {
                    let moved = self.hub.update(|h| h.rename_group(old, group.as_deref()));
                    if self.collapsed_groups.remove(old) {
                        if let Some(target) = &group {
                            self.collapsed_groups.insert(target.clone());
                        }
                        self.persist_collapsed_groups();
                    }
                    apply_group_color_change(&mut self.group_colors, Some(old), group.as_deref(), prompt.color, false);
                    self.persist_group_colors();
                    match &group {
                        Some(name) if name == old => self.toast(format!("Updated group {name}")),
                        Some(name) => self.toast(format!("Renamed group to {name} ({moved} tabs)")),
                        None => self.toast(format!("Ungrouped {moved} tabs")),
                    }
                }
            }
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

    /// Merge the shared folder's backup (local settings win), then write the
    /// merged profile back. `import` is false for the periodic write.
    fn sync_profile_folder(&mut self, import: bool) {
        if self.sync_folder.is_empty() {
            return;
        }
        let dir = std::path::PathBuf::from(&self.sync_folder);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let file = dir.join(sync_file_name(&self.profile_name));
        let Some(store) = &self.store else { return };
        if import && file.is_file() {
            if let Ok(json) = std::fs::read_to_string(&file) {
                let _ = store.import_export_json(&json);
            }
        }
        if let Ok(json) = store.export_json() {
            // Write to a per-process temp then rename, so another Mac never
            // reads a half-written profile file.
            let tmp = file.with_extension(format!("json.tmp-{}", std::process::id()));
            if write_private_file(&tmp, json.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, &file);
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    fn use_icloud_sync(&mut self, cx: &mut Context<Self>) {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
        let dir = icloud_relay_dir(&home);
        if std::fs::create_dir_all(&dir).is_err() {
            self.toast("iCloud Drive is not available on this Mac");
            cx.notify();
            return;
        }
        self.sync_folder = dir.to_string_lossy().into_owned();
        if let Some(store) = &self.store {
            let _ = store.set_setting("sync_folder", &self.sync_folder);
        }
        self.sync_profile_folder(true);
        self.toast("Syncing through iCloud Drive/Relay");
        cx.notify();
    }

    fn set_start_background(&mut self, bg: theme::StartBackground, cx: &mut Context<Self>) {
        let (resolved, img) = match &bg {
            theme::StartBackground::Image(path) => match load_wallpaper_image(Path::new(path)) {
                Some(image) => (bg, Some(image)),
                None => {
                    self.toast("Could not load that image (use a local PNG, JPEG, WebP, or GIF under 8 MB)");
                    cx.notify();
                    return;
                }
            },
            _ => (bg, None),
        };
        self.start_bg = resolved;
        self.start_bg_image = img;
        if let Some(store) = &self.store {
            let _ = store.set_setting("start_bg", &self.start_bg.to_setting());
        }
        cx.notify();
    }

    fn choose_start_wallpaper(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a blank-tab image".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            if let Some(path) = picked {
                let path_str = path.to_string_lossy().into_owned();
                let _ = this.update(cx, |app, cx| {
                    if !theme::StartBackground::is_safe_local_path(&path_str) {
                        app.toast("Choose a local image file");
                        cx.notify();
                        return;
                    }
                    app.set_start_background(theme::StartBackground::Image(path_str), cx);
                });
            }
        })
        .detach();
    }

    fn choose_sync_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a sync folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            if let Some(path) = picked {
                let _ = this.update(cx, |app, cx| {
                    app.sync_folder = path.to_string_lossy().into_owned();
                    if let Some(store) = &app.store {
                        let _ = store.set_setting("sync_folder", &app.sync_folder);
                    }
                    app.sync_profile_folder(true);
                    app.toast(format!("Syncing through {}", app.sync_folder));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn sync_now(&mut self, cx: &mut Context<Self>) {
        self.sync_profile_folder(true);
        self.last_sync = Instant::now();
        self.toast(if self.sync_folder.is_empty() { "Choose a sync folder first" } else { "Profile synced" });
        cx.notify();
    }

    fn export_bookmarks(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let Ok(bookmarks) = store.bookmarks() else { return };
        let html = relay_extensions::bookmarks_html::export(&bookmarks);
        let dir = relay_bridge::paths::relay_home();
        let rx = cx.prompt_for_new_path(&dir, Some("bookmarks.html"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let res = write_private_file(&path, html.as_bytes());
                let _ = this.update(cx, |app, cx| {
                    match res {
                        Ok(()) => app.toast(format!("Exported bookmarks to {}", path.display())),
                        Err(e) => app.toast(format!("Bookmark export failed: {e}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn import_bookmarks(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import bookmark HTML".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            let Some(path) = picked else { return };
            let rows = relay_extensions::bookmarks_html::read(&path).unwrap_or_default();
            let _ = this.update(cx, |app, cx| {
                let mut added = 0;
                if let Some(store) = &app.store {
                    for b in &rows {
                        if store.add_bookmark(b).ok() == Some(true) {
                            added += 1;
                        }
                    }
                }
                app.refresh_start_page();
                app.toast(format!("Imported {added} bookmarks"));
                cx.notify();
            });
        })
        .detach();
    }

    fn backup_profile(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let json = match store.export_json() {
            Ok(json) => json,
            Err(e) => {
                self.toast(format!("Could not export profile: {e}"));
                cx.notify();
                return;
            }
        };
        let dir = relay_bridge::paths::relay_home();
        let rx = cx.prompt_for_new_path(&dir, Some("relay-profile.json"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let res = write_private_file(&path, json.as_bytes());
                let _ = this.update(cx, |app, cx| {
                    match res {
                        Ok(()) => app.toast(format!("Backed up profile to {}", path.display())),
                        Err(e) => app.toast(format!("Backup failed: {e}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn restore_profile(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Restore Relay profile".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            let Some(path) = picked else { return };
            let json = std::fs::read_to_string(&path).ok();
            let _ = this.update(cx, |app, cx| {
                let Some(store) = &app.store else { return };
                match json.as_deref().map(|j| store.import_export_json(j)) {
                    Some(Ok(summary)) => app.toast(format!(
                        "Restored {} bookmarks · {} history rows · {} settings",
                        summary.bookmarks, summary.history, summary.settings
                    )),
                    Some(Err(e)) => app.toast(format!("Restore failed: {e}")),
                    None => app.toast("Could not read that backup file"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_download_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose download folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten().and_then(|mut paths| paths.pop());
            if let Some(path) = picked {
                let _ = this.update(cx, |app, cx| {
                    app.downloads_dir = path.to_string_lossy().into_owned();
                    app.persist_downloads();
                    if let Some(store) = &app.store {
                        let _ = store.set_setting("downloads_dir", &app.downloads_dir);
                    }
                    app.toast(format!("Downloads save to {}", app.downloads_dir));
                    cx.notify();
                });
            }
        })
        .detach();
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

    fn toggle_float_panel(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active() else { return };
        self.web.borrow_mut().toggle_float_panel(tab);
        self.toast("Toggled the floating video panel");
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

    fn save_library_rename(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let Some(lib) = self.library.as_mut() else { return };
        let Some((url, title)) = lib.rename_input.clone() else { return };
        match store.rename_bookmark(&url, &title) {
            Ok(true) => {
                lib.rename_input = None;
                let _ = lib.refresh(store);
            }
            Ok(false) => self.toast("Bookmark title cannot be empty or the bookmark is gone"),
            Err(e) => self.toast(format!("Could not rename the bookmark: {e}")),
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
        self.agent_input_focused = false;
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
        let (space_ids, active) = {
            let h = self.hub.lock();
            (h.space_tab_ids(), h.active())
        };
        if space_ids.is_empty() {
            return;
        }
        let order = tab_switch_sequence(self.tab_switch_order, &self.mru, &space_ids);
        let next = mru_next(&order, active, delta);
        if let Some(next) = next {
            self.activate(next);
        }
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
                self.agent_input_focused = false;
                self.hub.set_sidebar(self.inspector_open, self.sidebar_tab.as_str());
                if self.inspector_open {
                    if let Some(t) = active {
                        self.web.borrow_mut().apply(BrowserRequest::RefreshCookies(t));
                    }
                } else {
                    self.focus_active_page();
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
            Command::AskAi => self.open_ai_chat(cx),
            Command::ShowTabSwitcher => self.open_tab_switcher(cx),
            Command::DuplicateTab => {
                if let Some(url) = self.active().and_then(|t| self.hub.lock().tab(t).map(|s| s.url.clone())) {
                    self.open_url(url, true);
                }
            }
            Command::MuteTab => {
                if let Some(tab) = self.active() {
                    self.toggle_mute(tab);
                }
            }
            Command::SleepTab => {
                if let Some(tab) = self.active() {
                    self.sleep_tab_now(tab, cx);
                }
            }
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
        self.agent_input_focused = false;
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
        if self.close_confirm.is_some() {
            match k.key.as_str() {
                "escape" => self.cancel_close_confirm(),
                "enter" => self.confirm_close_action(cx),
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.tab_switcher.is_some() {
            match k.key.as_str() {
                "escape" => self.close_tab_switcher(),
                "enter" => self.activate_tab_switcher(cx),
                "up" => {
                    if let Some(s) = self.tab_switcher.as_mut() {
                        s.selected = s.selected.saturating_sub(1);
                    }
                }
                "down" | "tab" => {
                    if let Some(s) = self.tab_switcher.as_mut() {
                        s.selected = (s.selected + 1).min(s.tabs.len().saturating_sub(1));
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.agent_input_focused && !self.web.borrow().shell_is_focused() {
            self.agent_input_focused = false;
        }
        if agent_composer_captures_keys(
            self.inspector_open,
            self.sidebar_tab,
            self.agent_input_focused,
            self.has_modal_overlay(),
            self.find.is_some(),
            self.hints.is_some(),
        ) {
            match k.key.as_str() {
                "escape" => self.unfocus_agent_input(),
                "enter" => self.send_ai(cx),
                "backspace" => {
                    if let Some(chat) = self.ai_chat.as_mut() {
                        chat.input.pop();
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(chat) = self.ai_chat.as_mut() {
                            if chat.input.chars().count() < 2000 {
                                chat.input.push_str(c);
                            }
                        }
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.profile_prompt.is_some() {
            match k.key.as_str() {
                "escape" => self.close_profile_prompt(),
                "enter" => self.save_profile_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.profile_prompt.as_mut() {
                        prompt.value.pop();
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(prompt) = self.profile_prompt.as_mut() {
                            if prompt.value.chars().count() < 32 {
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
        if self.site_card.is_some() && k.key.as_str() == "escape" {
            self.close_site_card();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.space_prompt.is_some() {
            match k.key.as_str() {
                "escape" => self.close_space_prompt(),
                "enter" => self.save_space_prompt(cx),
                "backspace" => {
                    if let Some(prompt) = self.space_prompt.as_mut() {
                        prompt.confirm_delete = false;
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
                            prompt.confirm_delete = false;
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
        if self.library.as_ref().is_some_and(|l| l.rename_input.is_some()) {
            match k.key.as_str() {
                "escape" => {
                    if let Some(lib) = self.library.as_mut() {
                        lib.rename_input = None;
                    }
                }
                "enter" => self.save_library_rename(cx),
                "backspace" => {
                    if let Some(lib) = self.library.as_mut() {
                        if let Some((_, title)) = lib.rename_input.as_mut() {
                            title.pop();
                        }
                    }
                }
                _ => {
                    if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                        if let Some(lib) = self.library.as_mut() {
                            if let Some((_, title)) = lib.rename_input.as_mut() {
                                if title.chars().count() < 200 {
                                    title.push_str(c);
                                }
                            }
                        }
                    }
                }
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
                _ if self.settings_tab == SettingsTab::Ai => match k.key.as_str() {
                    "enter" => self.save_ai_settings(cx),
                    "tab" => self.ai_focus = (self.ai_focus + 1) % 3,
                    "backspace" => match self.ai_focus {
                        0 => {
                            self.ai_endpoint.pop();
                        }
                        1 => {
                            self.ai_model.pop();
                        }
                        _ => {
                            self.ai_key_draft.pop();
                        }
                    },
                    _ => {
                        if let Some(c) = k.key_char.as_deref().filter(|_| !k.modifiers.secondary() && !k.modifiers.control) {
                            match self.ai_focus {
                                0 if self.ai_endpoint.chars().count() < 300 => self.ai_endpoint.push_str(c),
                                1 if self.ai_model.chars().count() < 100 => self.ai_model.push_str(c),
                                2 if self.ai_key_draft.chars().count() < 400 => self.ai_key_draft.push_str(c),
                                _ => {}
                            }
                        }
                    }
                },
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
            "bottom_bar" => {
                self.status_bar_position = if setting_enabled(Some(value)) {
                    StatusBarPosition::Bottom
                } else {
                    StatusBarPosition::Hidden
                };
            }
            "status_bar_position" => {
                self.status_bar_position = StatusBarPosition::parse(Some(value), None);
            }
            "autofill_logins" => self.autofill_logins = setting_enabled(Some(value)),
            "tab_glyphs" => self.tab_glyphs = setting_enabled(Some(value)),
            "compact_tabs" => self.compact_tabs = setting_enabled(Some(value)),
            "bookmarks_bar" => self.show_bookmarks_bar = setting_enabled(Some(value)),
            "address_bar" => self.show_address_bar = setting_enabled(Some(value)),
            "tab_icons_only" => self.tab_icons_only = setting_enabled(Some(value)),
            "handoff" => self.handoff = setting_enabled(Some(value)),
            "ai_page_context" => self.ai_page_context = setting_enabled(Some(value)),
            "extension_toolbar" => self.extension_toolbar = setting_enabled(Some(value)),
            "bookmarks_icons_only" => self.bookmarks_icons_only = setting_enabled(Some(value)),
            "bookmarks_new_tab_only" => self.bookmarks_new_tab_only = setting_enabled(Some(value)),
            "always_show_close" => self.always_show_close = setting_enabled(Some(value)),
            "last_tab_close" => self.last_tab_close = LastTabClose::parse(Some(value)),
            "confirm_close_multiple" => self.confirm_close_multiple = setting_enabled(Some(value)),
            "tab_switch_order" => self.tab_switch_order = TabSwitchOrder::parse(Some(value)),
            "start_bg" => {
                let (bg, img) = load_start_background(Some(value));
                self.start_bg = bg;
                self.start_bg_image = img;
            }
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
        let overlay_open = self.has_modal_overlay();
        if overlay_open {
            // Keep native views from covering the palette or settings overlay.
            self.web.borrow().layout(&[]);
            self.placements.borrow_mut().clear();
        }
        let t = self.theme;
        let dark = t.background.l < 0.5;
        let shell_bg = self.current_space_look().strip_background(dark, t.background);
        let show_top_status = !self.app_mode && self.status_bar_position == StatusBarPosition::Top;
        let show_bottom_status = (!self.app_mode && self.status_bar_position == StatusBarPosition::Bottom)
            || (!show_top_status && (self.toast.is_some() || self.hints.is_some()));
        // A web app window starts chromeless; a popup tab brings the strip
        // back rather than hiding a tab the user cannot reach.
        let show_strip = !self.app_mode || self.hub.lock().tabs().count() > 1;
        let strip_visible = show_strip && !self.strip.folded;
        let has_left_strip = strip_visible && self.strip.placement == tabstrip::Placement::Left;
        let has_right_strip = strip_visible && self.strip.placement == tabstrip::Placement::Right;
        let has_top_strip = strip_visible && self.strip.placement == tabstrip::Placement::Top;

        let browser_card = if overlay_open {
            div().flex_1().h_full().bg(t.page).into_any_element()
        } else {
            let ext_bar = if self.extension_toolbar && !self.app_mode { self.render_extension_toolbar(cx) } else { None };
            let show_bm_bar = bookmarks_bar_shown(self.show_bookmarks_bar, self.bookmarks_new_tab_only, self.active().is_some()) && !self.app_mode;
            let login_fill = self.render_login_fill_banner(cx);
            let login_save = self.render_login_save_banner(cx);
            let has_top_bar = show_top_status
                || (self.show_address_bar && !self.app_mode)
                || ext_bar.is_some()
                || show_bm_bar
                || self.hub.agents_paused()
                || self.challenge_handoff().is_some()
                || login_fill.is_some()
                || login_save.is_some();
            self.web
                .borrow_mut()
                .set_corner_style(if self.app_mode { 0.0 } else { 10.0 }, !has_top_bar, !show_bottom_status);

            let mut card = div().flex_1().h_full().flex().flex_col().overflow_hidden().bg(t.page);
            if show_top_status {
                card = card.child(self.render_status(true, cx));
            }
            if self.show_address_bar && !self.app_mode {
                card = card.child(self.render_address_bar(cx));
            }
            if let Some(bar) = ext_bar {
                card = card.child(bar);
            }
            if show_bm_bar {
                card = card.child(self.render_bookmarks_bar(cx));
            }
            if self.hub.agents_paused() {
                card = card.child(self.render_agent_banner(cx));
            } else if self.challenge_handoff().is_some() {
                card = card.child(self.render_challenge_banner(cx));
            }
            if let Some(banner) = login_fill {
                card = card.child(banner);
            }
            if let Some(banner) = login_save {
                card = card.child(banner);
            }
            card = card.child(self.render_content(cx));
            // When hidden, a compact temporary bar still appears only for a
            // toast or link hints so those messages stay visible above WKWebView.
            if show_bottom_status {
                card = card.child(self.render_status(false, cx));
            }
            if self.app_mode {
                card.into_any_element()
            } else {
                card.rounded(px(12.))
                    .border_1()
                    .border_color(t.border.opacity(0.45))
                    .shadow_md()
                    .into_any_element()
            }
        };

        // Frame the middle browser card on the unified shell canvas with
        // balanced margins, clearing macOS traffic lights when the left strip
        // is folded or placed on the right.
        let stage = if self.app_mode {
            div().flex_1().min_w_0().h_full().flex().overflow_hidden().child(browser_card)
        } else {
            let pad_top = if !has_left_strip && !has_top_strip { px(32.) } else if has_top_strip { px(2.) } else { px(8.) };
            let pad_left = if has_left_strip { px(0.) } else { px(8.) };
            let pad_right = if has_right_strip || self.inspector_open { px(0.) } else { px(8.) };
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .pt(pad_top)
                .pb(px(8.))
                .pl(pad_left)
                .pr(pad_right)
                .overflow_hidden()
                .child(browser_card)
        };

        // Horizontal workspace: left sidebar, elevated center card, optional
        // right tab strip, and right Agent/Inspector sidebar always sit side-by-side.
        let mut workspace = div().flex_1().min_h_0().flex().overflow_hidden();
        if has_left_strip {
            workspace = workspace.child(self.render_tab_bar(cx));
        }
        workspace = workspace.child(stage);
        if has_right_strip {
            workspace = workspace.child(self.render_tab_bar(cx));
        }
        if self.inspector_open {
            workspace = workspace.child(self.render_inspector(cx));
        }

        let mut main = div().flex_1().min_h_0().flex().flex_col().overflow_hidden();
        if has_top_strip {
            main = main.child(self.render_tab_bar(cx));
        }
        main = main.child(workspace);

        let mut root = div()
            .key_context("Relay")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(shell_bg)
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
                app.request_quit(cx);
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
            .on_action(cx.listener(|app, _: &ToggleFloatPanel, _, cx| {
                app.toggle_float_panel(cx);
            }))
            .on_action(cx.listener(|app, _: &AskAi, _, cx| {
                app.open_ai_chat(cx);
            }))
            .on_action(cx.listener(|app, _: &ShowTabSwitcher, _, cx| {
                app.open_tab_switcher(cx);
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
        // The folded-strip pill is a small overlay over the page, not part of
        // the layout: show it on a page too, or hiding the status bar leaves
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
        if let Some(confirm) = self.close_confirm {
            root = root.child(self.render_close_confirm(confirm, cx));
        }
        if let Some(switcher) = &self.tab_switcher {
            root = root.child(self.render_tab_switcher(switcher, cx));
        }
        if let Some(prompt) = &self.profile_prompt {
            root = root.child(self.render_profile_prompt(prompt, cx));
        }
        if let Some(host) = &self.site_card {
            root = root.child(self.render_site_card(host, cx));
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
/// Safe profile directory name: letters/digits/-/_ only, 32 chars.
pub(crate) fn profile_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if (c.is_whitespace() || c == '/' || c == '\\') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

/// One-shot 140 ms fade for modal panels.
pub(crate) fn fade_in(panel: gpui::Div, id: &'static str) -> impl IntoElement {
    panel.with_animation(id, Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()), |el, delta| el.opacity(delta))
}

/// iCloud Drive's Relay folder under a home directory.
pub(crate) fn icloud_relay_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Mobile Documents/com~apple~CloudDocs/Relay")
}

#[cfg(target_os = "macos")]
fn ai_key(store: Option<&Store>) -> Option<String> {
    let profile = std::env::var("RELAY_PROFILE").ok();
    match relay_extensions::mac::read_ai_key(profile.as_deref()) {
        Ok(Some(key)) => Some(key),
        Ok(None) => std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.trim().is_empty()),
        Err(_) => None,
    }
    .filter(|_| store.is_some() || std::env::var("OPENAI_API_KEY").is_ok())
}

#[cfg(not(target_os = "macos"))]
fn ai_key(_store: Option<&Store>) -> Option<String> {
    std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.trim().is_empty())
}

pub(crate) fn ai_config(endpoint: &str, model: &str) -> Option<relay_extensions::ai::AiConfig> {
    Some(relay_extensions::ai::AiConfig {
        endpoint: relay_extensions::ai::normalize_endpoint(endpoint)?,
        model: model.trim().to_string(),
    })
}

/// Only ordinary http(s) browser tabs are advertised through Handoff.
pub(crate) fn handoff_target(url: &str, private: bool, enabled: bool, app_mode: bool) -> Option<&str> {
    (enabled && !private && !app_mode && (url.starts_with("http://") || url.starts_with("https://"))).then_some(url)
}

/// Backup file name for a profile; Default gets an explicit suffix.
pub(crate) fn sync_file_name(profile: &str) -> String {
    if profile.is_empty() { "relay-profile-default.json".to_string() } else { format!("relay-profile-{profile}.json") }
}

/// Profile directories under the base home.
pub(crate) fn profile_names() -> Vec<String> {
    let base = std::env::var_os("RELAY_BASE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("RELAY_HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(relay_bridge::paths::relay_home);
    let mut names: Vec<String> = std::fs::read_dir(base.join("profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

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

/// Parse persisted `group_colors` JSON (`{"Group": "teal"}`). Unknown or
/// deleted colour names are dropped so groups fall back to their deterministic
/// colour without losing the group.
pub(crate) fn parse_group_colors(raw: Option<&str>) -> std::collections::BTreeMap<String, theme::GroupColor> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return std::collections::BTreeMap::new();
    };
    let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, String>>(raw) else {
        return std::collections::BTreeMap::new();
    };
    let mut out = std::collections::BTreeMap::new();
    for (name, color_name) in map {
        if let (Some(norm), Some(color)) = (relay_bridge::hub::normalize_group_name(&name), theme::GroupColor::parse(&color_name)) {
            out.insert(norm, color);
        }
    }
    out
}

pub(crate) fn serialize_group_colors(colors: &std::collections::BTreeMap<String, theme::GroupColor>) -> String {
    let raw: std::collections::BTreeMap<&str, &str> = colors.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    serde_json::to_string(&raw).unwrap_or_else(|_| "{}".into())
}

/// Parse persisted `space_looks` JSON (`{"Work": {"fill": "preset:ocean", "icon": "work"}}`).
/// Invalid or deleted fills/icons fall back to defaults without losing the space.
pub(crate) fn parse_space_looks(raw: Option<&str>) -> std::collections::BTreeMap<String, theme::SpaceLook> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return std::collections::BTreeMap::new();
    };
    let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(raw) else {
        return std::collections::BTreeMap::new();
    };
    let mut out = std::collections::BTreeMap::new();
    for (name, val) in map {
        let norm = Hub::normalize_space_name(&name).unwrap_or_default();
        let fill = val.get("fill").and_then(serde_json::Value::as_str).map(theme::SpaceFill::parse).unwrap_or_default();
        let icon = val.get("icon").and_then(serde_json::Value::as_str).and_then(theme::SpaceIcon::parse).unwrap_or_default();
        let look = theme::SpaceLook { fill, icon };
        if !look.is_default() {
            out.insert(norm, look);
        }
    }
    out
}

pub(crate) fn serialize_space_looks(looks: &std::collections::BTreeMap<String, theme::SpaceLook>) -> String {
    let raw: std::collections::BTreeMap<&str, serde_json::Value> = looks
        .iter()
        .filter(|(_, v)| !v.is_default())
        .map(|(k, v)| (k.as_str(), serde_json::json!({ "fill": v.fill.to_setting(), "icon": v.icon.as_str() })))
        .collect();
    serde_json::to_string(&raw).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn resolve_group_color(colors: &std::collections::BTreeMap<String, theme::GroupColor>, name: &str) -> theme::GroupColor {
    colors.get(name).copied().unwrap_or_else(|| theme::GroupColor::for_group_name(name))
}

/// Update `colors` when a group is created, recoloured, renamed, or merged.
/// - Explicit `chosen` colour always wins for `new_group`.
/// - Choosing Default (`chosen = None`) on the same group clears its override.
/// - Renaming/merging `old -> new` with no new override keeps `new`'s existing
///   colour if present (merge target wins deterministically), else carries `old`'s.
pub(crate) fn apply_group_color_change(
    colors: &mut std::collections::BTreeMap<String, theme::GroupColor>,
    old_group: Option<&str>,
    new_group: Option<&str>,
    chosen: Option<theme::GroupColor>,
    old_still_used: bool,
) {
    let old_color = old_group.and_then(|old| colors.get(old).copied());
    if let Some(target) = new_group {
        if let Some(c) = chosen {
            colors.insert(target.to_string(), c);
        } else if old_group == Some(target) {
            colors.remove(target);
        } else if !colors.contains_key(target) {
            if let Some(c) = old_color {
                colors.insert(target.to_string(), c);
            }
        }
    }
    if let Some(old) = old_group {
        if !old_still_used && new_group != Some(old) {
            colors.remove(old);
        }
    }
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

/// Ordered tab sequence for Ctrl+Tab and the visual tab switcher.
pub(crate) fn tab_switch_sequence(mode: TabSwitchOrder, mru: &[TabId], space_ids: &[TabId]) -> Vec<TabId> {
    match mode {
        TabSwitchOrder::Strip => space_ids.to_vec(),
        TabSwitchOrder::Mru => {
            let mut order: Vec<TabId> = mru.iter().copied().filter(|id| space_ids.contains(id)).collect();
            for id in space_ids {
                if !order.contains(id) {
                    order.push(*id);
                }
            }
            order
        }
    }
}

/// Whether closing the last tab should quit Relay.
pub(crate) fn should_quit_on_last_tab(app_mode: bool, pref: LastTabClose, is_last: bool) -> bool {
    is_last && (app_mode || pref == LastTabClose::Quit)
}

/// Whether a multi-tab close operation should ask before proceeding.
pub(crate) fn should_confirm_multi_close(confirm_enabled: bool, count: usize) -> bool {
    confirm_enabled && count >= 2
}

/// Next tab in the ordered list, wrapping; `None` when the list is empty.
fn mru_next(order: &[TabId], active: Option<TabId>, delta: isize) -> Option<TabId> {
    if order.is_empty() {
        return None;
    }
    let pos = active.and_then(|a| order.iter().position(|t| *t == a)).unwrap_or(0) as isize;
    Some(order[((pos + delta).rem_euclid(order.len() as isize)) as usize])
}

fn zoom_step(current: f64, direction: i8) -> f64 {
    let current = if current.is_finite() && current > 0.0 { current } else { 1.0 };
    match direction {
        0 => 1.0,
        d if d > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > current + 1e-6).unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]),
        _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 1e-6).unwrap_or(ZOOM_STEPS[0]),
    }
}

#[cfg(unix)]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
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

/// Maximum local wallpaper size accepted for the blank tab (8 MB).
pub(crate) const MAX_WALLPAPER_BYTES: u64 = 8 * 1024 * 1024;

/// Load and validate a local blank-tab wallpaper image. Rejects missing files,
/// oversized files, unknown formats, and decompression bombs. Never fetches
/// remote URLs.
pub(crate) fn load_wallpaper_image(path: &Path) -> Option<Arc<Image>> {
    if !path.is_absolute() {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_WALLPAPER_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_WALLPAPER_BYTES {
        return None;
    }
    let (ext, fmt) = if favicon_signature(&bytes, "png") {
        ("png", ImageFormat::Png)
    } else if favicon_signature(&bytes, "jpg") {
        ("jpg", ImageFormat::Jpeg)
    } else if favicon_signature(&bytes, "webp") {
        ("webp", ImageFormat::Webp)
    } else if favicon_signature(&bytes, "gif") {
        ("gif", ImageFormat::Gif)
    } else {
        return None;
    };
    let (w, h) = favicon_dimensions(&bytes, ext)?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > 36 * 1024 * 1024 {
        return None;
    }
    Some(Arc::new(Image::from_bytes(fmt, bytes)))
}

/// Resolve persisted `start_bg` setting into `(StartBackground, Option<Arc<Image>>)`.
/// Invalid or oversized wallpaper files fall back cleanly to `StartBackground::Default`.
pub(crate) fn load_start_background(raw: Option<&str>) -> (theme::StartBackground, Option<Arc<Image>>) {
    let parsed = theme::StartBackground::parse(raw);
    match &parsed {
        theme::StartBackground::Image(path) => match load_wallpaper_image(Path::new(path)) {
            Some(img) => (parsed, Some(img)),
            None => (theme::StartBackground::Default, None),
        },
        _ => (parsed, None),
    }
}

/// Pull the largest usable image out of an ICO container: prefers embedded PNG
/// entries, and falls back to decoding classic BMP-DIB entries (32/24/8/4/1-bit
/// with AND transparency mask) into a standard RGBA PNG.
fn ico_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if !favicon_signature(bytes, "ico") {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || count > 64 || bytes.len() < 6 + count * 16 {
        return None;
    }
    let mut best_png: Option<(u32, usize, usize)> = None;
    let mut best_bmp: Option<(u32, usize, usize, u32, u32)> = None;
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
        if size < 8 || end > bytes.len() {
            continue;
        }
        let area = width.saturating_mul(height);
        if bytes[offset..end].starts_with(b"\x89PNG\r\n\x1a\n") {
            if best_png.is_none_or(|(best_area, _, _)| area > best_area) {
                best_png = Some((area, offset, size));
            }
        } else if size >= 40 && best_bmp.is_none_or(|(best_area, _, _, _, _)| area > best_area) {
            best_bmp = Some((area, offset, size, width, height));
        }
    }
    if let Some((_, offset, size)) = best_png {
        return Some(bytes[offset..offset + size].to_vec());
    }
    let (_, offset, size, dir_w, dir_h) = best_bmp?;
    ico_bmp_dib_to_png(&bytes[offset..offset + size], dir_w, dir_h)
}

fn ico_bmp_dib_to_png(dib: &[u8], dir_w: u32, dir_h: u32) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let hdr_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    if hdr_size < 40 || hdr_size > dib.len() {
        return None;
    }
    let raw_w = i32::from_le_bytes(dib[4..8].try_into().ok()?);
    let raw_h = i32::from_le_bytes(dib[8..12].try_into().ok()?);
    let bpp = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let colors_used = u32::from_le_bytes(dib[32..36].try_into().ok()?) as usize;
    if compression != 0 || !matches!(bpp, 1 | 4 | 8 | 24 | 32) {
        return None;
    }
    let w = if raw_w > 0 { raw_w as u32 } else { dir_w };
    let h = if raw_h != 0 { (raw_h.unsigned_abs() / 2).max(1) } else { dir_h };
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let top_down = raw_h < 0;
    let palette_entries = if bpp <= 8 {
        if colors_used > 0 { colors_used.min(256) } else { 1usize << bpp }
    } else {
        0
    };
    let palette_bytes = palette_entries.checked_mul(4)?;
    let pixel_start = hdr_size.checked_add(palette_bytes)?;
    if pixel_start > dib.len() {
        return None;
    }
    let palette = &dib[hdr_size..pixel_start];
    let row_stride = (((w as usize) * (bpp as usize)).div_ceil(32)) * 4;
    let xor_size = row_stride.checked_mul(h as usize)?;
    let mask_start = pixel_start.checked_add(xor_size)?;
    if mask_start > dib.len() {
        return None;
    }
    let xor_data = &dib[pixel_start..mask_start];
    let mask_stride = (w as usize).div_ceil(32) * 4;
    let mask_size = mask_stride.checked_mul(h as usize)?;
    let mask_data = (mask_start.checked_add(mask_size).is_some_and(|end| end <= dib.len()))
        .then(|| &dib[mask_start..mask_start + mask_size]);

    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    let mut any_alpha = false;
    for y in 0..h as usize {
        let src_y = if top_down { y } else { (h as usize) - 1 - y };
        let row = &xor_data[src_y * row_stride..(src_y + 1) * row_stride];
        for x in 0..w as usize {
            let dst = (y * (w as usize) + x) * 4;
            match bpp {
                32 => {
                    let b = row[x * 4];
                    let g = row[x * 4 + 1];
                    let r = row[x * 4 + 2];
                    let a = row[x * 4 + 3];
                    if a != 0 {
                        any_alpha = true;
                    }
                    rgba[dst..dst + 4].copy_from_slice(&[r, g, b, a]);
                }
                24 => {
                    let b = row[x * 3];
                    let g = row[x * 3 + 1];
                    let r = row[x * 3 + 2];
                    rgba[dst..dst + 4].copy_from_slice(&[r, g, b, 255]);
                }
                8 | 4 | 1 => {
                    let idx = match bpp {
                        8 => row[x] as usize,
                        4 => {
                            let byte = row[x / 2];
                            (if x % 2 == 0 { byte >> 4 } else { byte & 0x0f }) as usize
                        }
                        _ => ((row[x / 8] >> (7 - (x % 8))) & 1) as usize,
                    };
                    if idx < palette_entries {
                        let p = idx * 4;
                        rgba[dst..dst + 4].copy_from_slice(&[palette[p + 2], palette[p + 1], palette[p], 255]);
                    }
                }
                _ => return None,
            }
        }
    }
    if bpp != 32 || !any_alpha {
        for y in 0..h as usize {
            let src_y = if top_down { y } else { (h as usize) - 1 - y };
            for x in 0..w as usize {
                let dst = (y * (w as usize) + x) * 4;
                let transparent = mask_data.is_some_and(|m| {
                    let byte = m[src_y * mask_stride + (x / 8)];
                    ((byte >> (7 - (x % 8))) & 1) != 0
                });
                rgba[dst + 3] = if transparent { 0 } else { 255 };
            }
        }
    }
    Some(encode_rgba_png(w, h, &rgba))
}

fn encode_rgba_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(parts: &[&[u8]]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for part in parts {
            for &b in *part {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
                }
            }
        }
        !crc
    }
    fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[tag, data]).to_be_bytes());
    }

    let mut raw = Vec::with_capacity((h as usize) * (1 + (w as usize) * 4));
    for y in 0..h as usize {
        raw.push(0); // filter type None
        let start = y * (w as usize) * 4;
        raw.extend_from_slice(&rgba[start..start + (w as usize) * 4]);
    }
    // Wrap raw filtered scanlines in a valid zlib stream using stored deflate blocks.
    let mut zlib = Vec::with_capacity(raw.len() + 32);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut offset = 0usize;
    while offset < raw.len() {
        let chunk_len = (raw.len() - offset).min(65535);
        let is_last = offset + chunk_len == raw.len();
        zlib.push(if is_last { 0x01 } else { 0x00 });
        let len16 = chunk_len as u16;
        zlib.extend_from_slice(&len16.to_le_bytes());
        zlib.extend_from_slice(&(!len16).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + chunk_len]);
        offset += chunk_len;
    }
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in &raw {
        s1 = (s1 + u32::from(b)) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&w.to_be_bytes());
    ihdr[4..8].copy_from_slice(&h.to_be_bytes());
    ihdr[8] = 8; // 8-bit
    ihdr[9] = 6; // RGBA
    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);
    png
}

/// Extract safe `http(s)` fallback favicon URLs reported by the page bridge
/// when same-origin `fetch()` hit CORS.
fn favicon_candidate_urls(reply: Option<&serde_json::Value>, tab_host: &str) -> Vec<String> {
    let Some(list) = reply.and_then(|v| v.get("urls")).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|v| v.as_str())
        .filter(|url| favicon_url_allowed(tab_host, url))
        .take(5)
        .map(str::to_string)
        .collect()
}

/// Whether the docked Agent chat composer currently owns shell key and edit
/// commands (never while a modal, Find bar, Link Hints, or Inspector tab is active).
fn agent_composer_captures_keys(
    inspector_open: bool,
    sidebar_tab: SidebarTab,
    agent_input_focused: bool,
    has_modal: bool,
    find_open: bool,
    hints_open: bool,
) -> bool {
    inspector_open && sidebar_tab == SidebarTab::Agent && agent_input_focused && !has_modal && !find_open && !hints_open
}

/// True only for globally routable unicast IP addresses (rejects loopback,
/// RFC1918 private, CGNAT 100.64.0.0/10, link-local, ULA, site-local,
/// IPv4-mapped IPv6, unspecified, broadcast, documentation, and multicast).
pub(crate) fn favicon_ip_public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            let is_zero = o[0] == 0;
            let is_cgnat = o[0] == 100 && (64..=127).contains(&o[1]);
            let is_doc = (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113);
            let is_bench = o[0] == 198 && (o[1] == 18 || o[1] == 19);
            let is_reserved = o[0] >= 240;
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || is_zero
                || is_cgnat
                || is_doc
                || is_bench
                || is_reserved)
        }
        std::net::IpAddr::V6(v6) => {
            if v6.to_ipv4_mapped().is_some() {
                return false;
            }
            let s = v6.segments();
            let is_ula = (s[0] & 0xfe00) == 0xfc00;
            let is_link_local = (s[0] & 0xffc0) == 0xfe80;
            let is_site_local = (s[0] & 0xffc0) == 0xfec0;
            let is_doc = s[0] == 0x2001 && s[1] == 0x0db8;
            let is_v4_compat = s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0;
            let is_nat64 = (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)
                || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001);
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || is_ula
                || is_link_local
                || is_site_local
                || is_doc
                || is_v4_compat
                || is_nat64)
        }
    }
}

/// Reject legacy/obfuscated numeric IPv4 host forms (`127.1`, `0x7f000001`,
/// `2130706433`, octal `0177.0.0.1`) that resolvers accept as loopback/LAN.
fn looks_like_numeric_host(host: &str) -> bool {
    if host.is_empty() {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    let all_numeric_parts = parts.iter().all(|part| {
        !part.is_empty()
            && (part.chars().all(|c| c.is_ascii_digit())
                || part.strip_prefix("0x").is_some_and(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())))
    });
    if all_numeric_parts {
        // Only canonical dotted-quad IPv4 without leading zeros or hex is parsed as IpAddr.
        let canonical = parts.len() == 4
            && parts.iter().all(|p| !p.starts_with("0x") && (p.len() == 1 || !p.starts_with('0')));
        return !canonical;
    }
    false
}

/// Parse `(raw_host_without_brackets, port)` from an `http(s)` URL authority.
fn parse_favicon_authority(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        443
    } else if scheme.eq_ignore_ascii_case("http") {
        80
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    if let Some(inner) = authority.strip_prefix('[') {
        let (host, tail) = inner.split_once(']')?;
        let port = match tail {
            "" => default_port,
            t if t.starts_with(':') => t[1..].parse::<u16>().ok()?,
            _ => return None,
        };
        let host = host.trim().to_ascii_lowercase();
        (!host.is_empty()).then_some((host, port))
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok()?),
            None => (authority, default_port),
        };
        let host = host.trim().to_ascii_lowercase();
        (!host.is_empty()).then_some((host, port))
    }
}

/// Allow only public `http(s)` icon URLs for the native CORS fallback. Same-origin
/// local/LAN icons already succeed in-page via `fetch()`; the native fallback
/// strictly rejects localhost, private/loopback/link-local/CGNAT/IPv6-special
/// ranges, and obfuscated numeric hosts (never exempting DNS hosts via `tab_host`).
pub(crate) fn favicon_url_allowed(_tab_host: &str, url: &str) -> bool {
    if url.len() > 2048 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((raw_host, _port)) = parse_favicon_authority(url) else { return false };
    if raw_host == "localhost"
        || raw_host.ends_with(".localhost")
        || raw_host.ends_with(".local")
        || raw_host.ends_with(".internal")
        || looks_like_numeric_host(&raw_host)
    {
        return false;
    }
    if let Ok(ip) = raw_host.parse::<std::net::IpAddr>() {
        return favicon_ip_public(ip);
    }
    true
}

/// Resolve `url` and verify EVERY returned IP is globally routable before
/// connecting, returning `(raw_host, port, pinned_ip)` so `curl --resolve`
/// prevents DNS rebinding and dual-A/same-host loopback bypasses.
fn resolve_safe_favicon_target(tab_host: &str, url: &str) -> Option<(String, u16, std::net::IpAddr)> {
    use std::net::ToSocketAddrs;
    if !favicon_url_allowed(tab_host, url) {
        return None;
    }
    let (raw_host, port) = parse_favicon_authority(url)?;
    let addrs: Vec<std::net::IpAddr> = (raw_host.as_str(), port).to_socket_addrs().ok()?.map(|s| s.ip()).collect();
    if addrs.is_empty() || addrs.iter().any(|ip| !favicon_ip_public(*ip)) {
        return None;
    }
    Some((raw_host, port, addrs[0]))
}

/// Detect, validate, and normalize raw favicon bytes fetched natively.
pub(crate) fn favicon_bytes_to_payload(mut bytes: Vec<u8>) -> Option<(Vec<u8>, &'static str)> {
    if bytes.is_empty() || bytes.len() > 256 * 1024 {
        return None;
    }
    let mut ext = if favicon_signature(&bytes, "png") {
        "png"
    } else if favicon_signature(&bytes, "ico") {
        "ico"
    } else if favicon_signature(&bytes, "svg") {
        "svg"
    } else if favicon_signature(&bytes, "webp") {
        "webp"
    } else if favicon_signature(&bytes, "jpg") {
        "jpg"
    } else if favicon_signature(&bytes, "gif") {
        "gif"
    } else {
        return None;
    };
    if ext == "ico" {
        bytes = ico_png(&bytes)?;
        ext = "png";
    }
    if !favicon_pixels_ok(&bytes, ext) {
        return None;
    }
    (bytes.len() <= 256 * 1024).then_some((bytes, ext))
}

/// Cookie-free native fallback fetch for cross-origin CDN favicons (e.g.
/// GitHub, Google, YouTube, Reddit) whose `fetch()` is blocked by CORS.
/// Disables `.curlrc` (`-q`), disables auto-redirects (no 302 to LAN/metadata),
/// resolves DNS upfront, validates every IP, and pins via `--resolve`.
fn fetch_favicon_candidates(tab_host: &str, urls: &[String]) -> Option<(Vec<u8>, &'static str)> {
    use std::io::Read;
    for url in urls.iter().take(5) {
        let Some((raw_host, port, pinned_ip)) = resolve_safe_favicon_target(tab_host, url) else {
            continue;
        };
        let host_arg = if raw_host.contains(':') { format!("[{raw_host}]") } else { raw_host };
        let ip_arg = match pinned_ip {
            std::net::IpAddr::V4(v4) => v4.to_string(),
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        };
        let resolve_arg = format!("{host_arg}:{port}:{ip_arg}");
        let mut child = std::process::Command::new("curl")
            .args([
                "-q",
                "--silent",
                "--fail",
                "--max-time",
                "4",
                "--max-filesize",
                "262144",
                "--proto",
                "=http,https",
                "--globoff",
                "--resolve",
                &resolve_arg,
                "--",
                url,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut buf = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let _ = stdout.take(256 * 1024 + 1).read_to_end(&mut buf);
        }
        let status = child.wait().ok();
        if status.is_some_and(|s| s.success()) {
            if let Some(payload) = favicon_bytes_to_payload(buf) {
                return Some(payload);
            }
        }
    }
    None
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
    // Remove any older cached extension for this host so reload order on restart is deterministic.
    for old_ext in ["png", "jpg", "webp", "gif", "svg"] {
        if old_ext != ext {
            let _ = std::fs::remove_file(dir.join(format!("{name}.{old_ext}")));
        }
    }
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

