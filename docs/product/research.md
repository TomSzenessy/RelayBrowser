# Research

<!-- index: extend | Comparable products, prior art, and Relay's angle | Scoping a feature or judging what to borrow. -->

These sources come from the owner's brief (2026-11-22) plus the primary docs
for the APIs Relay uses. The brief named the three GitHub projects; the agent
has not reviewed their code in depth, so treat what to borrow from them as the
owner's intent.

## Comparable products and prior art

- **Zed / GPUI** — https://github.com/zed-industries/zed — the command palette,
  fuzzy finder, and split panes are the patterns Relay's shell follows
  (keyboard-first, everything reachable from ⌘K).
- **BrowserMCP** — https://github.com/BrowserMCP/mcp — the tool vocabulary
  agents already know (navigate, click, type, snapshot, console logs). Relay
  keeps those names (`navigate`, `click_element`, `read_dom`,
  `get_console_logs`, `get_network_requests`) but answers from bridge state
  instead of screenshots.
- **driceroland/Search** — https://github.com/driceroland/Search — named in the
  brief for its WKWebExtension setup, Chrome Web Store downloader, and Chrome
  API shims.
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
