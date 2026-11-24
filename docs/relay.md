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
| `relay-webkit` | Content-blocker compiler (EasyList subset → WebKit JSON), tab-sleep policy, download naming policy (`downloads.rs`); `mac.rs`: `Engine` creating one `WKWebView` per awake tab with the bridge injected, rule lists, cookies, snapshots, the Web Inspector fallback, and the page's own context menu (with a Reading Mode item that drives the bridge) | policy portable, engine macOS |
| `relay-extensions` | Chrome importer (Bookmarks JSON, History SQLite, extension ids, password CSV), CRX3 download and unpack, manifest compatibility report, SQLite profile `Store`; `login.rs`: pure origin, candidate, fill, and save/decline model with redacted, zeroized secrets; `mac.rs`: WKWebExtension loading, Keychain import | importer portable, loaders macOS |
| `relay-mcp` | `RelayMcp` (rmcp 3.x tool router), user-only Unix socket server, `relay mcp` stdio proxy | portable (Unix) |
| `relay-ui` | The `relay` binary. `app.rs`: the GPUI shell (native menu bar, tab strip with close buttons/agent group/fold/move plus explicit Left/Top/Right items, panes and split, ⌘K palette, a ten-tab Settings overlay (General, Appearance, Tabs, Browsing, Passwords, Privacy, Extensions, Agents, Shortcuts, About), an optional bookmarks bar, an always-visible-close option, a default-browser request through LaunchServices, LaunchServices website-data clearing, and per-tab media mute, Inspector with console/network filters, link hints, a hideable status bar, a 16 ms pump draining MCP requests, WebKit events and hub changes, history recording, tab sleeping, web app install/remove). `web.rs`: `WebHost` over the `Engine` (a no-op off-Mac). `tabstrip.rs`: placement/fold logic. `startpage.rs`: new-tab sections and rows. `webapp.rs`: manifest validation, same-site start URLs, a signed/registered `.app` bundle writer whose launcher resolves Relay at launch. `palette.rs`, `inspector.rs`: tested view logic. `theme.rs`: shading tokens (background/surface/raised/well) from [`design.md`](./design.md) with WCAG AA contrast tests; the chrome uses surfaces and shadows instead of 1 px outlines. `main.rs`: the CLI (`mcp`, `--app`, `import-chrome`, `import-passwords`, `install-extension`, `compile-rules`, `config`, `export`, `clear-data`, `doctor`) | GPUI everywhere; WebKit only on macOS. Linux shows a placeholder pane |

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

### Credential path

Passwords enter Relay only through `relay import-passwords` (CSV to Keychain)
or through the native login path (#57, implemented; hardening continues in
#186). The pure model in `relay-extensions/src/login.rs` owns origin binding,
candidate matching, and redacted secrets; the native handler applies the
explicit fill click and the save/decline choice. The richer `FillOffer` /
`FormSave` state machine is unit-tested design and is not wired into the
runtime yet. No bridge message, DOM snapshot, MCP tool, or audit row may carry
a password. The design and open owner decisions are in
[`login-autofill.md`](./login-autofill.md).

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
one quiet search field that opens the ⌘K palette (command rows read the live
keymap, so custom shortcuts show immediately), and up to four sections from
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

## Stdio proxy

`relay mcp` keeps its socket write half alive after stdin EOF and waits up to
5 s for the server's final reply before exiting, so an agent that closes stdin
immediately after a request does not lose the response. The socket read half
is still copied concurrently.

## Navigation failures

`webView:didStartProvisionalNavigation:` clears the tab's previous DOM, title,
console, network, error, challenge, and hotkeys before the new document
arrives. A failed after-load navigation therefore cannot leave stale page
state under a load error; the shell's existing error pane and Retry button show
because the DOM is empty.

## Extension install limits

`relay install-extension` bounds every read: the HTTPS fetch has a 30 s
timeout and 256 MiB compressed cap, CRX2/3 headers are checked without
panicking and must be followed by a zip payload, and unpack refuses more than
256 MiB decompressed total and removes its staging directory on any failure.
CRX3 signatures are still not verified (residual).

## Downloads

A navigation WebKit cannot display is accepted as a `WKDownload`; the delegate
saves it under `~/Downloads/Relay` with a sanitized collision-safe name and
records URL/filename/path/state and the owning tab in the hub. The naming
policy (`relay-webkit/src/downloads.rs`) is portable: it keeps only the last
path component of the server's suggestion, replaces control characters and `:`,
turns dot-only names into `download`, and appends `(n)` instead of overwriting.
Its unit tests run on every platform, including Linux CI, and cover traversal
(`../`), absolute paths, reserved names, and collisions; only the `WKDownload`
delegate (`mac.rs`) needs macOS. `⇧⌘J` (View → Downloads, palette) opens a panel
with **Active** and **Finished** sections, the filename, state/URL, and the
destination path (home shortened to `~`; the row ellipsizes overflow; before
WebKit chooses a destination the row says where files save), Cancel for a
running row, Retry for a failed/cancelled row, Reveal in Finder, and Clear
finished. `Hub::downloads_in_panel_order` is the one display order — running
rows first, then finished rows, oldest first within each section — so headings,
rows, and keyboard selection agree. Running rows show native byte progress
(received/total and percent) from `WKDownload.progress`, polled at 500 ms while
any row is active. Cancel marks the row immediately and calls
`WKDownload.cancel` through the tab's bridge handler; Retry navigates the
owning tab to the original URL so WebKit decides again whether it is a
download, creating a fresh row when it is. ↑/↓/Home/End move the selection in
panel order and ↩ activates the selected row (Cancel running, Retry failed,
Reveal done). The metadata is in memory only; the file is the user's, outside
the profile. The panel and the rest of the GPUI chrome have no VoiceOver
labels: GPUI 0.2.2 exposes no accessibility tree, so this residual is #64, not
a downloads-model gap.

## Picture in Picture

`⇧⌘P` (View → Picture in Picture, palette) asks the page to toggle its first
`<video>` through WebKit's presentation mode or the standard API. It is a
page-local action; if the WebView/page has no video or no support, the toast
still says "requested" and nothing else changes. Reading mode and PiP together
complete the Search-parity page tools.

## Reading mode

`⇧⌘R` (View → Reading Mode, palette) asks the page for a reader overlay built
from `<article>`/`[role=main]`/`<main>`/`<body>`. Scripts, styles, forms,
nav/aside/footer, inline handlers, and copied Relay ids are removed; images and
links remain. A second press removes it, and a page load clears the per-tab
state. PiP for video is not built yet.

## MCP install board

`relay mcp-config [claude-code|codex|opencode|cursor|zed]` prints the exact
snippet and target path for each client; without a client it prints all five.
Settings → Connect an agent has the same five rows with a Copy button. Relay
only copies text and changes no client file, so a stale format cannot corrupt
a config. `--install` is deliberately not offered until each format is
verified against a live client.

## MCP tool: screenshot

`screenshot {tab_id?}` captures the visible tab through WKWebView's snapshot API
as a JPEG (5 s timeout) and returns it as MCP image content. It is read-only,
obeys Block/Stop, and writes one audit row; the image itself is not stored or
audited.

## MCP tool: set_cookie

`set_cookie {tab_id?, name, value}` writes one cookie on the tab's own origin
with `Path=/; SameSite=Lax` (Secure on https). It is mutating, so Read-only/
Block policies refuse it; it cannot set another domain or HttpOnly, and rejects
names outside `[A-Za-z0-9_.-]`, newlines, and >4 KB values. The audit row has
only tool/host/tab/outcome/time, never the value.

## MCP tool: eval_js

`eval_js {tab_id?, script}` evaluates one JavaScript expression in the page and
returns its JSON value. It is off by default: Settings → Agent JavaScript
(`agent_eval`) must be on and the site policy must be Allow; Stop/Block/Read-
only and the disabled state all fail before evaluation. Results are capped at
100 KB, the script at 50 KB, and the audit row contains only tool/host/tab/
outcome/time.

## MCP waits

`get_cookies` records the tab's `cookies_seq`, asks WebKit for a refresh, and
waits on the hub watch for a newer sequence (2 s cap) instead of sleeping
150 ms. `wait_for` checks once, then waits on the same hub notification and
re-checks only when state changes, with the caller's timeout; it no longer
polls the whole tree every 50 ms.

## Shell plan/apply (settings, shortcuts, start page, sidebar, theme)

Settings → Connected agents and the Agent sidebar both list each MCP client
that has initialized, with a per-client Grant/Revoke; the global Agent shell
changes switch overrides every grant. A Revoke is read on the next tool call:
the MCP re-reads `mcp_clients` on every plan/apply, so no browser restart is
needed. When a client is granted (or the global switch is on), it can call
`shell_plan_change` to get a one-time id and a human diff, then
`shell_apply_change {plan_id}`. A plan expires after 60 s, is single-use, and
at most 32 pending plans are kept (expired ones are pruned first).

Plan fields are bounded: `theme` (`system`/`light`/`dark`), `accent` (the four
named accents), `start_sections` (an ordered unique subset of
`recent`/`frequent`/`apps`/`bookmarks`), at most eight allowlisted `settings`
(`search`, `blocking`, `sleep_minutes`, `tabs_position`, `tabs_folded`,
`restore_session`, `keybindings`; each value ≤4096 bytes and trimmed before
storage), a sidebar target
(`inspector`, `agents`, or `off`), and one small start-page widget
(`title` ≤80 chars, `body` ≤280; `visible: false` clears it). The widget is
plain text rendered by GPUI, never HTML; angle brackets and `javascript:` are
refused before a plan exists. The shell has no URL, HTML, script, or
plugin parameter, and unknown JSON fields are rejected before validation.
Start sections and the widget reject control characters; unknown sections
(including URLs or markup) are refused. The only theme tokens accepted are the
named light/dark/accent pairs whose WCAG AA contrast is tested in
`relay-ui/src/theme.rs`; a raw colour or unlisted name is refused. The MCP
therefore cannot choose a palette that has not passed AA.

Shortcut overrides are checked in the MCP against the shared action-id table
and the live default keymap: unknown ids, repeated action ids, duplicate keys,
default-key conflicts, bare keys, and ⌘Q misuse are rejected before a plan
exists. On Linux `secondary` and `ctrl` are compared as the same modifier, as
`Keymap::from_setting` does. Apply writes the Store, pushes the `ShellChange`s
the GPUI pump drains (theme/accent/settings update live, and a `keybindings`
change rebuilds GPUI bindings and the menu bar through the same
`apply_keymap` path as Settings; start sections
rebuild; sidebar/widget update), and audits `shell_apply` with the MCP client
name, plan id, an `ok` result, and 64-bit before/after hashes for every changed
key including sidebar and widget state (values never enter the audit).
Denied and failed applies write a compact audit outcome too; unknown or
expired ids return a stable error and write no audit row because nothing was
applied. `mcp`, `shell_control`, `agent_eval`, `mcp_socket`,
`mcp_socket_path`, and `clear_data` are reserved. Off by default; no arbitrary
code or shortcut writes.

## Shell read tools

The MCP also exposes read-only shell state: `shell_settings` (a bounded
allowlist), `shell_shortcuts` (the raw `keybindings` override, its length, and
every known action with its default key), `shell_start_page` (section order),
`shell_sidebar` (the live open flag and pane read from the hub), `shell_apps`
(installed web apps, ≤50, with capped id/name/URL), and `shell_version`. They
never read page content, cannot write or execute code, and write one audit row
each. Plan/apply is the only shell write path, and it requires a grant.

## MCP tools: batch actions

`batch_actions {tab_id?, actions:[…]}` runs sequential `click`, `type`,
`press_key`, `scroll`, or `wait` steps in one tab. It stops at the first
failure and reports the completed count; every inner op still passes the Stop
and per-site policy checks and is audited, plus one `batch_actions` row.

## MCP tools: keys and storage

`press_key {tab_id?, key, modifiers?}` dispatches a key to the focused element
with ctrl/alt/shift/meta; it is a mutating op, so Read-only/Block policies
refuse it. `get_storage {tab_id?}` reads localStorage/sessionStorage, redacting
names that look like secrets and capping values at 16 KB/200 keys. Both write
one audit row, never the key or storage values.

## Agent audit

Every page tool, shell read, shell plan, and applied or denied shell apply
writes one row to `agent_audit`: tool, host, tab, outcome, and time. Page
tools use `ok`/`error`; shell plan/apply use `ok`/`error`/`denied`, and a
successful apply carries the client name, plan id, and before/after hashes. An
unknown or expired plan id returns an error and writes no row because nothing
was applied. The log never stores typed text, cookie values, setting values,
widget text, or page content. The table
keeps the newest 5,000 rows; `relay audit [--limit N]` prints them,
`relay clear-data --audit --yes` erases them, and `relay export` includes the
array. The audit write is best-effort and cannot fail a browser
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
  (`Actor::Agent` vs `Actor::User` in `SharedHub::begin`). The shell shows a
  "waiting for you" banner above the page; **Continue** clears the challenge
  and returns focus to the page so the next agent call can proceed. This
  follows browse's human-handoff pattern without its Chromium/CDP stack.
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

Filter lists use EasyList syntax. A `$domain` list that mixes includes and
excludes (`$domain=news.test|~blog.news.test`) compiles to the include block
plus an ordered `ignore-previous-rules` exception for the excluded hosts,
because WebKit accepts only one of `if-domain`/`unless-domain` per trigger.
The exception also relaxes earlier rules with the same URL filter on the
excluded host, since WebKit cannot scope it to one block. An element-hiding
scope that mixes them (`news.test,~blog.news.test##.ad`) is skipped and
counted: the only WebKit exception would cancel network blocking on that page
too.

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

## Palette editing

The query has a byte-index cursor on a character boundary: ←/→/Home/End move
it, ⌫ deletes the character before it, Delete removes the one after,
Shift+←/→ extends a selection, Option+←/→ moves by word, ⌘A selects all,
⌘C/⌘X copy/cut, ⌘⌫/⌥⌫ clears all, pasted text replaces a selection, and
clicking the shaped line places the cursor. The caret and selection background
are drawn at the model positions. A custom text element calls
`Window::handle_input` during paint and implements `EntityInputHandler`: IME
marked text is stored as a byte range, underlined, and mapped to/from UTF-16;
`bounds_for_range` positions the candidate window. `on_key` stops inserting
`key_char` while that handler is active, so committed text is not duplicated.
The real Japanese/Chinese input-source check on a Mac remains the external
evidence for #14.

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

1. **Portable tests** (`cargo test --workspace`, 265 test functions) run anywhere: protocol, hub,
   MCP round-trip with a real client, Chrome import, CRX, content rules, sleep,
   palette, inspector filters, hints, WCAG AA theme contrast, key bindings, and
   CLI subcommands (`doctor`, `config`, `export`, `clear-data`). The stdio
   transport itself is covered by `crates/relay-ui/tests/mcp_proxy.rs`, which
   spawns the real `relay mcp` binary, connects it to a socket-backed hub, and
   performs the initialize / tools list / list_tabs handshake. Test code uses
   `Store::in_memory`, gated behind the `test-support` feature enabled only by
   dev-dependencies; it is not in the production API.
2. **The injected bridge runs in Playwright's WebKit 26.0** in Linux CI
   (`js/`, 23 tests): DOM snapshots and patches, fetch/XHR status and bodies,
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

`Manifest::compatibility().report_lines()` is the one install report: shimmed
APIs, partial support (for example `downloads`/`nativeMessaging`), missing
APIs, host match patterns, and MV2/background warnings. `installable()` was
removed; install still proceeds after printing the full list.

`read_history` copies Chrome's `History` and its `-wal`/`-shm` sidecars
together into a 0600 temp database, reads the same snapshot (including rows
not yet checkpointed), and removes every copy. A writer can still be racing
the copy, but committed WAL rows are no longer silently missed.

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

`keymap.rs` owns one action table: the `action_table!` invocation lists each
action once (stable id, Settings label, default keystroke, GPUI action type)
and expands to `ActionId`, `ALL`, every accessor, and the GPUI bindings, so a
new shortcut is one row. 49 actions have defaults. Settings → Keyboard
shortcuts lists every action; **Change** clears bindings and menus, captures
the next keystroke, and rejects bare keys, conflicts (including `secondary` vs
`Ctrl` on Linux), and `⌘Q` for anything but Quit. Overrides persist as
`id=keystroke;…` in `settings.keybindings`, reload at startup, and rebuild
menus immediately so macOS key equivalents match. `app::key_bindings()`
delegates to the default map, and `.agents/keymap-actions.txt` (the MCP's
allowed-id list) is checked against the same table.

`palette.rs` owns one command table for the ⌘K rows: `command_table!` lists
each command once with its palette title and, when it has one, the keymap
action behind it. `Command::keys()` is derived from that action's default, so
the palette cannot advertise a shortcut that no binding uses; the nine
palette-only commands (Import, Copy MCP setup, close duplicates, the two
per-site toggles, install / remove web app, theme and accent cycles)
deliberately have no action and show no key. Tests assert unique labels and
ids, every command mapped, exactly 28 commands bound to actions, bindings
built from the table, and that the keyboard map in
[`docs/design.md`](./design.md) matches the action table row for row.

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
  subset it needs on ubuntu-24.04. The image and the pinned Playwright
  lockfile are validated together; moving to a newer Ubuntu image is a
  separate CI change with its own WebKit run.
- `third_party/xattr` is a patched copy of xattr 0.2.3 that fixes a Linux build
  break in gpui's dependency tree ([`../third_party/README.md`](../third_party/README.md)).
