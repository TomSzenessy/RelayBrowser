# Login fill and save: security design

<!-- index: design | Login offer/fill/save design, Keychain storage, redaction, and open owner decisions | Before implementing password fill/save or reviewing its security. -->
<!-- covers: crates/relay-extensions/src/login.rs crates/relay-extensions/src/mac.rs crates/relay-webkit/src/mac.rs crates/relay-ui/src/web.rs -->

Status: **implemented (native macOS Keychain + isolated `WKContentWorld` path); residual hardening tracked in #186.**
The pure model in
[`crates/relay-extensions/src/login.rs`](../crates/relay-extensions/src/login.rs),
macOS Keychain CRUD in
[`crates/relay-extensions/src/mac.rs`](../crates/relay-extensions/src/mac.rs),
isolated `WKContentWorld("relay.login")` capture/fill in
[`crates/relay-webkit/src/mac.rs`](../crates/relay-webkit/src/mac.rs), and
native offer/save/delete banners in `relay-ui` implement D1–D5 below. Passwords
never enter `js/bridge.js`, `SharedHub`, `relay-mcp`, `profile.sqlite`, or logs.

## Problem and scope

Relay can import a Chrome password CSV into the macOS login Keychain, but it
cannot offer, fill, or save a login. The feature must not turn the browser's
debugging surfaces — the injected bridge, the domain snapshot, the MCP server,
or logs — into a password path.

In scope: explicit fill of an existing Keychain login, an offer to save a
successful login, decline/no-repeat behavior, and the security constraints
below.

Out of scope for the first implementation: password generation, breach
checking, sync, passkeys/WebAuthn (see [`passkeys.md`](./passkeys.md)), Touch
ID-unlocked vaults, iframe login forms, and a `www.`/eTLD+1 alias policy. Each
is an owner decision or a later issue, not an implicit part of #57.

### Owner pass (#189)

- `Settings → Passwords` has independent **Fill saved sign-ins**, **Fill a
  single saved login automatically** (off by default; a click is the explicit
  consent in the default build), and **Offer to save passwords** switches.
  The older `login_offers` key still sets both, so an existing profile keeps
  its choice.
- A form-less SPA is filled through `formIdx = -1`, which now reaches the
  helper unchanged; open shadow roots are searched, and a two-step sign-in's
  account field is filled before the password step.
- A fill reports `LoginFilled { ok }` from the page; the shell no longer
  toasts success before the field is set.
- Password capture is refused in native code when saving is off, and passwords
  are zeroized when a `Password`/`PendingCapture` is dropped. The confirmed
  login crosses the native seam as a named `PendingLogin`, not a tuple.
- A save offer requires a main-frame 2xx response in addition to the same
  origin, so an error page that still renders a form cannot ask to store the
  posted password. The response status is tracked in the native handler, not
  read from the page.
- A submit signal must be a trusted user event (`event.isTrusted`); a page
  calling `requestSubmit()` cannot trigger capture.
- Every provisional navigation rotates a 128-bit nonce and injects it into the
  isolated `relay.login` world only. Focus/submit signals, the fill helper, and
  the capture helper compare against it, so a stale document or a page-world
  caller cannot answer for the current one.
- Keychain failures distinguish **Denied** (user cancel/auth/interaction) from
  **Unavailable**; a denied read is not retried against another path and the
  banner says "Keychain access was denied" rather than a generic failure. Find's
  case mode, PDF export, media keys, camera/microphone prompts (including their
  persisted choices), and the native find callback are unrelated to this path.
- Relay records the logins it writes (host, user, path) in the profile. The
  Settings list, fill offers, and Delete act only on those records, so another
  browser's Keychain items are never offered or destroyed. A second save for
  the same record is labelled **Replace**.
- Clearing cookies/site data, the page's Reading Mode item, and asking to be
  the default browser go through the same native delegate; none touches the
  credential path.

## Assets and trust boundaries

| Asset | Where it may live | Where it must never live |
|---|---|---|
| Password | Page field while typing, native process memory during capture/fill, macOS Keychain | Bridge message, `SharedHub`/DOM snapshot, MCP reply, audit row, `profile.sqlite`, export, logs, crash strings, URLs, test fixtures |
| Username + origin | Keychain metadata, native UI, bounded settings list | Public issue text, telemetry, agent-readable page snapshot if it was not already page content |
| Save/fill decisions | Native memory per form/session | Bridge or MCP |

Trust boundaries:

- The page is untrusted. It can forge DOM events, call `requestSubmit`,
  navigate itself, and read values typed or filled into its own fields. It
  cannot call native Relay code, read another content world, or open the
  Keychain.
- A local same-user process is the accepted MCP trust boundary. It can drive
  the page but must not gain a credential-read tool; `eval_js` sees the page
  world, not the isolated login world or the Keychain.
- The native UI is the only source of trusted user intent for fill and save.

## Decisions

### D1 — Native-only credential path

Passwords move between page fields, native memory, and the Keychain. They do
**not** move through `js/bridge.js`, `window.webkit.messageHandlers.relay`,
`BridgeHandler`, `SharedHub`, `relay-mcp`, the Inspector, the audit log, or
`profile.sqlite`.

The bridge protocol has no fill/save/credential operation. The MCP server gets
no login tool. This is a deliberate one-way boundary: adding a bridge operation
for credentials would mean a page-capable channel carries secrets, so it is
refused even if the payload were redacted for display.

### D2 — Isolated WKWebView content world, arguments not source strings

The implementation uses `WKContentWorld.worldWithName("relay.login")`
(`WKContentWorld` is available through the existing `objc2-web-kit`
dependency):

1. A small helper is installed with `WKUserScript` at
   `WKUserScriptInjectionTime::AtDocumentStart`, main frame only, in the named
   content world. Page script cannot call or enumerate it.
2. Capture and fill are invoked with
   `WKWebView.callAsyncJavaScript:arguments:inFrame:inContentWorld:completionHandler:`
   and an `NSDictionary<NSString, AnyObject>` of arguments. The password is an
   argument, never interpolated into JavaScript source; this avoids both
   script-injection escaping bugs and source strings in crash logs.
3. The live origin read from the WKWebView at fill time plus a native form
   key are arguments to the helper. The helper rejects a mismatched form or
   navigation, so a stale native call cannot fill a new document.
4. Capture returns the username/password in the completion handler directly to
   native code. It does not call `postMessage`, does not touch the bridge
   handler, and does not enqueue a hub patch.

The page still sees a filled value in its own input, just as it sees a typed
value. That is inherent to a web form and is recorded as a residual risk. DOM
snapshots continue to suppress password, `cc-*`, `one-time-code`, and
`data-relay-private` values (existing `isSecret` path in `js/bridge.js`).

### D3 — Exact origin binding

An origin is `scheme + canonical host + non-default port`:

- `https` is required, with one exception: `http` is accepted only for
  `localhost` and loopback literals (`127.0.0.0/8`, `::1`), matching the
  issue's "except localhost" wording. Plain HTTP to any other host is refused
  at parse time.
- Hosts are lowercased and canonicalized. IPv4/IPv6 literals are normalized;
  default ports (443/80) are dropped; an explicit non-default port is kept.
- URLs with userinfo (`user:pass@`), control/whitespace/backslash characters,
  percent-encoded authorities, non-ASCII hosts, empty labels, and trailing
  dots are refused. The origin parser's errors never echo the URL.
- There is **no** `www.`-stripping, eTLD+1, suffix, IP-range, or public-suffix
  alias. `https://example.com` and `https://www.example.com` are different
  origins until the owner explicitly approves an alias rule. This favors
  missed offers over a credential leak.
- The origin is read from native `WKWebView.URL` and compared with the
  origin/form the offer was built for. A page's `location.href` is not trusted.
  Cross-origin navigation after a submit drops the capture.
- The Keychain item's matching fields (server, port, protocol) are reconstructed
  into the same `Origin` before an offer or fill. A path is a ranking hint
  only and never widens the origin.
- Only the main frame is supported. Subframe/iframe login forms are refused in
  the first version; lowering that restriction needs an owner decision and a
  top-frame origin check that cannot be spoofed by a subframe.

### D4 — Explicit user gesture, no silent fill

Fill is offered only after a **native** trusted focus/click in a login field
and is applied only after a second explicit native choice (click or Return).

Trusted intent is established natively, not by `event.isTrusted`:
`isTrusted` is page-observable and `requestSubmit()`/`focus()` edge cases make
it unsuitable as the only boundary. The macOS engine records the timestamp of
the last real key/mouse event delivered while the `WKWebView` is first
responder (local event monitor or a `WKWebView` subclass). A focus, submit,
or capture that does not fall inside a short window after that native event is
not trusted and is ignored. The exact window and event source are an owner
decision with a recommendation of 1.5 s and native key/mouse events only.

Properties:

- A synthetic `focus`/`click`/`submit` from page script cannot create an offer,
  capture a password, or confirm a fill.
- Opening the candidate list is not a fill. The model's
  `FillOffer::confirm` requires a selection plus the same origin and form; there
  is no API that fills the first candidate.
- One fill consumes that trusted gesture. Another fill needs another gesture.
- The native picker shows usernames and the origin. It does not show a password.

### D5 — Save prompt after a successful login

Capture happens in two steps so a failed or cross-origin login never produces a
prompt:

1. **Trusted submit.** After a native-trusted submit/click within a login form,
   the isolated-world helper reads the username and password into native
   memory. The native form key is the tab and the form index the page reported;
   recorded. An untrusted or cross-origin form action is not captured.
2. **Success heuristic.** The prompt is armed only when all of these hold:
   - the main frame committed a new document;
   - the destination origin is exactly the origin where the password was
     entered;
   - the transport is `https` (or loopback HTTP as above);
   - the navigation response was a success (2xx/3xx, not an error page);
   - `didFinishNavigation` completed without a WebKit error; and
   - the resulting document has no visible password field.

If any condition fails, the captured value is dropped immediately and the form
returns to idle. A wrong-password reload that still shows a password field is
therefore not offered. A same-document (`history.pushState`) login is **not**
offered in the first version: it needs a separate, owner-approved timeout and
success signal. This is conservative by design; a missed save prompt is
recoverable, a false one trains users to approve the wrong thing.

The save prompt is a native Relay surface (sheet/sidebar), not a page overlay.
The page cannot draw, move, or click it. It shows the origin, the username, and
a masked password. Accept writes the Keychain; Decline/Dismiss stores nothing
and is terminal for that native form key, so the same form is not offered again
in the session. A new navigation creates a new form key.

If the Keychain already has a Relay login for the same origin + username, the
prompt offers Replace / Keep Existing. Replace updates the existing item in
place; Keep Existing stores nothing. There is no partial write: the old item
stays readable until a successful replacement.

### D6 — Save/decline state machine

The pure model is `FormSave` in `login.rs`. State and effects:

```
Idle ──trusted capture──▶ AwaitingNavigation ──successful same-origin nav──▶ Offer
  ▲                              │ failed/cross-origin/insecure/        │
  └────────discard───────────────┘ password-field-still-present          ├─Accept──▶ Saving ──saved──▶ Saved
                                                                          │             │
                                                                          │             └─failure──▶ Failed
                                                                          └─Decline──▶ Declined
Saving ──duplicate──▶ Duplicate ──KeepExisting──▶ Declined
                         └──Replace──▶ Saving
```

`Saved`, `Declined`, and `Failed` are terminal: later events are ignored and a
second capture for the same form is discarded. The machine owns the captured
secret only between `capture` and `Accept`; `Accept` moves it out to the writer,
and every discard path drops it. `Debug` on the secret, capture, and outcome
prints `<redacted>`.

### D7 — Keychain storage through security-framework

Relay writes standard macOS **internet password** items so WebKit/Safari
AutoFill can also see them:

- `kSecClassInternetPassword`, server = canonical host (no brackets), port =
  effective port (or omitted for the protocol default), protocol = HTTPS (HTTP
  only for loopback), account = username, path = `/` for a new Relay-saved
  item, authentication type = HTML form.
- `security-framework`'s `set_internet_password` uses `SecItemAdd`, then
  `SecItemUpdate` on `errSecDuplicateItem`; it never deletes first. The save
  path must use the same add-or-update shape and must return a bounded failure
  when it cannot update.
- A Keychain write is one item operation. A failed add creates no item; a
  failed update leaves the old item intact. The negative tests query the
  Keychain after an injected failure and assert there is no partial item.
- Relay-saved items carry a Relay marker (recommended: `kSecAttrDescription =
  "Relay"`; `kSecAttrLabel` is user-facing and is part of the owner decision)
  so Settings can list/delete only Relay-created entries. Imported entries keep
  their existing attributes and remain readable.
- An existing same-origin item found during a preflight is never overwritten
  without Replace. If the existing item is an imported one, the prompt says so
  and replacing updates it in place; Keep Existing leaves it untouched.
- Deletion is a Keychain delete of a Relay-tagged item only. `relay
  clear-data` and `relay export` keep their current behavior: no Keychain
  deletion, no passwords in export.

Touch ID/`SecAccessControl` is **not** decided here. Recommendation: require
user presence for fill after the first working slice; imported items will not
have that access control, so the UX must distinguish them.

### D8 — Redaction and logging

- `Password` has no `Serialize`, no `Clone`, and a `Debug` that prints
  `<redacted>`. `CapturedLogin` and `SaveOutcome` use it, so a save effect can
  be formatted without leaking the secret.
- The native save/fill path logs host, form state, and a bounded failure code
  only. No username/password in logs. `OriginError` and `SaveFailure` carry no
  URL or platform error text.
- No password in a URL: userinfo is refused before parsing, so a logged origin
  cannot contain credentials.
- No password in `profile.sqlite`, `relay export`, `relay audit`, MCP replies,
  bridge messages, DOM snapshots, `read_dom`, screenshots, or crash metadata.
  Screenshots can still contain a password the page displays; the user controls
  screenshot capture and agents require an explicit attachment.
- Existing bridge redaction (`input[type=password]`, `cc-*`,
  `one-time-code`, `data-relay-private`) stays in place; the login path adds no
  bridge operation that could bypass it.

### D9 — Synthetic-event refusal

There are three independent refusals, so one bug does not become a credential
leak:

1. **No page-reachable API.** The fill helper lives in an isolated content
   world and is invoked only by native `callAsyncJavaScript`; the bridge/MCP
   have no login operation.
2. **Native trust check.** The native layer passes the form index and checks
   form key; the helper rejects stale or missing values. Rust's
   `FillOffer::open` and `FormSave::capture` refuse `Gesture::Synthetic` and
   `trusted: false`, including a synthetic focus, submit, or `requestSubmit()`.
3. **Prompt isolation.** The save prompt is native; a page overlay cannot
   impersonate or click it. The model's `confirm` also requires a selection and
   the unchanged origin/form.

Tests cover the model's synthetic refusals. A macOS test will dispatch
synthetic `focus`/`submit`/`click` from page script and assert no offer, no
capture, and no Keychain read.

### D10 — Settings and deletion

- One user-facing switch disables offers (and therefore fill) without deleting
  anything. The persisted value is a boolean in the existing `settings` table.
  The default is an owner decision: recommendation is **off during the
  security-review preview**, then **on** once the owner accepts the review,
  because an unannounced autofill prompt is the riskier default.
- Settings lists Relay-tagged entries (origin, username, last used) and can
  delete selected entries. It never shows a password.
- Delete is explicit per item with a confirmation; imported/Safari entries are
  not listed or modified by Relay. Existing imported entries remain readable
  and usable for fill.
- `relay clear-data` does not delete Keychain items. A separate
  `--logins`/Settings action is an owner decision, because deleting credentials
  by profile cleanup is surprising and destructive.

### D11 — Failure and rollback

- Capture or write failure: show a non-secret error, drop the capture, leave
  the form terminal for this form key, and do not retry in the background.
- Keychain denied/unavailable/invalid item: `SaveFailure` distinguishes
  `Denied`/`Unavailable`/`InvalidItem`; no partial item is left.
- Tab close, navigation, Stop, or sleep drops any pending capture and hides
  the prompt.
- Disabling offers hides prompts and stops new offers; a capture already in
  progress is dropped.
- Removing the feature leaves imported Keychain items untouched.

## Target data flow

```
                      trusted native key/mouse
                                │
page form ──focus/submit──▶ WebKit engine ──callAsyncJavaScript──▶ isolated world
                                │  (arguments: form index, user, secret)     │
                                │◀──────── completion handler ─────────────┘
                                │
                     captured login (native memory only)
                                │
                      save prompt (native UI)
                         │                 │
                      Accept            Decline/Dismiss
                         │                 └── drop, terminal
                         ▼
                Keychain (Security.framework)
                         │
                 fill: explicit native choice
                         │
              callAsyncJavaScript(arguments: username, password)
                         │
                    isolated world sets the field
```

Nothing in this diagram crosses `window.webkit.messageHandlers.relay`,
`SharedHub`, the Inspector, or `relay-mcp`.

## Implemented now (pure model)

[`crates/relay-extensions/src/login.rs`](../crates/relay-extensions/src/login.rs)
is platform-independent, I/O-free, and tested on Linux CI:

- `Origin` parsing/normalization and loopback-only HTTP; no URL in errors.
- `SavedLoginMeta` and `match_candidates` (exact origin, path ranking,
  recency, username dedup, displayable-username filter, cap).
- `Password`, `CapturedLogin`, and redacted `Debug`.
- `FormKey`, `FormSave`, `SaveState`, `SaveEvent`, `SaveEffect`: trusted
  capture, success-gated offer, decline/no-repeat, duplicate Replace/Keep,
  terminal failure, and secret drop on every refusal.
- `FillOffer`, `Gesture`, `LoginField`, `FillTarget`, `FillRefusal`: trusted
  offer, explicit selection, stale-origin/form refusal, synthetic refusal.
- Tests for origin normalization, origin errors not echoing a URL, candidate
  ranking/dedup/caps, save/decline/duplicate/failure transitions, redaction,
  and synthetic/auto-fill refusal.

It does not read or write the Keychain, touch WebKit, or render UI. A future
native slice must depend on this model rather than re-implementing the
decisions in the engine or UI. It lives in `relay-extensions` because that crate
already owns Keychain import; if wiring the WebKit engine to it creates a cycle
or an awkward dependency, move it to a dedicated `relay-credentials` crate
before implementation rather than copying it.

## Test plan and evidence

| Layer | Test | Gate |
|---|---|---|
| Pure model | `cargo test -p relay-extensions login::` (implemented) | Linux CI |
| macOS Keychain | `#[cfg(target_os = "macos")]` tests behind `RELAY_KEYCHAIN_TEST=1`: add/read/update/delete a unique `relay-test-<uuid>.invalid` item, duplicate replace, and a forced failure leaving no item; skip when the test keychain is unavailable | macOS job where the runner permits; never the real login keychain |
| WebKit isolated world | A macOS example/test that installs the helper, dispatches synthetic focus/submit/click, and asserts no native callback/offer; a real trusted click offers; a confirmed fill sets the field and the bridge snapshot still omits the value | macOS owner run; off-Mac CI cannot exercise `WKContentWorld` |
| Bridge redaction | Existing WebKit Playwright test asserting password/card/OTP values never leave the page; add a `type` reply assertion if the path grows | `npm test` |
| MCP | Assert no login/fill tool exists and `read_dom`/`get_storage` still redact; no credential appears in an audit row | MCP round-trip |
| Owner evidence | Log in on a test page, save, reload, explicit fill; decline and confirm no repeat; attempt a synthetic fill; Keychain Access screenshot (no password) | Issue #57 acceptance |

CI where the runner permits the Keychain is not proof of the real login
Keychain UX; the owner login and screenshots remain the acceptance artifact.

## Owner decisions required before implementation

| # | Decision | Recommendation | Why it is owner-only |
|---|---|---|---|
| 1 | Offers/fill default on or off | Off for preview, then on | Changes what every existing profile sees after upgrade |
| 2 | Touch ID / `SecAccessControl` for fill | Require user presence after the first slice | Affects Keychain item shape and imported-item UX |
| 3 | Relay marker attribute | `kSecAttrDescription = "Relay"` | User-visible Keychain metadata and deletion scope |
| 4 | `www.`/subdomain alias policy | None in v1 | Relaxes origin binding, the core security property |
| 5 | SPA/same-document success signal | None in v1; separate issue | False-positive save prompts are a security UX risk |
| 6 | Password-only/empty-username forms | Allow, show `(no username)` | Keychain item semantics and candidate UX |
| 7 | HTTP loopback fill | Allow only `localhost`/loopback | Local apps can bind loopback ports |
| 8 | Imported HTTP items | Readable, never filled | Existing imports already exist; filling them is new risk |
| 9 | Keychain access group / sandbox entitlements | Use the app's default access group; no sharing | Changes which processes can read items |
| 10 | Deletion path (`Settings`, `clear-data`, both) | Settings only; no `clear-data` | Destructive action scope |
| 11 | Memory zeroization | Evaluate `zeroize` on `Password` | Rust best-effort; not a guarantee |
| 12 | Subframe/iframe support | Refuse in v1 | Top-frame origin enforcement is subtle |
| 13 | Where the prompt lives | Native sheet owned by the active window | Prevents page clickjacking |

No owner decision above is encoded as “safe by default” in the model; the
model only refuses the unsafe direction.

## Residual risks

- **The page sees what it gets.** A filled password is a value in the page's
  own input; malicious page script can read and exfiltrate it after any fill or
  typing. This is inherent to web forms and is not solved by Relay.
- **Phishing.** Relay can show the origin in the picker, but a convincing page
  can still ask the user to choose and submit. No anti-phishing service or
  password-health check is included.
- **Same-user access.** The login Keychain is accessible to the local user and,
  depending on item ACLs, may prompt for other apps. A same-user process is
  already inside the accepted MCP boundary; this feature does not add a remote
  listener or a credential socket.
- **Memory.** `Password` prevents accidental formatting, not memory copies;
  Rust strings are not zeroized. Crash dumps and swap are outside this design.
- **Keychain/OS behavior.** Access prompts, iCloud Keychain, managed profiles,
  and hardware security policy vary by Mac. Only the owner's test machine and
  CI runner are evidence.
- **WebKit coupling.** `WKContentWorld`/`callAsyncJavaScript` isolation is
  Apple-supported but not independently audited here. A macOS run must prove
  the page cannot reach the helper; Linux type-checks cannot.
- **Save heuristic.** The success gate is deliberately conservative and misses
  same-document logins and some successful navigations. A false negative is a
  missed save prompt; a false positive would train approval of the wrong
  origin, which is why the heuristic is gated as above.
- **No server-verified success.** A site can return a 200 error page, a 2FA
  challenge, or an account-locked page with no password field; the heuristic
  would treat that as a candidate success and offer a save. The prompt shows the
  exact origin, and the user can decline, but Relay cannot prove the login
  succeeded. A future success signal must be owner-approved.
- **Origin strictness.** Users with a login saved at `www.` or a sibling
  subdomain will not be offered it. That is a usability cost accepted for v1.
- **Imported items.** Existing `http` imports remain in the Keychain but are
  never offered (only secure origins fill). Their paths are imports from a
  third-party CSV and are only ranking hints.
- **Public secret accessor.** `Password::expose` is public inside the crate's
  API so the native writer/fill code can use it; it is not a memory boundary.
  Any future code that calls it must be reviewed, and it must never feed a log
  or serialization path.
- **Review status.** An independent critic reviewed #57; its blockers (live-origin check at fill, prompt clearing on tab switch/sleep, stale docs) are fixed. Residuals are tracked as issues (default-ON offers, Replace/Keep prompt, Relay marker on Keychain items, https gate on offer, gesture window granularity, zeroizing passwords, macOS interactive evidence).

## Implementation scope

The feature is implemented: native Keychain list/read/save/delete, isolated
`relay.login` world capture and fill, trusted-gesture gating, live-origin check
at fill, save/decline banners, and Settings toggle plus delete. Private tabs
never capture or offer. Real-Mac interactive evidence is still owed.
