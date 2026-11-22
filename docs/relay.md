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
| `relay-webkit` | Content-blocker compiler (EasyList subset → WebKit JSON), tab-sleep policy; `mac.rs`: `Engine` creating one `WKWebView` per awake tab with the bridge injected, rule lists, cookies, snapshots, the Web Inspector fallback | policy portable, engine macOS |
| `relay-extensions` | Chrome importer (Bookmarks JSON, History SQLite, extension ids, password CSV), CRX3 download and unpack, manifest compatibility report, SQLite profile `Store`; `mac.rs`: WKWebExtension loading, Keychain import | importer portable, loaders macOS |
| `relay-mcp` | `RelayMcp` (rmcp 3.x tool router), user-only Unix socket server, `relay mcp` stdio proxy | portable (Unix) |
| `relay-ui` | The `relay` binary. `app.rs`: the GPUI shell (tab bar with agent group, panes and split, ⌘K palette, ⌘, Settings overlay, Inspector with console/network filters, link hints, status bar, a 16 ms pump draining MCP requests, WebKit events and hub changes, history recording, tab sleeping). `web.rs`: `WebHost` over the `Engine` (a no-op off-Mac). `palette.rs`, `inspector.rs`: tested view logic. `theme.rs`: tokens from [`design.md`](./design.md) with WCAG AA contrast tests. `main.rs`: the CLI (`mcp`, `import-chrome`, `import-passwords`, `install-extension`, `compile-rules`, `config`, `export`, `clear-data`, `doctor`) | GPUI everywhere; WebKit only on macOS. Linux shows a placeholder pane |

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

## Shell loop

The browser starts the MCP socket server on a tokio thread, then opens one
GPUI window. Every 16 ms `RelayApp::pump`:

1. applies `ChannelBrowser` requests to WebKit;
2. handles WebKit events (`target=_blank` opens a tab after its opener;
   snapshot ready);
3. re-renders on a hub tick, records history visits, finishes wake-up scroll
   restores, and starts link hints on an `f` hotkey;
4. checks idle tabs for sleep every 30 s.

Panes report their bounds from a `canvas` each frame, and `WebHost::layout`
moves the native `WKWebView`s to match. Native views draw above GPUI, so web
views are hidden while the palette is open. Link-hint labels are drawn inside
the page, and their status line goes in the status bar.

## Agent safety (attached mode)

- Agent tabs (`Owner::Agent`) share the user's cookie store and live in a
  tinted group in the tab bar.
- When the bridge reports a CAPTCHA or 2FA prompt (`challenge`) on any tab,
  agent mutations there (click, type, edit_style) fail with "waiting for a
  human"; reads keep working, and the user can still click through
  (`Actor::Agent` vs `Actor::User` in `SharedHub::begin`).
- Agent commands count as activity, so tabs an agent is using do not sleep.
  If a tab is already asleep, the next MCP call wakes it and waits for the
  reload.
- Agent tabs are kept out of the user's history.
- Pages can only open `http(s)` URLs in new tabs.
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

## Palette smart input

[`bangs.rs`](../crates/relay-ui/src/bangs.rs), [`calc.rs`](../crates/relay-ui/src/calc.rs)
and [`clean_url.rs`](../crates/relay-ui/src/clean_url.rs) are pure string
modules the palette calls before its normal URL/search item:

- **Offline !bangs**: `!gh rust lang` or `rust lang !gh` (also `rs`, `crates`,
  `docs`, `mdn`, `yt`, `so`, `npm`, `w`, `k`, `g`, `ddg`, `maps`) becomes
  `Item::Bang` at the top; an empty query opens the site homepage; unknown
  bangs fall through to the normal search. `Palette::custom_bangs`
  (`trigger`, template with `%s`) overrides built-ins. No network lookup.
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

1. **Portable tests** (`cargo test --workspace`) run anywhere: protocol, hub,
   MCP round-trip with a real client, Chrome import, CRX, content rules, sleep,
   palette, inspector filters, hints, WCAG AA theme contrast, key bindings, and
   CLI subcommands (`doctor`, `config`, `export`, `clear-data`). The stdio
   transport itself is covered by `crates/relay-ui/tests/mcp_proxy.rs`, which
   spawns the real `relay mcp` binary, connects it to a socket-backed hub, and
   performs the initialize / tools list / list_tabs handshake.
2. **The injected bridge runs in Playwright's WebKit 26.0** in Linux CI
   (`js/`, 8 tests): DOM snapshots and patches, fetch/XHR status and bodies,
   console and uncaught errors, click/type/highlight, hints, and 2FA detection.
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
  subset it needs on ubuntu-24.04 (Playwright 1.56 does not support 26.04).
- `third_party/xattr` is a patched copy of xattr 0.2.3 that fixes a Linux build
  break in gpui's dependency tree ([`../third_party/README.md`](../third_party/README.md)).
