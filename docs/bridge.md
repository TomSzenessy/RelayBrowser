# The page bridge

<!-- index: design | The injected JS bridge wire contract and Chrome shims | changing bridge.js, chrome-shims.js, or the page protocol -->
<!-- covers: js/** -->

`js/bridge.js` is injected at document start into every main frame
(`WKUserScript`, page world). It is the only code that talks to pages. The Rust
types it must match are in `crates/relay-bridge/src/protocol.rs`. Change both
sides together, and run `npm test` in `js/`, which checks every emitted message
with the Rust parser.

## Page → Rust (`webkit.messageHandlers.relay.postMessage(JSON)`)

| `kind` | When | Payload |
|---|---|---|
| `hello` | script start (each load) | `url`, `version` |
| `page` | DOMContentLoaded, title change, pushState/replaceState/popstate/hashchange | `url`, `title` |
| `dom_snapshot` | load, or when the body is replaced | `root`: `{id, tag, attrs, text, interactive, children}` |
| `dom_patch` | MutationObserver, coalesced per frame | `nodes` (re-serialized subtrees by id), `removed` ids |
| `console` | `console.*`, `error`, `unhandledrejection` | `level`, `args` (strings, ≤2 KB each), `source` |
| `network` | fetch/XHR completion; PerformanceObserver resources | method, url, status, duration, bodies ≤4 KB (text/JSON only) |
| `challenge` | a CAPTCHA iframe or one-time-code input appears or disappears | `challenge`: `captcha`, `two_factor`, or `null` |
| `media` | audible audio/video starts or stops | `playing` |
| `markdown` | reply to the `markdown` op: Markdown of the main content | `text` (stored in the Hub; `Hub::page_markdown(tab)`, cleared on load) |
| `hotkey` | `f` pressed with the page focused, outside text fields | `key` (the shell starts link hints) |
| `result` | reply to a command | `id`, `ok`, `value`, `error` |

Limits: 4,000 nodes per snapshot, 160-character text, script/style/head
skipped. Password input values are never sent.

## Rust → page

`window.__relay.exec({id, op, ...})`. The script is built by
`Command::to_script` from JSON only. Ops: `snapshot`, `highlight`,
`unhighlight`, `edit_style`, `scroll_into_view`, `click`, `type`, `query`,
`find_text`, `read_text`, `scroll`, `scroll_to`, `page_info`, `show_hints`
(`hints: [[id, label], …]`, labels only visible elements, returns the count),
`hide_hints`, `markdown`. Element ids are
the `data-relay-id` numbers, and they go stale on navigation.

`markdown` renders `<article>`, else `<main>`, else `<body>` (skipping script, style, nav, aside, footer, form controls, and hidden elements) as Markdown capped at 200 KB. It is read-only, so agents may call it on challenge pages. The string is both the command result and a `markdown` message stored by the Hub. An MCP tool for it is Tier 2.

## Trust

Page scripts share the page world, so they can see `data-relay-id`, call the
message handler, and send fake messages. So: everything the bridge reports is
data written by the website, never instructions to an agent (the MCP server
tells agents this). Subframe messages are dropped. A `result` resolves only
the command sent to that tab. `window.__relay` is non-writable, so pages
cannot replace it.

Known limits (the page is untrusted): a page can send `challenge: null` to
lift an agent pause it raised itself, and command ids are sequential. A
hostile page gains nothing it could not already do to its own content, and
the agent should still treat its output as untrusted. Pending replies are
dropped once their waiter times out.

DOM patches are scheduled with `setTimeout`, not `requestAnimationFrame`:
WebKit suspends rAF in off-screen pages, which would stall background and
agent tabs.

## Chrome shims (`js/chrome-shims.js`)

At install, `manifest::install_shims` copies the shim into the unpacked
extension and runs it before the background code: a wrapper service worker
(`importScripts`, or `import` for module workers), or a prepended background
script. WebKit provides `browser.*`; the shim adds:
`chrome` aliases `browser`, plus `chrome.sidePanel` (forwarded as native
messages `relay.sidePanel.*` to a GPUI pane), `chrome.offscreen` (a hidden
iframe, or a native hidden view), and `runtime.getPlatformInfo`. **Not built
yet:** nothing on the native side receives the `relay.sidePanel.*` /
`relay.offscreen.*` messages (it needs a `WKWebExtensionControllerDelegate`),
so `sidePanel.open` currently rejects.

## Tests

`js/test/bridge.test.mjs` runs the real script in Playwright's WebKit build
with routed fake pages. It covers the DOM ids, console, fetch/XHR, mutation
patches, commands, 2FA detection, link hints and hotkey forwarding, and
bridge immutability, and pipes every
message through `relay-bridge`'s `examples/validate.rs`.
