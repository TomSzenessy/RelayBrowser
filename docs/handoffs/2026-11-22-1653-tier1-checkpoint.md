# Handoff — Relay Tier 1 Checkpoint (Post PR #35 & PR #38 Merge)

<!-- index: operate | Handover record scaffold (local HANDOVER.md or committed handoff) | A fresh actor needs a focused continuation record. -->

- **Created (UTC):** `2026-11-22T16:53:00Z`
- **Next actor/session focus:** Finish the Tier 1 GPUI/WebKit UI wiring on `feat/tier1-daily-driver` (issue #39), pass `make ci-local` + critic + macOS CI, merge PR, then proceed through Tiers 2 → 3 → 4.
- **Sharing boundary:** public repository (`github.com/TomSzenessy/RelayBrowser`).

## Objective

Continue the multi-tier roadmap from the competitor analysis (Search, Helium, BrowserOS neo, Orion, de-bloated Brave) while preserving Relay's core constraints (`< 15 MB` binary, `~50 MB` idle RAM, One Bridge, 80/20 native Rust Inspector + MCP server, macOS-first).

## Current state

- **`main` @ `4fff273`:**
  - **Merged PR #38** (`a56e512`, `Fixes #37`): repo-scoped `.agents/environment.json` for repeatable Ubuntu 24.04 cloud VM setup.
  - **Merged PR #35** (`4fff273`, `Fixes #2`, `Fixes #3`): working `macos-15` Apple Silicon CI, real-`WKWebView` smoke test, `Relay-macOS.dmg` (5.7 MB) / `relay` (11 MB) artifacts + automated GitHub Releases (`publish-release`), Rust + Playwright caching, Settings overlay (`⌘,`), `relay config`/`export`/`clear-data`, Inspector filters, WCAG AA tests, `AppIcon.icns`, and privacy docs.
  - **`main` macOS release run [`45045432338`](https://github.com/TomSzenessy/RelayBrowser/actions/runs/45045432338):** in progress at handoff time (first post-merge run that will execute `publish-release` to create the rolling `macos-latest` GitHub Release with `Relay-macOS.dmg`).
- **Active branch:** `feat/tier1-daily-driver` @ `bfb486d` (pushed to `origin/feat/tier1-daily-driver`, clean working tree, tracking issue **#39**).
- **Done & verified on `feat/tier1-daily-driver` (`bfb486d`):**
  - Palette logic modules + unit tests: [`crates/relay-ui/src/bangs.rs`](../../crates/relay-ui/src/bangs.rs) (offline `!bangs` + custom overrides), [`crates/relay-ui/src/calc.rs`](../../crates/relay-ui/src/calc.rs) (safe recursive-descent arithmetic evaluator), [`crates/relay-ui/src/clean_url.rs`](../../crates/relay-ui/src/clean_url.rs) (tracking-param & Google/Facebook redirect-wrapper stripper), wired into [`palette.rs`](../../crates/relay-ui/src/palette.rs), [`main.rs`](../../crates/relay-ui/src/main.rs), and [`app.rs`](../../crates/relay-ui/src/app.rs) (`Item::Bang`, `Item::Calc`, `Command::CopyCleanUrl`, `Command::CloseDuplicateTabs`).
  - Persistence layer: [`crates/relay-extensions/src/store.rs`](../../crates/relay-extensions/src/store.rs) now implements `save_session` / `load_session` / `clear_session` (`SessionTab`, `http(s)`-only, max 200), `site_pref` / `set_site_pref` / `reset_site_pref` / `all_site_prefs` (`SitePrefs { zoom, compat_mode, blocking_off, hidden_selectors }`), and `toggle_bookmark` / `remove_bookmark` / `is_bookmarked`, plus `export_json` coverage (no passwords).
  - Bridge Markdown op: `Op::Markdown` + `Hub::page_markdown` (read-only, allowed on challenge pages, 200 KB cap) in [`crates/relay-bridge/src/protocol.rs`](../../crates/relay-bridge/src/protocol.rs), [`hub.rs`](../../crates/relay-bridge/src/hub.rs), and [`js/bridge.js`](../../js/bridge.js).
  - Docs updated: `docs/relay.md`, `docs/bridge.md`, `docs/privacy.md`, `docs/legal/data-inventory.md`, `docs/product/features.csv`.

## Evidence collected on `bfb486d`

| Check | Result | Command |
|---|---|---|
| Workspace Rust tests | **passed** (all crates: 46 `relay-ui` bin + 5 CLI + 1 MCP proxy + 3 MCP roundtrip + 9 `relay-webkit` + `relay-extensions` + `relay-bridge`, 0 failed) | `cargo test --workspace --locked` |
| Darwin cross-check | **passed** | `bash scripts/mac-check.sh` |
| Playwright WebKit 26.0 bridge suite | **passed** (10/10, incl. 2 new Markdown tests + Rust schema validation) | `cargo build --locked -p relay-bridge --example validate && npm --prefix js test` |
| Repo coherence gate | **passed** | `make check` |

## Remaining work (in order)

1. **Verify `main` release run `45045432338`:** check `gh run view 45045432338` and `gh release list` to confirm `macos-latest` Release published `Relay-macOS.dmg` cleanly (fix `scripts/publish-macos-release.sh` if anything failed).
2. **Finish Tier 1 UI & WebKit wiring on `feat/tier1-daily-driver` (issue #39):**
   - Wire `Store::clear_session()` into `relay clear-data` (`crates/relay-ui/src/main.rs`) + CLI test.
   - Wire lazy session restore on launch/quit (`restore_session` setting, restored tabs start asleep and wake on click).
   - Wire `Command::CopyMarkdown` (`⌘⇧M`) to run `Op::Markdown` via the async bridge op runner and copy to clipboard; add `⌘⇧C` (`CopyCleanUrl`), `⌘⌥P` (Pin/Unpin tab + sleep-on-`⌘W` for pinned tabs), `⌘D` (bookmark toggle), `⌘F` (find-in-page via `WKWebView::findString`), `⌘+`/`⌘-`/`⌘0` (per-host `WKWebView::pageZoom` persisted via `SitePrefs.zoom`), audio speaker indicator in sidebar, per-site blocking/compat mode (`WKContentRuleList` `ignore-previous-rules` + Safari UA), custom bangs from `settings("bangs")`, and conservative CMP cookie-banner rules in `content_rules.rs`.
   - Update `key_bindings_cover_documented_shortcuts`, docs (`README.md`, `docs/relay.md`, `docs/design.md`, `docs/product/features.csv`), run `make ci-local` + critic, open PR (`Fixes #39`), verify green Linux + `macos-15` CI, and merge to `main`.
3. **Follow-up residuals (issue #36):** darken light-theme `warn`/`error` tokens to 4.5:1 AA and add file-backed `clear_history` disk-reclaim test.
4. **Tier 2 (Agent Browser Leadership):** One-click MCP Install Board (Claude Code, Codex, OpenCode, Cursor, Zed), in-window Agent Trust Banner + live Stop pill (no restart), per-domain agent policy, lean SQLite action audit log, and `batch_actions` / `screenshot` / `get_storage` / `set_cookie` / `press_key` / `eval_js` MCP tools.
5. **Tier 3 (Native Rust Inspector):** Network detail drawer (captured request/response bodies + headers + URL filter + Copy as cURL + Replay), Elements click-to-inspect & live `EditStyle`, editable Cookies + `localStorage`/`sessionStorage` Application tab, Console JS REPL + Clear.
6. **Tier 4 (Extension Parity & macOS Polish):** `WKWebExtensionControllerDelegate` (`sidePanel` GPUI pane, `offscreen` hidden `WKWebView`, `identity` `ASWebAuthenticationSession`, Chrome Native Messaging), CRX3 signature verification + in-browser CWS install, Private tabs (`⌘⇧N`), Reading Mode (`⌘⇧R`), Auto PiP (`⌘⇧P`), battery-aware Low Power Mode.

## Exact next action

```bash
git checkout feat/tier1-daily-driver
gh run view 45045432338 --json status,conclusion,jobs && gh release list --limit 3
# Then complete item #2 above in crates/relay-ui/src/{app,main,web}.rs and crates/relay-webkit/src/{mac,content_rules}.rs
```

## Redaction and retention check

No secrets, credentials, tokens, or personal data are present. Safe for the public repository.
