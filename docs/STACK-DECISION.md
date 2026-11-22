# Stack decision record

<!-- index: operate | Framework/toolchain decision and rationale | Choosing a project stack or replacing an assumed tool. -->

Status: accepted
Project: relay
Owner: TomSzenessy
Date: 2026-11-22

## Decision

The owner's brief chose this stack. Versions were checked on crates.io on
2026-11-22 and are pinned in `Cargo.lock`:

- Rust 2024 edition, stable toolchain (1.100 at decision time); 5-crate Cargo
  workspace (`docs/relay.md`).
- UI: `gpui` 0.2.2 (Zed's GPU UI framework).
- Engine: system WebKit through `objc2` 0.6, `objc2-web-kit` / `objc2-app-kit` /
  `objc2-foundation` 0.3.2 (`WKWebView`, `WKWebExtension`,
  `WKContentRuleListStore`, `WKHTTPCookieStore`).
- Agents: `rmcp` 3.11 (official Rust MCP SDK) on `tokio` 1.x, served on a Unix
  socket and bridged to stdio by `relay mcp`.
- Storage: `rusqlite` 0.43 (system SQLite on macOS, bundled elsewhere);
  passwords in the macOS Keychain through `security-framework` 3.7.
- Extensions: `zip` 9 plus `ureq` 3 (native TLS) for the Chrome Web Store
  `.crx` installer.
- Bridge tests: Node 22 `node:test` + Playwright 1.56 WebKit.

## Why this, and what else was considered

- **Tauri or wry**: a webview inside a web UI. Lost because the chrome, palette,
  and Inspector would be HTML, which works against the 120fps native goal, and
  wry hides `WKWebExtension` and rule-list access.
- **Electron or CEF**: bundles Chromium, which breaks the ~15 MB / ~50 MB
  constraint.
- **SwiftUI + WKWebView**: native, but the brief chose Rust so one binary can
  hold the MCP server, the bridge state, and the UI with no FFI layer between
  them.
- **Screenshot-based agent control (BrowserMCP, Playwright MCP)**: agents get
  structured DOM, console, and network state from the bridge instead.

## Verification and rollback

- `cargo test --workspace --locked` and
  `cargo check --workspace --locked --exclude relay-ui --target aarch64-apple-darwin` (CI, on the
  self-hosted linux/arm64 runner); `npm test` in `js/`.
- The running app must be judged on a Mac (`cargo run -p relay-ui --release`);
  Linux can only type-check the AppKit/WebKit code.
- Rollback: each crate sits behind a narrow seam (`Browser` trait,
  `SharedHub`, `PageMessage`). Replacing GPUI or rmcp touches one crate.
