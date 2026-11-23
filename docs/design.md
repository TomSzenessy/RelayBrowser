# Design

<!-- index: design | Visual direction, layout, keyboard map, and UX principles | before any UI work -->

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

- **Tab strip:** a vertical list of title rows with status dots, or the same
  rows in a horizontal strip across the top (`⇧⌘S`). The active tab and the
  row under the pointer show `×` (a middle click closes too). The user's tabs
  come first, then an "Agent" group header with tinted rows for agent-owned
  tabs. Asleep tabs are dimmed. A tab waiting for a human shows ⚠. `⌘S` folds
  the strip to a thin, clickable rail (14 pt at the side, 10 pt at the top)
  that brings it back; the edge and folded state persist in `tabs_position`
  and `tabs_folded`.
- **Palette:** one input with ranked rows (open tabs, history, bookmarks,
  commands). The first row is always "Open <typed URL>" or "Search <query>".
- **Inspector:** a tab strip of four panels over monospace rows. Hovering an
  Elements row highlights the node in the page.
- **Sidebar:** a 380 px right sidebar with **Agent** and **Inspector** tabs and
  a × collapse button. Agent shows MCP state/socket, Stop/Resume, per-site
  access, the copyable setup command, and the untrusted-page reminder;
  Inspector keeps the Elements/Network/Console/Cookies panels. The sidebar
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
  Bookmarks rows have a move prompt for the folder path. Esc or click-outside
  closes and returns focus to the page.
- **New tab:** with no active tab, a quiet page with the app name, one search
  field (opens ⌘K), and two columns of Recent / Frequent / Apps / Bookmarks
  rows. Rows are one line with a muted note; empty sections say "Nothing yet";
  only a genuinely fresh profile shows the welcome card. `start_sections` owns the order and `none` hides the page's sections.
- **Menu bar:** native macOS menus (Relay, File, Edit, View, History,
  Bookmarks, Tab, Window, Help) carry every shortcut. AppKit checks menu key
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
surface. Settings → Appearance changes it live and stores `theme`/`accent`.

Type: system UI font at 13 pt for chrome and 12 pt monospace for the Inspector.
Corner radius is 6 pt and spacing uses a 4 pt grid. Motion is 150 ms, and is
none under Reduce Motion.

## Keyboard map

| Keys | Action |
|---|---|
| ⌘K | palette |
| ⌘L | palette prefilled with the current URL |
| ⌘T / ⌘W / ⌘⇧T | new tab / close tab / reopen closed tab |
| ⌘1–⌘8, ⌘9 (last tab), ⌃Tab / ⌃⇧Tab | switch tab |
| ⌘Q | quit |
| ⌘[ / ⌘] / ⌘R | back / forward / reload |
| ⌘⌥I | toggle the native Inspector |
| ⌘⌥U | WebKit Web Inspector (fallback) |
| ⌘, | toggle Settings (search engine, blocking, sleep timeout, MCP, clear data) |
| ⌘\ | split / unsplit the content area |
| ⌘J, or f with the page focused (not in a text field) | link hints |
| ⌘⇧C | copy the current URL without tracking parameters |
| ⌘⇧M | copy the page's main content as Markdown |
| ⌘D | bookmark the page, or remove its bookmark |
| ⌘F | find in page (↩ next, ⇧↩ previous, Esc closes) |
| ⌥⌘I | open / close the right sidebar; × collapses it, Agent / Inspector tabs switch panes |
| ⇧⌘. | pause / resume every agent tool call; a tinted banner with Resume appears |
| ⌘⌥A | cycle agent access on this site: Allow → Read only → Block |
| Settings → Keyboard shortcuts | click Change, press a new shortcut; conflicts, bare keys, and ⌘Q are refused; Reset all restores defaults; changes apply without restart |
| ⇧⌘R | reading mode: a clean article overlay; second press closes |
| ⌘Y / ⇧⌘B | History / Bookmarks overlay (type to filter, ↑↓, ↩ opens, × deletes, move sets a folder) |
| ⌘= / ⌘- / ⌘0 | zoom in / out / actual size, remembered per site |
| ⌘S | fold / unfold the tab strip |
| ⇧⌘S | move the tab strip left → top → right |
| ⌘X / ⌘C / ⌘V / ⌘A, ⇧⌘Z | the page's cut / copy / paste / select all, undo / redo |
| ⌘M | minimize the window |
| (tab bar) | `×` closes (hover/active), middle click closes, `♪` playing audio, `pin` pinned, `zz` asleep |
| ⌘⌥P | pin / unpin the tab; pinned tabs sit at the top, show `pin`, never auto-sleep, and ⌘W sleeps them instead of closing |
| Esc | close palette / settings / hints |

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
