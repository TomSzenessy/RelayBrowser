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
- Storage: `rusqlite` 0.43 with bundled SQLite by default (`bundled-sqlite` feature, disabled only for Linux's `--target aarch64-apple-darwin` cross-check because macOS 15's system SQLite 3.43.2 lacks `sqlite3_set_clientdata`);
  passwords in the macOS Keychain through `security-framework` 3.7.
- Extensions: `zip` 9 plus `ureq` 3 (native TLS) for the Chrome Web Store
  `.crx` installer.
- Bridge tests: Node 22 `node:test` + Playwright 1.65 WebKit.

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

- `cargo test --workspace --locked`, `bash scripts/mac-check.sh`, and `npm test`
  in `js/` (GitHub-hosted `ubuntu-24.04`); full macOS build, tests, real-WebKit
  smoke, and `Relay.app` packaging on `macos-15` (`.github/workflows/macos.yml`).
- The running app's visual polish must still be judged on a Mac
  (`cargo run -p relay-ui --release` or `bash scripts/verify-macos.sh`).
- Rollback: each crate sits behind a narrow seam (`Browser` trait,
  `SharedHub`, `PageMessage`). Replacing GPUI or rmcp touches one crate.
