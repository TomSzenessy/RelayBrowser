# Privacy engineering

<!-- index: launch | Privacy-by-design engineering and rights workflow | Personal data, telemetry, cookies, retention, or user rights exist. -->

Privacy is a data-lifecycle property. This document starts the engineering
process; it is not legal advice or a GDPR conformity certificate.

## Data minimization by default

- Do not collect a field because it is convenient. Record purpose, source,
  subject, recipient, retention, access, and deletion path first.
- Keep anonymous, pseudonymous, and personal data distinct. A hash or opaque ID
  can still be personal data when linkable.
- Separate optional analytics/consent from necessary service operation. Never
  infer consent from account creation or bundle use.
- Minimize logs, traces, error payloads, uploads, backups, and support exports;
  redact at the boundary and bound retention.
- Treat free-form content, names, IDs, IP addresses, device data, payment
  metadata, and user-generated media as potentially sensitive.

## Rights and lifecycle

For every personal-data class, define a testable path for access/export,
correction, restriction, objection, consent withdrawal where applicable, and
erasure. Track the actual stores, replicas, backups, providers, and delayed jobs;
a database row deletion or HTTP success is not physical deletion evidence.
Record exceptions, owner, trigger, maximum period, and release condition.

### Semantic code search cache and consent

`make similar ... CODE=1` writes its local vector index under the ignored
`.agent/` cache: vectors, paths, line/byte ranges, source hashes, model,
endpoint, scope, and schema — never code or keys. That cache is private to the
resolved root: a physical root change — even a clone of the same origin and
branch at a different path — requires a fresh confirmation. The cache is
sent nowhere except the consent-gated embeddings call; a query export
confirms the same consent. A stored consent is withdrawn automatically
when the endpoint, model, eligible-file inventory, or any budget changes.
The confirmation record stores the canonical scope fields — the sorted
eligible paths, the file and chunk counts, and the policy — never code,
keys, or HMACs. A code edit that preserves the eligible path set and the
chunk count does not re-prompt; it only makes cached vectors stale by
content hash, so future eligible code is indexed under the stated policy
while changed chunks are re-embedded on the next explicit index build. Any
other inventory change — a file added, removed, renamed, or a chunk-count
change — requires a fresh confirmation. Similarity never authorizes
deleting or rewriting code, and nothing here is legal advice.
Consent binds the file scope (paths, counts, budgets, endpoint, model), not file
content: confirming a scope means "these files may be indexed", and every send
is an explicit `make similar CODE_INDEX=1` run that prints that scope. A file
edited while an index build runs is skipped as stale rather than recorded under
a mismatched hash.
See [`security.md`](./security.md#semantic-code-search-115) and
[`capabilities.md`](./capabilities.md).

## Cookies and browser storage

Inventory every cookie, local/session storage key, pixel, SDK, and third-party
embed by purpose, provider, duration, and whether it is strictly necessary.
Apply the applicable consent/terminal-device rule before optional access or
storage; provide a real withdrawal path. A source search does not prove runtime
behavior—test an authenticated production-like browser.

## Relay browser data and agent access

Relay is a local-only desktop browser. It operates no cloud sync service,
telemetry pipeline, crash-upload endpoint, or analytics SDK.

- **Profile creation:** the profile directory is created `0700` and the
  SQLite file `0600` before it is opened, on the CLI entry points as well as
  the GUI; a directory that already exists with a wider mode is tightened.
  The temporary copy of Chrome's History is created `0600` in one step and
  removed on both success and failure.
- **Screenshots:** `screenshot` returns a JPEG of the visible tab to the local
  agent in memory; Relay does not write it to disk or include it in the audit.
- **Agent JavaScript:** when enabled, `eval_js` returns the expression's JSON
  value to the local agent; results are capped at 100 KB. The feature is off
  by default and only runs on Allow sites.
- **Agent audit:** when a local MCP client calls a tool, Relay stores the tool
  name, normalized host, tab id, outcome, and time in `agent_audit` (5,000-row
  cap). Typed text, cookie values, and page content are never included.
- **Favicons:** while a page is open, Relay asks the page itself for its icon
  (`<link rel=icon>` or `/favicon.ico`) with `same-origin` credentials, a 3 s
  timeout, and a 256 KB cap; cross-origin icon URLs receive no cookies.
  Accepted PNG/JPEG/WebP/GIF bytes are cached 0600 in
  `relay_home()/favicons` (sanitized host names) and never sent to an agent.
- **Local profile storage:** History, bookmarks, settings, saved session tabs (http/https URL, title, pinned, scroll offset; max 200; replaced on every save; erased by `relay clear-data` and Settings → Clear history; not written at all with `restore_session off`), per-site preferences (zoom, compat, blocking, hidden selectors), installed
  extension metadata, and installed web app records (name, start URL, bundle
  path, install time) live in `$RELAY_HOME/profile.sqlite` (default
  `~/Library/Application Support/Relay` on macOS). The profile directory uses
  mode `0700` and `profile.sqlite` (along with its SQLite `-wal`/`-shm`
  sidecars) is hardened to mode `0600`. Nothing in the profile is synced to a
  Relay server or sent as telemetry.
- **Installed web apps:** Choosing “Install This Site as an App” writes a
  `.app` bundle the user asked for into `~/Applications` (Info.plist, a
  launcher, the site's manifest icon, and a marker recording the start URL).
  The launcher runs the installed Relay binary, so the app shares the
  browser's WebKit store, cookies, and updates instead of carrying a second
  engine. The bundle is the user's file; Relay only deletes one when the site
  is open and the matching marker is still present.
- **Passwords and form secrets:** Imported logins (`relay import-passwords`)
  are written directly to the macOS login Keychain through `Security.framework`,
  and the source plaintext CSV is deleted once every entry succeeds. Passwords
  are never stored in `profile.sqlite`, `Credential`'s `Debug` output redacts
  password values, and `js/bridge.js` suppresses `<input type="password">`
  values so password fields are never streamed from the page.
- **Connected MCP agent access:** While Relay is running, local agents connect
  over `$RELAY_HOME/mcp.sock` (mode `0600` inside the `0700` profile directory,
  accessible only to the local OS user). Over that socket, an attached agent can
  inspect the live DOM (minus password input values), console entries, network
  request/response metadata and text bodies up to 4 KB (`4096` bytes), and site
  cookies. Agent automation pauses automatically when a CAPTCHA or 2FA
  challenge is detected, and agents can only open `http(s)`/`about:blank` URLs
  and close their own tabs—they cannot close user tabs. Any data an external
  MCP client forwards to a model provider is governed by that client's policy.
- **User export and erasure:** Users can export local profile data with
  `relay export`, clear browsing history and the saved session with
  `relay clear-data --history --yes` (`--session` clears only the saved tabs), or erase all Relay-managed local state
  (profile, unpacked extensions, and socket state) by removing `$RELAY_HOME`.
  Passwords imported into the macOS Keychain are managed and deleted through
  macOS Keychain Access or the Passwords app.

## Launch gate

Before public launch, the operator and qualified counsel must validate the
actual jurisdiction, controller/operator identity, lawful bases, notices,
provider roles and transfers, retention, rights process, age/minor policy,
cookie behavior, security measures, and any DPIA/DPO/imprint requirements. Mark
each as `pending`, `implemented`, `verified`, or `approved`; do not turn a
source-level default into a legal conclusion.

Use [`legal/data-inventory.md`](./legal/data-inventory.md) for factual inventory,
including prompts/transcripts, issues, CI artifacts, memory, and skills, and
[`legal/README.md`](./legal/README.md) for the jurisdiction/counsel gate.
External research and source links live in
[`research/agentic-repository-baselines.md`](https://github.com/TomSzenessy/AI_TEMPLATE/blob/main/docs/research/agentic-repository-baselines.md).
