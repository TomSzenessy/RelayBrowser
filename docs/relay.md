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
| `relay-ui` | The `relay` binary: GPUI window, ⌘K palette, tab bar, split panes, Inspector | GPUI on macOS; on Linux it runs with a placeholder instead of a web view |

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

## Agent safety (attached mode)

- Agent tabs (`Owner::Agent`) share the user's cookie store and live in a
  tinted group in the tab bar.
- When the bridge reports a CAPTCHA or 2FA prompt (`challenge`), mutating ops
  (click, type, edit_style) fail in agent tabs with "waiting for a human";
  reads keep working.
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

## Build notes

- `cargo test --workspace` runs everywhere. On Linux,
  `cargo check --target aarch64-apple-darwin` type-checks the macOS code;
  only a Mac can link and run it.
- On macOS, SQLite links the system `libsqlite3`; elsewhere it is bundled.
  `ureq` uses native TLS (Security.framework on macOS), so no C
  cross-compilation is needed.
- `third_party/xattr` is a patched copy of xattr 0.2.3 that fixes a Linux build
  break in gpui's dependency tree ([`../third_party/README.md`](../third_party/README.md)).
