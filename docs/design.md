# Design

<!-- index: design | Visual direction, layout, keyboard map, and UX principles | before any UI work -->
<!-- covers: crates/relay-ui/src/theme.rs crates/relay-ui/src/tabstrip.rs crates/relay-ui/src/startpage.rs crates/relay-ui/src/app.rs -->

Direction (owner brief, 2026-11-22): **minimal, keyboard-first, native macOS**.
The look is closer to Zed than to Chrome. The page gets almost the whole window,
the chrome is quiet, and every action is a shortcut or a ⌘K entry.

## Layout

```
┌──────────┬────────────────────────────────────────┬──────────────┐
│ TABS  «  │                                        │ Inspector    │
│ ● docs  ×│          WKWebView (active tab)        │ (⌘⌥I, 380pt) │
│ ○ gh    ×│                                        │ Elements     │
│ ── Agent │      split: ⌘\ adds a second pane      │ Network      │
│ ◆ shop  ×│                                        │ Console      │
├──────────┴────────────────────────────────────────┴──────────────┤
│ status: url · blocked count · agent connected                    │
└──────────────────────────────────────────────────────────────────┘
        ⌘K palette floats centred over everything (560 pt wide)
```

- **Tab switching:** Ctrl+Tab / Ctrl+Shift+Tab walk the current space in
  most-recently-used order; the MRU list is per session and drops closed tabs.
- **Tab strip:** a vertical list of title rows with status dots, an optional
  54 pt icons-only rail, or the same rows in a horizontal strip (`⇧⌘S`). Tabs can carry one named
  group: a heading with a colour dot, name, count, and collapse chevron; the
  tab menu moves a tab in or out, and ⌥⌘[ / ⌥⌘] fold or unfold every group. The active tab and the
  row under the pointer show `×` (a middle click closes too). The user's tabs
  come first, then an "Agent" group header with tinted rows for agent-owned
  tabs. Asleep tabs are dimmed. A tab waiting for a human shows ⚠. `⌘S` folds
  the strip to a thin, clickable rail (14 pt at the side, 10 pt at the top)
  that brings it back; the edge and folded state persist in `tabs_position`
  and `tabs_folded`.
- **AI:** Settings → AI stores an OpenAI-compatible endpoint/model and an API
  key in the login Keychain. ⌘⇧A opens a chat that sends the current URL,
  title, up to 20,000 characters of visible page text, and the question, then
  shows the reply; Esc closes.
- **Motion:** palette, Settings, and Library fade in over 140 ms with
  ease-out-quint; no other transitions are animated yet.
- **Palette:** also exposes duplicate, mute, and sleep tab commands.
- **Palette:** one input with ranked rows (open tabs, history, bookmarks,
  commands). The first row is always "Open <typed URL>" or "Search <query>".
  The query is a real text input: GPUI's platform handler supplies IME
  composition, marked text is underlined, and the candidate window follows the
  shaped line.
- **Inspector:** a tab strip of four panels over monospace rows. Hovering an
  Elements row highlights the node in the page.
- **Sidebar:** a 380 px right sidebar with **Agent** and **Inspector** tabs and
  a × collapse button. Agent shows MCP state/socket, Stop/Resume, per-site
  access, per-client shell-write Grant/Revoke, the copyable setup command, and
  the untrusted-page reminder; Inspector keeps the
  Elements/Network/Console/Cookies panels. The sidebar
  works at every tab-strip placement.
- **Agent stop:** while an agent is paused, a tinted line above the status bar
  says page automation is stopped and offers Resume. The state lives in the
  shared hub, so every MCP tool fails fast before touching a page.
- **Tab icon:** a 14 px favicon, decoded from the page's own icon link
  (PNG/JPEG/WebP/GIF under 256 KB); a dot is the fallback while loading,
  after a refusal, and for sites without an icon. Active tabs keep the accent
  ring/background.
- **Library:** a modal over the page for History (`⌘Y`) or Bookmarks
  (`⇧⌘B`): one query line, wrapped selection, row click opens, × deletes, and
  Bookmarks rows have rename and move prompts; Settings can export/import the
  whole set as Netscape bookmark HTML. Esc or click-outside
  closes and returns focus to the page.
- **Downloads:** the destination defaults to `~/Downloads/Relay`, can be set
  with `relay config set downloads_dir PATH` or the Settings → General
  **Choose…** folder picker, and Settings can ask for a save panel per file. The modal (`⇧⌘J`) has **Active** and **Finished**
  sections. A row shows the filename, the state with byte progress and the
  source URL, then the destination path with the home directory as `~`; long
  text ellipsizes instead of widening the panel. A running row offers Cancel, a
  failed row Retry, and a finished row Reveal; Clear finished and the empty
  state (which says where files save) live with the panel. ↑/↓/Home/End move the
  selection in section order and ↩ runs the selected row's action.
- **Import:** the welcome card and Settings import Chrome, Brave, Edge, Arc,
  Dia, or Chromium bookmarks/history, plus Safari bookmarks/history (Safari
  data needs Full Disk Access).
- **New tab:** with no active tab, a quiet page with **no wordmark**, one
  search field (opens ⌘K), and two columns of Recent / Frequent / Apps /
  Bookmarks rows. Rows are one line with a muted note; empty sections say "Nothing yet";
  only a genuinely fresh profile shows the welcome card. `start_sections` owns the order and `none` hides the page's sections.
- **Page context menu:** Back, Forward, Reload, Copy Page URL, Reading
  Mode, Print, and Inspect Element, with the page's own link/image items
  preserved. The shell draws the tab menu as a floating panel when the strip
  is horizontal.
- **Float video:** ⌘⇧F or View → Float Video pins the first `<video>` to a
  fixed 16 pt corner overlay with a shadow; the same command restores the
  original style. ⌘⌥⇧F opens a native always-on-top panel with the page's
  current video URL; PiP remains a separate native option.
- **Media keys:** the system play/pause key controls the first media element
  in the active tab; the native MediaPlayer handlers only enqueue a request,
  which the shell maps to `Op::media` on its main-thread pump.
- **Extensions:** enabled extensions appear as a compact right-aligned
  toolbar (WebKit action icons, initial fallback, plus a gear); clicking one opens its action popup,
  and **Show extension toolbar** in Settings can hide the row. Settings → Extensions has an inline Chrome Web Store
  URL/ID field, a clipboard fallback, enable/disable, permission text, and
  load-error text, an **Options** button for extensions that declare an
  options page, and a **Popup** button that presents the default action's
  `popupWebView` (only when `presentsPopup` is true) in a floating panel. It never claims a failed load succeeded.
- **Export:** File → Export as PDF… renders the page through WebKit's PDF
  pipeline and opens a save panel with the page title.
- **Menu bar:** native macOS menus (Relay, File, Edit, View, History,
  Bookmarks, Tab, Window, Help) carry every shortcut; View names Tabs on the
  Left, Top, and Right explicitly, not only the cycling command.
- **Address bar:** off by default (absence of the setting means off); Settings → Appearance adds a 26 pt URL line
  above the page. It shows a lock/warning, host, path, and ⌘L; clicking opens
  the palette with the URL selected so typing replaces it.
- **Bookmarks bar:** off by default (absence means off); Settings → Tabs turns on a 24 pt row of
  saved pages under the strip, optionally icon-only or only on the new-tab page. Each row is a site icon and title, click to
  open; ⌘D adds the current page.
- **Default browser:** Settings → General → Make default asks LaunchServices
  to open http and https links with Relay; macOS shows its own confirmation.
- **Status line:** hovering a link replaces the status URL with `→ target`;
  leaving the link restores the page URL.
- **Site card:** clicking the status-bar URL/host opens a compact panel with
  per-site zoom (in/out/reset), blocking, agent policy, compatibility mode, and
  camera/microphone choices with Forget. Esc or an outside click closes it.
- **Site permissions:** camera/microphone requests show a native Allow/Block
  alert naming the origin; ordinary tabs remember the choice across launches
  (Settings → Privacy → Forget clears it), private tabs ask every time.
  Notifications/location memory is not built.
- **Privacy:** Settings → Privacy hides consent banners with the blocker's CMP
  selectors, and Clear cookies & site data needs a second click because it
  signs out of every site.
- **Browsing:** Settings → Browsing sets the default page zoom for sites that
  have no saved zoom; per-site ⌘=/⌘- still wins.
- **Tab context menu:** pin, duplicate, mute/unmute, sleep, close others, and
  close, with no outline around the menu. AppKit checks menu key
  equivalents before the first responder, so they work while WKWebView has
  focus; `Edit` forwards `⌘X`/`⌘C`/`⌘V`/`⌘A` to the page.
- **Web app window:** an installed site runs `relay --app <url>`: one page,
  no tab strip or status bar (a status line appears only for a toast), no
  session restore, and no MCP socket.

## Tokens

| Role | Dark | Light |
|---|---|---|
| background | `#1b1c1f` | `#f6f6f7` |
| surface (tab strip, panes) | `#232428` | `#ffffff` |
| border | `#34363b` | `#e2e2e6` |
| text | `#e6e6e9` | `#1c1c1f` |
| muted text | `#9a9ca3` | `#6b6d75` |
| accent (focus, active tab) | `#5b8cff` | `#2f6bff` |
| agent tint | `#b07cff` at 18% | `#7a3cff` at 12% |
| error | `#ff6b6b` | `#c92f2f` |
| warn | `#ffb84d` | `#8a5300` |

Appearance is System (follow macOS), Light, or Dark; the accent is Blue
(default), Purple, Green, or Orange. `theme::Theme::for_choice` applies the
accent's light/dark pair, and every accent is checked at 3:1 against the
surface. Settings → Appearance changes it live and stores `theme`/`accent`. The
Settings rail has eleven pages: General, Appearance, Tabs, Browsing, Passwords,
Privacy, Extensions, Agents, Shortcuts, and About. Every row is a surface on
the background, not a boxed cell; only the groups' spacing separates them.
Tabs also offers site-icon/letter badges, compact rows, a bookmarks bar, and
always-visible close buttons.

Type: system UI font at 13 pt for chrome and 12 pt monospace for the Inspector.
Corner radius is 6 pt and spacing uses a 4 pt grid. Motion is 150 ms, and is
none under Reduce Motion. Surfaces carry the hierarchy: `background`,
`surface`, `raised` (overlays and cards), and `well` (inputs), with shadows
for elevation. A 1 px outline is not the default separator; only the tab-strip
edge, split divider, and status edge keep a hairline.

## Keyboard map

Every shortcut below is one row of the action table in
`crates/relay-ui/src/keymap.rs`; `design_keyboard_map_matches_the_action_table`
fails when this block and the table disagree. The palette (`palette.rs`) has
its own one-row-per-command table joined to these actions, so command labels
and displayed shortcuts cannot drift either.

<!-- keymap:start -->
| Keys | Action |
|---|---|
| ⌘K | Command palette |
| ⌘L | Open location |
| ⌘T | New tab |
| ⌘W | Close tab |
| ⌘⇧T | Reopen closed tab |
| ⌘R | Reload page |
| ⌘[ | Go back |
| ⌘] | Go forward |
| ⌃Tab | Next recent tab |
| ⌃⇧Tab | Previous recent tab |
| ⌘⌥I | Toggle Inspector |
| ⌘⌥U | WebKit Web Inspector |
| ⌘\ | Toggle split view |
| ⌘, | Settings |
| ⌘J | Link hints |
| ⌘⇧C | Copy clean URL |
| ⌘⇧M | Copy page as Markdown |
| ⌘D | Bookmark this page |
| ⌘⌥P | Pin / unpin tab |
| ⌘F | Find in page |
| ⌘⇧F | Float video |
| ⌘⌥⇧F | Float video in panel |
| ⌘⇧A | Ask AI |
| ⌘G | Find next |
| ⌘⇧G | Find previous |
| ⌘= | Zoom in |
| ⌘- | Zoom out |
| ⌘0 | Actual size |
| ⌘Q | Quit Relay |
| ⌘1 | Select tab 1 |
| ⌘2 | Select tab 2 |
| ⌘3 | Select tab 3 |
| ⌘4 | Select tab 4 |
| ⌘5 | Select tab 5 |
| ⌘6 | Select tab 6 |
| ⌘7 | Select tab 7 |
| ⌘8 | Select tab 8 |
| ⌘9 | Select tab 9 |
| ⌘S | Fold / unfold tab strip |
| ⌘⇧S | Move tab strip |
| ⌘X | Cut |
| ⌘C | Copy |
| ⌘V | Paste |
| ⌘A | Select all |
| ⌘Z | Undo |
| ⌘⇧Z | Redo |
| ⌘M | Minimize window |
| ⌘Y | Show history |
| ⌘⇧B | Show bookmarks |
| ⌘⇧. | Pause / resume agents |
| ⌘⌥A | Cycle agent access on this site |
| ⌘⇧R | Reading mode |
| ⌘⇧P | Picture in Picture |
| ⌘⇧J | Show downloads |
| ⌘⇧N | New private tab |
| ⌘P | Print page |
| ⌘⌥B | Show / hide bottom bar |
| ⌘⌥[ | Fold all tab groups |
| ⌘⌥] | Unfold all tab groups |
| ⌘⇧E | Switch space… |
<!-- keymap:end -->

Behaviour that is not a single action:

- **⌘K palette:** each command shows its current custom shortcut; ←/→/Home/End
  move, ⇧ extends, ⌥ moves by word, ⌘A/⌘C/⌘X select/copy/cut, and a click
  places the caret. ⌘L opens it prefilled with the current URL. Typing a URL
  prefix shows the rest of the first matching tab/bookmark/history URL in
  muted text; → at the end accepts it.
- **Find:** ↩ goes to the next hit, ⇧↩ to the previous, ⌘G/⇧⌘G step
  outside the field, `Aa` toggles case sensitivity, Esc closes.
- **Sidebar:** ⌘⌥I opens / closes it; × collapses it, and the Agent / Inspector
  tabs switch panes.
- **Paused agents:** ⇧⌘. pauses or resumes every agent tool call and shows a
  tinted banner with Resume. A CAPTCHA/2FA challenge on the active page shows a
  separate "Agent is waiting for you" banner with Continue.
- **Login offers:** a banner above the status bar, outside the page pane so
  the native web view never covers it, lists one button per saved account and
  a "Save password?" offer (Save/Replace / Not now). The list and Delete only
  cover logins Relay itself wrote. See [login-autofill.md](./login-autofill.md).
- **Handoff:** Settings → Privacy can advertise ordinary http(s) tabs through
  `NSUserActivity` (off by default); private tabs and app windows never
  advertise. A Relay receiver on another device is not implemented.
- **Folder sync:** Settings → General can point at a Dropbox/iCloud folder;
  launch merges the folder's profile file (local settings win) and the app
  rewrites it immediately and every minute. **Sync now** merges on demand and
  **iCloud** picks `iCloud Drive/Relay` in one click.
- **Backup/restore:** Settings → General writes a private JSON export via a
  save panel and merges a chosen backup (local settings win; extensions and
  session tabs are not imported). `relay import FILE` does the same.
- **Profiles:** `relay --profile NAME` (or Settings → General → Profile →
  Switch) gives a separate `RELAY_HOME` with its own tabs, settings, history,
  logins (a per-profile Keychain domain), extensions, and MCP socket, plus a
  stable WebKit website-data store UUID. Switching relaunches Relay; Default uses the base home.
- **Spaces:** named tab sets with their own pins and session entries. The
  current space is a small pill at the top of the strip; ⌘⇧E opens the switcher
  (type a new name, pick an existing space, or rename the current one—merging
  if the target exists), new tabs join the current space, and a two-step Delete
  removes the current space's tabs. The choice persists in `active_space`.
- **Tab groups:** a group heading is a 24 pt row with a chevron, colour dot,
  name, and count. Collapsed groups show only the heading; names are capped at
  40 characters and persist in `collapsed_groups`; a unit test covers the
  stored list parser. Group names survive session restore.
- **Tab bar:** `×` closes (hover/active), a middle click closes, `♪` marks
  playing audio, `pin` a pinned tab, and `zz` a sleeping one. Pinned tabs sit
  at the top, never auto-sleep, and ⌘W sleeps them instead of closing.
- **Reopen:** ⌘⇧T reopens the newest closed http(s) tab; the stack is capped
  at 50 so a long session cannot grow it without bound.
- **Esc** closes the palette, Settings, or link hints.
- **Settings → Agent shell changes:** off by default; a global switch or
  per-client grant under Connected agents or the Agent sidebar lets an agent
  plan/apply theme, accent, start sections, allowlisted settings, validated
  shortcuts, the sidebar, and one capped plain-text start-page note; revoke
  applies on the next tool call; the shell applies changes live.
- **Settings → Keyboard shortcuts:** click Change, press a new shortcut;
  conflicts, bare keys, and ⌘Q are refused; Reset all restores defaults;
  changes apply without restart.
- **Zoom** (`⌘=`, `⌘-`, `⌘0`) is remembered per site.

On Linux development builds, ⌘ is Ctrl (GPUI's `secondary` modifier).

## Focus

When a page has focus, WebKit receives the keystrokes. Relay's menu bar
receives its key equivalents first (AppKit consults the main menu before the
responder chain), which is why the global shortcuts work without stealing
plain typing from the page. A unit test fails if a bound action has no menu
item, because such a shortcut would only work while the GPUI view, not the
page, had focus. The palette and link hints move focus to the shell
while they are open (`Engine::focus_shell`) and hand it back to
the page when they close. The bridge forwards `f` as a `hotkey` message, so
hints start without leaving the page. As in Vimium, a bare `f` outside text
fields is taken by Relay on every site, so site shortcuts bound to `f` (such
as YouTube full screen) need ⌘J for hints instead. A per-site opt-out is not
built yet.

## States

- Empty window: the new-tab page with the welcome card when the profile has no
  history, bookmarks, or apps; otherwise the configured Recent / Frequent /
  Apps / Bookmarks sections.
- Loading: a thin accent progress line under the toolbar.
- Load failure: an in-pane error with Retry.
- Asleep: the snapshot, dimmed.
- Inspector with no DOM yet: "Waiting for page…".
- Closed tab: the `×` appears on hover and on the active row; closing a pinned
  tab sleeps it instead, and a middle click does the same.
- Folded strip: a 14 pt side rail or 10 pt top rail with a chevron; clicking
  it restores the strip.
- Web app install: the palette reports "Reading the web app manifest…", then
  "Installed … in ~/Applications", or a specific reason (no manifest,
  cross-origin start, disk error). A missing or non-PNG icon is not an error:
  the app uses Relay's fallback icon. Removal says "Click again to
  remove …" before deleting anything.
- App window: no tab strip or status bar with one page; a popup link
  navigates that page rather than revealing hidden chrome.
