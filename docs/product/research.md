# Research

<!-- index: extend | Comparable products, prior art, and Relay's angle | Scoping a feature or judging what to borrow. -->

These sources come from the owner's brief (2026-11-22) plus the primary docs
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
- **driceroland/Search** — https://github.com/driceroland/Search — a WebKit
  browser with the same constraints (system engine, small binary, no toolbar).
  Borrowed: tabs down the left **or across the top** (`⇧⌘S`), fold the strip
  away (`⌘S`) leaving a clickable rail, `×` on the row under the pointer, a
  native menu bar so shortcuts survive a focused web view, and the one-field
  keyboard path. Not copied: its SwiftUI implementation, spaces, sync, AI
  pages, and updater. Also studied: Helium, Orion, BrowserOS neo, and a
  de-bloated Brave for the daily-driver feature expectations.
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
