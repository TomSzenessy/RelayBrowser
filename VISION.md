# Project vision

<!-- index: operate | Accepted product direction, constraints, and success evidence | Starting a project or making a high-impact scope decision. -->

Status: accepted
Project: relay
Owner: TomSzenessy
Date: 2026-11-22

Source: the owner's written brief "Project Codename: Relay" (2026-11-22
session). Every decision below is the owner's own, not an agent's pick.

## Outcome

A 120fps, keyboard-first macOS browser written in Rust (GPUI) on native WebKit.
One lightweight injected bridge powers both a native DevTools Inspector and a
built-in MCP server, so coding agents (Claude Code, OpenCode, Codex, Zed) can
read and drive the user's live tabs without screenshots. Success metric: an
agent answers "why is this page failing?" from `get_console_logs` and
`get_network_requests` in under 1 ms of state access, with no screenshot.

## Users and context

Developers on macOS who work alongside coding agents every day. Today they use
Chrome or Arc for browsing, Chrome DevTools for inspection, and screenshot-based
or Playwright-based browser MCP servers for agents.

## Platforms and constraints

- macOS only for v1 (WKWebView, WKWebExtension, Keychain, Metal/GPUI); no
  cross-platform abstraction.
- Never bundle a browser engine: binary about 15 MB or less, idle RAM about
  50 MB.
- Everything is local: SQLite for history, bookmarks and settings; Keychain for
  passwords; no accounts, no telemetry.
- 80/20: build Elements/Network/Console/Cookies natively; leave heap profiling
  and breakpoints to WebKit's own Web Inspector, one shortcut away.

## Non-goals

- No custom cloud sync backend (use iCloud Drive or a folder/Git repo).
- No built-in LLM chat UI or subscriptions; Relay is the browser + MCP surface
  agents plug into.
- No Windows or Linux build at v1. Linux builds exist only to develop and test
  the portable crates.

## Success evidence

- Automated: `cargo test --workspace` (bridge state, MCP round-trip with a real
  MCP client, importer, CRX, rules, sleep); `npm test` in `js/` (bridge.js in
  WebKit, every message parsed by Rust); macOS type-check in CI.
- Observed on a Mac: the window opens, pages render in WKWebView, ⌘K/⌘T/⌘W
  work, the Inspector shows live DOM/Network/Console, and Claude Code connected
  through `relay mcp` can navigate, click, and read console and network state.
- Roadmap phases 1–4 in the brief are the feature list (`docs/product/features.csv`).
