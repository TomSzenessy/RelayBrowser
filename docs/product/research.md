# Research

<!-- index: extend | Comparable products, prior art, and Relay's angle | Scoping a feature or judging what to borrow. -->
<!-- covers: docs/product/features.csv -->

The current feature matrix is [`features.csv`](./features.csv); the
palette row notes live custom shortcuts; shell-write row covers plan/apply for theme, allowlisted settings, shortcuts, sidebar, a start-page note, hashed audit, and action-id/duplicate-key validation; the
Downloads row covers cancel/retry and native byte progress. These sources
come from the owner's brief (2026-11-22) plus the primary docs
for the APIs Relay uses. The owner named the three GitHub projects and asked
for Search's daily-driver feel in particular. What Relay borrowed from Search
is listed below; the code was read as MIT-licensed prior art, not copied, and
the Rust implementation is its own (`tabstrip.rs`, `webapp.rs`, `app_menus`).

## Comparable products and prior art

- **Zed / GPUI** — https://github.com/zed-industries/zed — the command palette,
  fuzzy finder, and split panes are the patterns Relay's shell follows
  (keyboard-first, everything reachable from ⌘K).
- **BrowserMCP** — https://github.com/BrowserMCP/mcp — the tool vocabulary
  agents already know (navigate, click, type, snapshot, console logs). Relay
  keeps those names (`navigate`, `click_element`, `read_dom`,
  `get_console_logs`, `get_network_requests`) but answers from bridge state
  instead of screenshots.
- **driceroland/Search** (MIT; cloned locally for study, not committed, at
  `8aab55db`, 2026-11-22, 198 Swift files) — a WebKit browser with the same
  constraints (system engine, small binary, no toolbar). Borrowed: tabs down
  the left **or across the top** (`⇧⌘S`), fold the strip away (`⌘S`) leaving a
  clickable rail, `×` on the row under the pointer, a native menu bar so
  shortcuts survive a focused web view, and the one-field keyboard path.
  Studied for the next layer: `Shortcuts.swift` (user key overrides, reserved
  and browser-only conflicts, extension commands, live menu rebuild),
  `HomeGrid.swift` (a blank tab's bookmark-folder grid), `AgentBanner.swift` /
  `AgentControl.swift` (agent asks before connecting, status + Stop, and it
  ignores synthetic clicks from Accessibility), and `Side.swift` (position,
  fold, icon-only rail, drag resize). Not copied: its SwiftUI implementation,
  spaces, sync, AI pages, and updater. License and commit recorded so nothing
  is mistaken for Relay code.
- **h4ckf0r0day/obscura** (Apache-2.0; cloned locally for study, not
  committed, at `fef52749`; Rust headless browser for agents)
  — not an engine to embed; a reference for the agent surface. Borrowed ideas:
  32 compact MCP tools with descriptions that teach the safe path, stable
  element refs (`e3`) until navigation, a default output character budget with
  a per-call override, SSRF denial with an *operator hint* that does not change
  policy, and a single dispatch table. Relay's shell tools will follow those
  shapes but plan/apply changes instead of mutating silently.
- **searchbrowser.app** (marketing site; light/dark screenshots captured in
  the local reference folder, not committed) — the visual direction to match: a narrow typographic measure,
  inline bold proof points, one obvious primary action, restrained
  black/white/grey, and a browser mockup whose only chrome is a tab column.
  No copy or assets are reused.
- **Model Context Protocol Rust SDK (rmcp)** — https://github.com/modelcontextprotocol/rust-sdk
  — the server and client used for `relay-mcp`.

## Platform references

- WKWebExtension (macOS 15.4+) — https://developer.apple.com/documentation/webkit/wkwebextension
- Content blockers and WKContentRuleListStore — https://developer.apple.com/documentation/webkit/wkcontentruleliststore
- WebKit content blocker rule format — https://webkit.org/blog/3476/content-blockers-first-look/

## Our angle

One bridge, two frontends: the native Inspector and the MCP server read the
same Rust structs. An agent learns why a checkout fails from console and
network state, with no screenshots and no second automation stack. Relay
also uses the system WebKit, so it stays small and Apple handles engine
security updates.

## Search parity snapshot (2026-11-22)

"At least as good as Search" is the target, not the current claim. This is the
honest gap list; each `no` is a candidate for the next slices, not a promise.

| Area | Search | Relay now | Next |
|---|---|---|---|
| Tabs: side/top, fold, close, pin | yes | yes (`↔`/`⌘S`/`×`/pin) | visual review on a Mac |
| Split / find / ad block | yes | yes | — |
| New tab: home grid, recents, apps | home bookmark grid | start page with recent/frequent/apps/bookmarks | shortcut/sidebar slices |
| Custom hotkeys | full Settings › Shortcuts | fixed key map + native menu | **shortcut editor** |
| Collapsible side column | yes, drag resize | tab strip only; Inspector pane | **agent/inspector sidebar** |
| History / bookmarks / downloads panels | three panels | palette + bookmark toggle | panels |
| Reading mode / PiP | yes | no | later |
| Passwords | save, autofill, Keychain | import only | later |
| Themes | light/dark/mural | system light/dark | theme tokens |
| Web apps | no (Search is the app) | **install-site-as-app** | icons, uninstall UI |
| Agents / MCP | no page MCP | page-control MCP | **shell-control MCP** |
| Spaces / sync / updater | yes | no | out of scope for now |

## The next platform layer: the same MCP shapes the browser

The owner's direction: the MCP that navigates pages should also be able to
reshape the shell — hotkeys, landing page, sidebar, theme, and declarative
widgets. That is a second *plane* of tools, kept separate from page control:

- **Read tools** report settings, key bindings, new-tab sections, sidebar
  state, and installed apps without touching a page.
- **Plan/apply** for writes: `plan_change` returns a human diff and a
  one-time id; `apply_change` applies it. A denied or expired plan changes
  nothing. Every apply is audited (client, tool, before/after hash, result).
- **Allowlists, not code**: settings keys, action ids, keystroke grammar,
  ordered new-tab sections, and theme tokens with a WCAG AA check. No
  arbitrary JS in the browser process; a later HTML-widget tier runs sandboxed
  with no bridge and no network by default.
- **Trust**: shell writes are off until the user grants the connected agent;
  `mcp`, `shell_control`, clear-data, and reserved system keys cannot be
  changed by an agent. The agent sidebar shows who is connected and what it
  changed, with Stop.
- **Why not copy Search or Obscura**: Search is the UX/feature reference and
  Obscura is the agent-surface reference; Relay's value is one bridge feeding
  both the Inspector and the MCP, now extended from *page* state to *shell*
  state.
