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
| `relay-ui` | The `relay` binary. `app.rs`: the GPUI shell (tab bar with agent group, panes and split, ⌘K palette, Inspector, link hints, status bar, a 16 ms pump draining MCP requests, WebKit events and hub changes, history recording, tab sleeping). `web.rs`: `WebHost` over the `Engine` (a no-op off-Mac). `palette.rs`, `inspector.rs`: tested view logic. `theme.rs`: tokens from [`design.md`](./design.md). `main.rs`: the CLI (`mcp`, `import-chrome`, `install-extension`, `compile-rules`) | GPUI everywhere; WebKit only on macOS. Linux shows a placeholder pane |

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
- The MCP socket is `relay_home()/mcp.sock`, mode 0600 inside a 0700 directory,
  so only the user's own processes connect.

## Tab sleeping

`SleepPolicy::due` picks background tabs idle for 15 minutes that are not
active, visible in a split, pinned, or playing audio (from the bridge's `media`
message). It always keeps at least one tab awake. `Engine::sleep` snapshots the
viewport to JPEG, records URL and scroll, then destroys the view. `Engine::wake`
recreates it, and `after_load` restores scroll once the bridge says `hello`.

## Checking the shell off-Mac

`crates/relay-ui/review/keyboard_e2e.sh` runs the real `relay` binary under
Xvfb, drives it with `xdotool`, and reads the browser's own state back over
MCP (`list_tabs`). It covers ⌘K/⌘T palette navigation, URL-vs-search, ⌘W,
⌘⇧T, and Esc. It needs `Xvfb`, `xdotool` and `python3`, so it is a manual
check rather than a declared verification command. Screenshots are not
possible on headless VMs without a working Vulkan display: under Xvfb and
Xorg-dummy, llvmpipe presents black frames even for `vkcube`. So visual
review happens on a Mac.

## Build notes

- Minimum macOS: 13.3 (`WKWebView.isInspectable`). Extensions need 15.4
  (`WKWebExtension`); on older systems Relay runs without them.

- `cargo test --workspace` runs everywhere. On Linux,
  `cargo check --workspace --exclude relay-ui --target aarch64-apple-darwin`
  type-checks the macOS code in `relay-webkit` and `relay-extensions`.
  `relay-ui` cannot be checked that way: gpui's macOS build compiles C and
  Metal code (`ring`, shaders), which needs the Apple SDK. Its macOS-only
  code (`web.rs`) is first compiled on a Mac.
- On macOS, SQLite links the system `libsqlite3`; elsewhere it is bundled.
  `ureq` uses native TLS (Security.framework on macOS), so no C
  cross-compilation is needed.
- Linux builds of the `relay` binary need GPUI's system libraries (xkbcommon,
  wayland, xcb, fontconfig, freetype, vulkan headers). CI on the self-hosted
  runner assumes they are installed.
- `third_party/xattr` is a patched copy of xattr 0.2.3 that fixes a Linux build
  break in gpui's dependency tree ([`../third_party/README.md`](../third_party/README.md)).
