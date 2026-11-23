# Relay

<!-- index: design | Relay crate map, data flow, threading, and seams | changing any crate or adding one -->
<!-- covers: crates/** Cargo.toml Cargo.lock third_party/** -->

Relay is a macOS browser: a GPUI shell over native `WKWebView`s. One injected
script ([`bridge.md`](./bridge.md)) feeds one Rust state store, and two
frontends read it: the native Inspector and the local MCP server.

## Crates

| Crate | Owns | Platform |
|---|---|---|
| `relay-bridge` | Page protocol (`protocol.rs`), the shared tab state `SharedHub` (`hub.rs`), the `Browser` trait the shell implements, `ChannelBrowser` for off-main-thread callers, `relay_home()` paths | portable |
| `relay-webkit` | Content-blocker compiler (EasyList subset → WebKit JSON), tab-sleep policy; `mac.rs`: `Engine` creating one `WKWebView` per awake tab with the bridge injected, rule lists, cookies, snapshots, the Web Inspector fallback | policy portable, engine macOS |
| `relay-extensions` | Chrome importer (Bookmarks JSON, History SQLite, extension ids, password CSV), CRX3 download and unpack, manifest compatibility report, SQLite profile `Store`; `mac.rs`: WKWebExtension loading, Keychain import | importer portable, loaders macOS |
| `relay-mcp` | `RelayMcp` (rmcp 3.x tool router), user-only Unix socket server, `relay mcp` stdio proxy | portable (Unix) |
| `relay-ui` | The `relay` binary. `app.rs`: the GPUI shell (native menu bar, tab strip with close buttons/agent group/fold/move, panes and split, ⌘K palette, ⌘, Settings overlay, Inspector with console/network filters, link hints, status bar, a 16 ms pump draining MCP requests, WebKit events and hub changes, history recording, tab sleeping, web app install/remove). `web.rs`: `WebHost` over the `Engine` (a no-op off-Mac). `tabstrip.rs`: placement/fold logic. `startpage.rs`: new-tab sections and rows. `webapp.rs`: manifest validation and `.app` bundle writer. `palette.rs`, `inspector.rs`: tested view logic. `theme.rs`: tokens from [`design.md`](./design.md) with WCAG AA contrast tests. `main.rs`: the CLI (`mcp`, `--app`, `import-chrome`, `import-passwords`, `install-extension`, `compile-rules`, `config`, `export`, `clear-data`, `doctor`) | GPUI everywhere; WebKit only on macOS. Linux shows a placeholder pane |

## Data flow

```
bridge.js ──postMessage──▶ BridgeHandler (main thread) ──ingest_json──▶ SharedHub
                                                                         │  watch tick
                                             Inspector (GPUI) ◀──────────┤
                                             RelayMcp (tokio) ◀──────────┘
RelayMcp ──Browser::eval/navigate──▶ ChannelBrowser ──▶ UI drains ──▶ Engine (WKWebView)
```

- **One writer.** Only bridge messages (plus cookie refreshes from
  `WKHTTPCookieStore`) write page state. UIs re-render on `SharedHub::subscribe`.
- **Commands.** `SharedHub::begin_command` assigns an id and returns the script
  (`Command::to_script`, JSON-only). The page replies with `kind: "result"`,
  which resolves only for the tab that was sent the command.
- **Threading.** WebKit lives on the main thread. The MCP server runs on a tokio
  runtime in a background thread and reaches WebKit only through
  `ChannelBrowser` requests, which the UI applies with the `Engine` methods.

## Shell loop

The browser starts the MCP socket server on a tokio thread, then opens one
GPUI window. Every 16 ms `RelayApp::pump`:

1. applies `ChannelBrowser` requests to WebKit;
2. handles WebKit events (`target=_blank` opens a tab after its opener;
   snapshot ready);
3. re-renders on a hub tick, records history visits, finishes wake-up scroll
   restores, and starts link hints on an `f` hotkey;
4. focusses the active page once its native view is attached (and retries on
   the next pump for a slept tab);
5. checks idle tabs for sleep every 30 s.

The pump keeps one `watch::Receiver` for its lifetime; it does not build a
new subscription every 16 ms. The tab strip renders `TabView` rows (id, title,
url, owner, state) built from `Hub::tab_views`, never cloned `TabState`s with
their DOM/console/network rings. The Inspector caches its row data per hub
version, tab, panel, expansion epoch, and filter, so a repaint paints cached
strings instead of walking the DOM again. The document title and the
`claude mcp add` line are cached until the hub version or process changes. `SharedHub::take_hotkeys` drains the page's
hotkey queue without ticking the watch, so an idle window settles instead of
waking itself every frame.

The bridge roots its snapshot at `documentElement` so html/head attribute
changes stay patchable; attribute-only updates arrive as `dom_attrs` and the
Inspector/agents read the same store either way.

Panes report their bounds from a `canvas` each frame, and `WebHost::layout`
moves the native `WKWebView`s to match. Native views draw above GPUI, so web
views are hidden while the palette is open. Link-hint labels are drawn inside
the page, and their status line goes in the status bar.

## Native menus and focus

`app_menus()` in `app.rs` installs a real macOS menu bar (`cx.set_menus`). GPUI
turns the key map into menu key equivalents, and AppKit checks those before the
focused `WKWebView`, so ⌘K/⌘T/⌘W/⌘C and the rest reach Relay even while the
page owns the responder chain. This is the reason the shortcuts work
"natively"; adding only `KeyBinding`s would leave them inside GPUI's view and
the page would swallow them.

The Edit menu (`Cut`, `Copy`, `Paste`, `Select All`, `Undo`, `Redo`) does not
use AppKit's `cut:` selectors directly. Each action reaches
`RelayApp::apply_edit`, which handles paste into the palette or find bar when
an overlay is open and otherwise calls `Engine::edit`. That sends the selector
to the window's **first responder** with `tryToPerform:with:` rather than
`NSApplication.sendAction:`: GPUI's app delegate also implements `copy:` and
`paste:` for its own menu items, and the app-level chain could route the
command back into Relay instead of the page. The focused page's first responder
is WebKit's content view, which owns the real editing implementation.

Page focus after a transition is deferred, not fired once: palette Enter/a row
click, Esc, Settings, find, a CLI URL, and switching to a slept tab all set
`focus_pending`, and the pump retries `Engine::focus_page` until WebKit's view
is attached and actually took first responder. The retry never fires while the
palette, Settings, or hints own the keyboard. Switching tabs also hides the
previous tab's link hints; Esc then focuses the newly active page, not the old
one.

## Tab strip

`tabstrip.rs` owns the placement (`Left`, `Top`, `Right`), fold state, metrics,
and the `tabs_position` / `tabs_folded` settings encoding; it is pure and unit
tested. `⌘S` folds to a clickable rail and `⇧⌘S` rotates the edge. Every row
renders a `×` (visible on the active row and while the pointer is over it); a
middle click closes too. Closing a pinned row sleeps it, and closing the last
row of an `--app` window quits that window. The old fixed 220 pt sidebar is
gone: the same renderer draws vertical rows or a horizontal row of pills.

## New-tab page

With no active tab, `render_start_page` paints the new-tab page: the app name,
one quiet search field that opens the ⌘K palette, and up to four sections from
`startpage.rs` — **Recent** (pure `last_visit` order, 5 rows), **Frequent**
(most-visited, with recent pages removed so the two lists do not repeat),
**Apps** (installed web apps from the `web_apps` table), and **Bookmarks**
(with their folder as the note). Each section caps at five rows; a row is one
line (title left, note right), and the area below the search field scrolls if a
long value ever needs it. Section order and visibility come from the
`start_sections` setting (`recent,frequent,apps,bookmarks`; `none` hides all
and leaves the title/search field). A hand-edited profile gets the lenient
reader: unknown names are ignored and an empty value falls back to the default
order. Set it deliberately with `relay config set start_sections "apps,recent"`;
the CLI rejects unknown or empty values and leaves the previous order in place.
The data is cached in `RelayApp::start_page` and rebuilt only after a visit,
bookmark change, Chrome import, install/remove, or history clear, plus the 30 s
housekeeping tick so a `relay config set start_sections` made while Relay is
open is picked up and repainted on that tick without a restart — never every
frame. With no profile store the page keeps the welcome state.
A brand-new profile whose configured sections are all empty shows the welcome
card (Import from Chrome, MCP setup); a profile with data in an unconfigured
section keeps the grid and says "Nothing yet" for the shown empty section.

## History and bookmarks library

`⌘Y` opens History and `⇧⌘B` opens Bookmarks in the same modal overlay
(`library.rs`). History uses `recent_sites` for an empty query and the store's
search for a typed one; Bookmarks are filtered across title, URL, and folder
path and sorted foldered-first. `↑`/`↓` wrap the selection, `↩` opens,
`×` deletes (one history row or every folder entry for that bookmark), and a
Bookmarks row's **move** button edits the folder path (`/` separated) before
`↩` calls `Store::move_bookmark`. The overlay hides native web views and
returns keyboard focus to the page when it closes.

## Installed web apps

The palette command **Install This Site as an App** (and File → Install This
Site as an App) sends `Op::WebApp` to the page. The bridge reads
`link[rel=manifest]`, resolves `start_url` against the manifest, picks the
largest PNG icon, fetches it as a data URL (2 MB cap), and posts the reply
as an ordinary async command result.

`webapp.rs` treats that reply as untrusted data:

- `start_url` must be http(s) and share the page's origin; the tab's own URL,
  not the page's claim, is the comparison base.
- The name is collapsed, stripped of path separators and control characters,
  capped, and falls back to the host.
- Only a real PNG data URL is decoded; the decoder is local, size-capped, and
  tested against malformed base64.
- The bundle goes to `~/Applications/<Name>.app` (with ` 2`, ` 3` on a name
  collision): `Contents/Info.plist`, a `/bin/sh` launcher, `Contents/Resources/icon.png`,
  and `AppIcon.icns` built with `sips` + `iconutil` when possible. No usable
  PNG icon (none declared, or only SVG/other formats) falls back to Relay's
  bundled icon rather than refusing an installable app. It does not copy
  Relay: the launcher runs the installed binary, so the app shares the
  browser's cookies, WebKit data store, and updates.
- `Contents/.relay-webapp` is the marker: it records the start URL and bundle
  id. Removal refuses a folder without a matching marker, so Relay never
  deletes something it did not create.
- The launcher `exec`s the installed Relay with `--app <url> --app-name <name>
  --app-icon <bundle>/Contents/Resources/AppIcon.icns` (falling back to
  `open -a Relay --args ...` if Relay moved). App mode sets that icon as the
  Dock image and uses the name for its menu and empty-page title.

The bundle id is `app.relay.webapp.<host-slug>.<fnv64>`, and the record
(`id`, name, start URL, bundle path, installed time) lives in the `web_apps`
table in `profile.sqlite` and appears in `relay export`.

`relay --app URL` opens one chromeless window: no tab strip while it has one
page, no status bar except a transient toast, no session restore or save, no
extension loading, and no MCP socket. `--app` accepts only http(s). A popup or
`target=_blank` link navigates the one page instead of creating hidden tabs.
More than one tab (only possible if a future feature adds one) brings the strip
back rather than hiding it.

## Agent audit

Every MCP tool writes one row to `agent_audit`: tool, host, tab, outcome
(ok/error), and time. It never stores typed text, cookie values, or page
content. The table keeps the newest 5,000 rows; `relay audit [--limit N]`
prints them, `relay clear-data --audit --yes` erases them, and `relay export`
includes the array. The audit write is best-effort and cannot fail a browser
action.

## Agent safety (attached mode)

- **Stop:** `⇧⌘.` (or Relay → Pause / Resume Agents) sets one atomic flag in
  `SharedHub`. Every MCP tool checks it before resolving a tab or reading the
  hub and fails fast with "paused by the user"; an `open_tab` creates nothing
  while paused. The shell repaints a banner from the hub tick and Resume clears
  the flag. It is not a page-level pause: the user can keep browsing.
- **Per-site policy:** `⌘⌥A` cycles Allow → Read only → Block for the active
  host; the value is stored in the `agent_policies` table and loaded into the
  hub at startup. Block refuses every agent tool on that host; Read only
  refuses navigation and mutating ops (`click`, `type`, `edit_style`) while
  reads/scroll/highlight continue; opening a blocked URL creates nothing.

- Agent tabs (`Owner::Agent`) share the user's cookie store and live in a
  tinted group in the tab strip.
- When the bridge reports a CAPTCHA or 2FA prompt (`challenge`) on any tab,
  agent mutations there (click, type, edit_style) fail with "waiting for a
  human"; reads keep working, and the user can still click through
  (`Actor::Agent` vs `Actor::User` in `SharedHub::begin`).
- Agent commands count as activity, so tabs an agent is using do not sleep.
  If a tab is already asleep, the next MCP call wakes it and waits for the
  reload.
- Agent tabs are kept out of the user's history.
- Pages can only open `http(s)` URLs or exactly `about:blank` in new tabs.
  Pages and agents share `relay_bridge::is_openable_url`, the single policy
  predicate; `about:blankx`, `file:`, `javascript:`, `data:`, and
  `view-source:` are refused by both.
- Agents may open only `http(s)`/`about:blank` URLs and may close only their own
  tabs.
- The MCP tools are `list_tabs`, `open_tab`, `navigate`, `go_back`,
  `read_dom`, `find_elements`, `wait_for`, `get_page_info`, `click_element`,
  `type_text`, `highlight_element`, `read_text`, `scroll`, `get_console_logs`,
  `get_network_requests`, `get_cookies`, and `close_tab`. `wait_for` polls the
  streamed DOM (no page round-trip), so an agent can wait for a result after a
  click without racing the page.
- A failed main-frame load arrives as `TabState::load_error` from
  `WKNavigationDelegate`; the shell hides WebKit's blank error page and shows a
  readable message with Retry, and MCP calls report the reason instead of a
  timeout (`wait_for_load` returns it immediately).
- The profile (`profile.sqlite`, its WAL sidecars) is 0600 inside the 0700
  `relay_home()` directory, and the temporary copy of Chrome's History is made
  0600 and deleted after import.
- The MCP socket is `relay_home()/mcp.sock`, mode 0600 inside a 0700 directory,
  so only the user's own processes connect.

## Tab sleeping

`SleepPolicy::due` picks background tabs idle for the configured timeout
(default 15 minutes; `0` disables sleeping) that are not active, visible in a
split, pinned, or playing audio (from the bridge's `media` message). It always
keeps at least one tab awake. `Engine::sleep` requests a viewport JPEG
snapshot, records URL and scroll, and tears down the `WKWebView` inside the
snapshot completion callback so in-flight snapshots are never aborted.
`Engine::wake` recreates it and marks it *warming*: the native view stays
hidden until the bridge delivers a DOM, while the shell paints the JPEG the tab
slept on. So the snapshot is what you see during the reload, not a white
WKWebView. `after_load` restores scroll once the bridge says `hello`.

## Find and zoom

⌘F opens a find bar over the content; each keystroke calls
`Engine::find` (`WKWebView findString`, wrapping, case-insensitive) and the
match answer returns as `WebEvent::Found`, so the bar can say "No matches".
Switching tabs or opening the palette closes it. ⌘= / ⌘- / ⌘0 step through
browser-style presets (50%–300%) and store the result in the host's
`SitePrefs.zoom`; every page load applies its host's zoom via
`Engine::set_zoom` (`pageZoom`), so a site keeps its zoom across tabs and
relaunches.

## Per-site blocking

The palette command "Toggle Blocking on This Site" flips the host's
`SitePrefs.blocking_off`, recompiles the rule list, and reloads the page once
WebKit has applied it (`Engine::load_content_rules_then` →
`WebEvent::RulesApplied`), never before. IPv6-literal hosts are refused (WebKit
`if-domain` cannot express them); if the recompile fails, nothing reloads and
the error is only logged.
`rules_json` appends one final `ignore-previous-rules` rule whose
`if-domain` lists every such host (`Compiled::allow_sites`, subdomains
included), so requests and element hiding on those pages are left alone.

"Toggle Compatibility Mode on This Site" flips `SitePrefs.compat_mode`: the
tab then sends a Safari-shaped user agent (`SAFARI_USER_AGENT`, via
`Engine::set_user_agent` / `customUserAgent`) for sites that reject WebKit's
default one. On every load `apply_site_prefs` reconciles the tab's agent with
its host: a compat site first fetched with the default agent reloads once
(`compat_change`, tested against loops); leaving it restores the default.
Sleeping or closing a tab forgets its agent, since a woken view starts fresh.
Known gaps: the agent is decided on the host where a load finishes, so a
site that redirects to another host before sniffing is not covered; a compat
site loads twice in a fresh view (default agent, then the reload), and that
reload can resubmit a POST that landed there.

## Cookie banners

With blocking on, the compiled rule list also includes
`content_rules::CMP_FILTERS`: `css-display-none` rules for the outer
containers of common consent platforms (OneTrust, Cookiebot, Usercentrics,
TrustArc, iubenda, Complianz, Osano, CookieYes, Klaro), by vendor-specific id
or class. Hiding a banner records no choice, so the site's configured
default applies (opt-in regions usually keep non-essential cookies off;
opt-out regions may set them). Platforms whose modal locks page
scrolling (Sourcepoint, Didomi popups, Quantcast) are left out, because a
hidden modal would leave the page unscrollable.

## Pinned tabs

`Hub::set_pinned` keeps pinned tabs as a block at the top of the order. The
sleep policy never picks them, and ⌘W on a pinned tab switches to the next tab
and sleeps the pinned one (scroll kept) instead of closing it; a lone pinned
tab stays open. Pins persist through the saved session.

## Session restore

With `restore_session` on (the default; `relay config set restore_session off`),
the shell saves the user's open `http(s)` tabs (order, title, pinned, active,
and the scroll offset of sleeping tabs) to `Store::save_session` when the app
quits (any quit path, via GPUI's `on_app_quit`) and on the 30 s sleep tick
when the set changed. Agent tabs are not saved. With it off nothing is
written, and launch erases any session an earlier run saved. On launch,
`restore_into` recreates the tabs and `Engine::restore_asleep` registers each
one as already asleep (no `WKWebView`, no snapshot); only the previously
active tab loads. The others load on first click through the normal wake path,
which restores their scroll once the real page says `hello` (`load_seq > 0`;
`page_loaded_since` keeps the restore off WebKit's initial blank document).
`relay clear-data --history` or `--session`, and Settings → Clear history,
erase the saved session.

## History visits

`visit_decision(load_seq, title, recorded)` in `app.rs` separates "a page
loaded" from "its title changed". A new `load_seq` records exactly one visit;
a title-only change updates the in-memory record and is written with the next
real load, so an SPA that rewrites `document.title` every second does not
inflate `history.visit_count` or the ⌘K frecency ranking. There is no SQL
write per title change.

## Palette smart input

Palette scoring lowercases the query once and iterates each candidate without
building a `Vec<char>` or a shortened URL string per candidate; `short_url`
still allocates only for the few items that are shown.

[`bangs.rs`](../crates/relay-ui/src/bangs.rs), [`calc.rs`](../crates/relay-ui/src/calc.rs)
and [`clean_url.rs`](../crates/relay-ui/src/clean_url.rs) are pure string
modules the palette calls before its normal URL/search item:

- **Offline !bangs**: `!gh rust lang` or `rust lang !gh` (also `rs`, `crates`,
  `docs`, `mdn`, `yt`, `so`, `npm`, `w`, `k`, `g`, `ddg`, `maps`) becomes
  `Item::Bang` at the top; an empty query opens the site homepage; unknown
  bangs fall through to the normal search. Custom bangs come from the
  `bangs` setting (`relay config set bangs "gh=https://example.com/s?q=%s
  w=https://wiki.test/?q=%s"`): space-separated `trigger=template` pairs,
  http(s) templates only, validated by `bangs::parse_custom`; they override
  built-ins and load at launch. No network lookup.
- **Calculator**: `+ - * / % ^`, parentheses, unary minus, decimals, optional
  leading `=`. Bare numbers, versions/IPs, dates, division by zero and
  non-finite results return nothing; nesting is capped at 64. The result is
  `Item::Calc` and Enter copies it.
- **Clean URL**: `clean_url::clean` strips `utm_*` and click ids (`si` only on
  YouTube/Spotify) and unwraps google `/url?q=` and facebook `l.php?u=`
  redirects. Backs the `Copy Clean URL` command (⌘⇧C). `Copy Page as
  Markdown` (⌘⇧M) runs `Op::Markdown` on the active tab, waits up to 3 s for
  the bridge's reply, and copies it; `Close Duplicate Tabs` runs in `app.rs`.

## Checking Relay off-Mac

Six layers, cheapest first:

1. **Portable tests** (`cargo test --workspace`) run anywhere: protocol, hub,
   MCP round-trip with a real client, Chrome import, CRX, content rules, sleep,
   palette, inspector filters, hints, WCAG AA theme contrast, key bindings, and
   CLI subcommands (`doctor`, `config`, `export`, `clear-data`). The stdio
   transport itself is covered by `crates/relay-ui/tests/mcp_proxy.rs`, which
   spawns the real `relay mcp` binary, connects it to a socket-backed hub, and
   performs the initialize / tools list / list_tabs handshake.
2. **The injected bridge runs in Playwright's WebKit 26.0** in Linux CI
   (`js/`, 12 tests): DOM snapshots and patches, fetch/XHR status and bodies,
   console and uncaught errors, click/type/highlight, hints, 2FA detection, and
   the async `web_app` manifest/icon read with its failure reasons.
   CI installs WebKit and its host libraries on ubuntu-24.04.
3. **The GPUI shell builds on Linux** with GPUI's system libraries
   (`libxkbcommon`, wayland, xcb, fontconfig, freetype); CI installs them and
   runs `cargo check -p relay-ui` through the full suite. Its macOS-only
   `web.rs` is compiled and linked in the macOS job.
4. **The Darwin type-check** is `bash scripts/mac-check.sh`: it installs the
   Apple standard library when rustup is available and type-checks
   `relay-webkit`, `relay-extensions`, `relay-mcp`, including the `smoke`
   example, with `--no-default-features` so `libsqlite3-sys` does not invoke a
   Darwin C cross-compiler on Linux. It proves API use, not behaviour.
5. **The macOS job** (`.github/workflows/macos.yml`) runs on Apple Silicon
   `macos-15` (on pull requests, pushes to `main` touching browser paths,
   published releases, and `workflow_dispatch`): it caches Cargo artifacts and
   Playwright WebKit by lockfile, runs `cargo test --workspace`, the real-WebKit
   `smoke` example (hosted in an offscreen borderless `NSWindow`), and the
   bridge suite on Apple's WebKit, then packages `Relay.app` (with
   `assets/AppIcon.icns`) and `Relay-macOS.dmg` via
   [`scripts/bundle-macos.sh`](../scripts/bundle-macos.sh), checks `relay
   doctor`, uploads the DMG/zip as workflow artifacts, and publishes them to
   GitHub Releases (`macos-latest` on `main`, or the release tag) via
   [`scripts/publish-macos-release.sh`](../scripts/publish-macos-release.sh).
   On the owner's Mac,
   [`scripts/verify-macos.sh`](../scripts/verify-macos.sh) is the one-command
   equivalent and writes `dist/macos-verification.txt`.
6. **Manual keyboard review** (`crates/relay-ui/review/keyboard_e2e.sh`) runs
   the binary under Xvfb and drives it with `xdotool`, reading state back over
   MCP (`list_tabs`); it covers ⌘K/⌘T palette navigation, URL-vs-search, ⌘W,
   ⌘⇧T, and Esc. It needs `Xvfb`, `xdotool` and `python3`. Screenshots are not
   possible on headless VMs without a working Vulkan display: under Xvfb and
   Xorg-dummy, llvmpipe presents black frames even for `vkcube`. Visual review
   therefore happens on a Mac.

## Chrome extension import

`read_extensions` orders version directories numerically (`1.10` after `1.9`)
and falls back to the next readable manifest, so one corrupt extension cannot
discard the rest of the preview. `relay install-extension` prints the
manifest's requested APIs, optional APIs, and host match patterns from one
`PermissionReport`; installing grants all of them, and the CLI says so.

## Favicons

`Op::Favicon` asks the page for its largest `<link rel=icon>` (or
`apple-touch-icon`), then `/favicon.ico`; the bridge fetches with
`credentials: "same-origin"`, a 3 s timeout, a 256 KB cap, and a
PNG/JPEG/WebP/GIF mime whitelist. SVG/ICO/text are refused. The shell decodes
at most one icon per host, caches it in memory and 0600 files under
`relay_home()/favicons/` (sanitized host names), and shows it in tab rows and
start-page rows. Starts with no icon get the deterministic dot/empty fallback;
a failed fetch is not retried for that host this session.

## Agent and Inspector sidebar

`⌥⌘I` toggles a 380 px right sidebar; × collapses it and returns focus to the
page. Its **Agent** tab shows MCP on/off, the socket, paused/accepting state,
the active site's policy, Pause/Resume, Cycle site access, and a click-to-copy
setup command. The **Inspector** tab keeps the Elements/Network/Console/
Cookies panels and their filters. The sidebar sits beside the content at every
tab-strip placement (top/left/right) and does not cover native views.

## Keyboard shortcuts

`keymap.rs` is the one action table: 46 defaults, stable ids, labels, and
GPUI bindings. Settings → Keyboard shortcuts lists every action; **Change**
clears bindings and menus, captures the next keystroke, and rejects bare keys,
conflicts (including `secondary` vs `Ctrl` on Linux), and `⌘Q` for anything but
Quit. Overrides persist as `id=keystroke;…` in `settings.keybindings`, reload
at startup, and rebuild menus immediately so macOS key equivalents match.
`app::key_bindings()` delegates to the default map, so the old duplicate table
is gone.

## Appearance

Settings → Appearance offers System / Light / Dark and a Blue / Purple /
Green / Orange accent (`theme::Theme::for_choice`). Both choices are stored in
`settings` (`theme`, `accent`) and apply immediately; the palette also has
Cycle Appearance and Cycle Accent commands. Contrast tests cover every accent
against both surfaces.

## CLI safety

`relay import-chrome` parses `--dry-run`, `--profile DIR`, and `--help`
strictly: unknown flags, a missing value, and a flag where a directory belongs
are usage errors, and `--help` returns before the profile is opened. `relay
doctor` connects to `mcp.sock` before calling it live, names a leftover file
as stale, and prints the protocol version from
`relay_bridge::PROTOCOL_VERSION`, the same constant the page `hello` carries.

## Build notes

- Minimum macOS: 13.3 (`WKWebView.isInspectable`). Extensions need 15.4
  (`WKWebExtension`); on older systems Relay runs without them.
- `assets/AppIcon.icns` is generated deterministically from Relay's design
  tokens by [`scripts/make-icon.py`](../scripts/make-icon.py) and bundled into
  `Relay.app` by [`scripts/bundle-macos.sh`](../scripts/bundle-macos.sh).
- Dev-profile dependencies build at `opt-level = 0` to keep CI cold builds
  short; release builds keep `opt-level = 3`, `lto = "thin"`, and `strip = true`.
- `cargo test --workspace` runs everywhere. On Linux,
  `bash scripts/mac-check.sh` (`cargo check --workspace --exclude relay-ui
  --no-default-features --target aarch64-apple-darwin`) type-checks the macOS
  code in `relay-webkit` and `relay-extensions` (including the `smoke`
  example); the script skips with a message when the Apple std is not
  installed. `relay-ui` cannot be checked for the Darwin target on Linux:
  gpui's macOS build compiles C and Metal code (`ring`, shaders), which needs
  the Apple SDK (covered by `.github/workflows/macos.yml`).
- `relay-extensions` bundles SQLite by default (`bundled-sqlite` feature) on
  both macOS and Linux because `rusqlite` 0.43 uses `sqlite3_set_clientdata`
  (SQLite 3.44.0+), which macOS 15's system SQLite 3.43.2 lacks. Linux's Darwin
  cross-check disables that feature (`--no-default-features`). `ureq` uses
  native TLS (Security.framework on macOS), so no C cross-compilation is needed.
- Linux builds of the `relay` binary need GPUI's system libraries (xkbcommon,
  wayland, xcb, fontconfig, freetype, vulkan headers). Linux CI installs the
  subset it needs on ubuntu-24.04 (Playwright 1.56 does not support 26.04).
- `third_party/xattr` is a patched copy of xattr 0.2.3 that fixes a Linux build
  break in gpui's dependency tree ([`../third_party/README.md`](../third_party/README.md)).
