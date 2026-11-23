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
| `dom_patch` | MutationObserver, coalesced per timer tick | `nodes` (re-serialized subtrees by id), `removed` ids |
| `dom_attrs` | attribute mutation on a node the store already knows | `nodes`: `{id, attrs}`; replaces that node's attribute map only |
| `console` | `console.*`, `error`, `unhandledrejection` | `level`, `args` (strings, ≤2 KB each), `source` |
| `network` | fetch/XHR completion; PerformanceObserver resources | method, url, status, duration, bodies ≤4 KB (text/JSON only) |
| `challenge` | a CAPTCHA iframe or one-time-code input appears or disappears | `challenge`: `captcha`, `two_factor`, or `null` |
| `media` | audible audio/video starts or stops | `playing` |
| `markdown` | reply to the `markdown` op: Markdown of the main content | `text` (stored in the Hub; `Hub::page_markdown(tab)`, cleared on load) |
| `hotkey` | `f` pressed with the page focused, outside text fields | `key` (the shell starts link hints) |
| `result` | reply to a command | `id`, `ok`, `value`, `error` |

Limits: 4,000 nodes per snapshot, 160-character text, script/style/title
skipped (head itself is kept as an attrs-only node). Values are never sent for
password inputs, `autocomplete` values starting with `cc-`,
`autocomplete="one-time-code"`, or any input carrying `data-relay-private`.
Those fields still expose id/tag/type/name/label so agents can address them;
ordinary text/search fields keep their values for debugging.

## Rust → page

`window.__relay.exec({id, op, ...})`. The script is built by
`Command::to_script` from JSON only. Ops: `snapshot`, `highlight`,
`unhighlight`, `edit_style`, `scroll_into_view`, `click`, `type`, `query`,
`find_text`, `read_text`, `scroll`, `scroll_to`, `page_info`, `show_hints`
(`hints: [[id, label], …]`, labels only visible elements, returns the count),
`hide_hints`, `markdown`, `web_app`, `favicon`, `press_key`, `storage`, `eval_js`, `set_cookie`, `reader`. Element ids are
the `data-relay-id` numbers, and they go stale on navigation.

`markdown` renders `<article>`, else `<main>`, else `<body>` (skipping script, style, nav, aside, footer, form controls, and hidden elements) as Markdown capped at 200 KB. It is read-only, so agents may call it on challenge pages. The string is both the command result and a `markdown` message stored by the Hub; an MCP tool for it is Tier 2.

`web_app` is the first asynchronous op: it reads `link[rel=manifest]`, fetches
and resolves the manifest, fetches the largest PNG icon (2 MB cap), and posts
the ordinary `result` when it settles. The reply is
`{available, page_url, name, short_name, start_url, scope, display,
theme_color, background_color, manifest_url, icon}` or
`{available: false, reason}`. Rust validates the origin, URL scheme, icon
signature, and sizes again before writing an installed `.app`; the page is
untrusted, so a manifest can never point the installer at another site. The
`favicon` is async too: it tries the page's `<link rel=icon>` /
`apple-touch-icon` URLs, then `/favicon.ico`, with `same-origin` credentials,
a 3 s timeout, a 256 KB cap, and a PNG/JPEG/WebP/GIF whitelist. It replies
`{available: true, data: "data:<mime>;base64,…", mime}` or
`{available: false}`; SVG, ICO, and text are refused. The shell validates the
prefix, mime, and decoded size before caching and painting it.

`press_key` dispatches keydown/keyup to the focused element (or `<body>`),
with `ctrl`/`alt`/`shift`/`meta`; Enter submits a focused form unless the page
prevented the event. It never inserts text.

`storage` reads localStorage/sessionStorage as
`{local:{values,truncated,redacted},session:…,url}`; keys matching
`/pass|token|secret|auth|key|credential/i` are `[redacted]`, values stop at
16 KB/200 keys, and access errors become `truncated: true`.

`reader {on}` builds a fixed overlay from `<article>`, `[role=main]`, `<main>`,
or `<body>`; scripts, styles, forms, nav/aside/footer, inline `on*` handlers,
and copied `data-relay-id`s are removed. `on: false` removes it. The clone is
inert content, not a second live DOM for commands.

`set_cookie {name, value}` writes one cookie on the tab's own origin with
`Path=/; SameSite=Lax` (and `Secure` on https); it cannot set domain-wide or
HttpOnly cookies, and refuses bad names, newlines, and values over 4 KB.

`eval_js` evaluates one expression with `new Function` in the page and returns
its JSON value (functions/cycles stringified; 100 KB result cap). The shell
only sends it when the user enabled agent JavaScript and the site policy is
Allow.

`exec` reply is a placeholder (`value: null`) for an async op; wait for the
`result` message.

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

The snapshot root is `document.documentElement`, so `html`, `head`, and `body`
have ids and their attribute changes can use `dom_attrs`; `head`'s
title/meta/link children are skipped. Attribute-only changes never resend a
subtree. A patch whose serialization would exceed the 4,000-node budget falls
back to one full snapshot instead of posting a partial patch.

Rust treats the patch as untrusted input. `DomStore::replace_all` and `patch`
reject a message that repeats an id within one tree (or would close a cycle
through the existing store) and leave the previous tree untouched.
`interactive_ids`, `tree`, and the outline walk each node once with a step
budget, so even a hand-corrupted store cannot hang the UI thread or recurse
without bound.

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
patches, commands, async `web_app` manifest/icon and `favicon` reads and their failure
reasons, 2FA detection, link hints and hotkey forwarding, and bridge
immutability, and pipes every message through `relay-bridge`'s
`examples/validate.rs`.
