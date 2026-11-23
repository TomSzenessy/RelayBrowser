# Threat Model — Relay Browser

<!-- index: design | STRIDE threat model and accepted trust boundaries | A security-sensitive tool, CI path, or agent permission is reviewed. -->

**Last Updated:** 2026-11-23
**Version:** 2.0.0
**Methodology:** STRIDE + natural-language analysis

## 1. System overview

Relay is a local macOS browser. A GPUI shell presents tabs and native menus; each
awake tab is a `WKWebView`; one injected `js/bridge.js` streams page state into a
shared Rust `SharedHub`; the native Inspector and a local MCP server read that
same hub. An installed Chrome extension runs inside WebKit with the permissions
granted at install. The profile (history/bookmarks/settings/audit) lives in
`$RELAY_HOME/profile.sqlite`; downloads leave the profile for
`~/Downloads/Relay`; Chrome import reads bookmarks/history and can write
passwords to the macOS login Keychain.

### Components

| Component | Purpose | Criticality | Entry points |
|---|---|---:|---|
| GPUI shell (`relay-ui`) | Tabs, palette, library, sidebar, settings, downloads | High | Local keyboard/mouse; native menus |
| WKWebView + injected bridge | Render pages; post DOM/console/network/hotkeys/results | Critical | Untrusted page content; arbitrary `https`/`http` |
| `SharedHub` (`relay-bridge`) | One state store for shell, Inspector, MCP | Critical | Bridge messages; local MCP calls |
| MCP server/socket | Let a local coding agent read and act on pages | Critical | `$RELAY_HOME/mcp.sock` (0600, same-user) |
| Profile store | History, bookmarks, settings, session, policies, audit | High | `profile.sqlite` (0600 in 0700 dir) + WAL/SHM |
| Extension loader | Unpack CRX, grant permissions, load in WebKit | High | Chrome Web Store download; local package |
| Chrome importer | Read bookmarks/history/extensions/passwords | High | Chrome profile files; temp 0600 copies |
| Keychain import | Write internet passwords | Critical | Plaintext CSV supplied by the user |
| Download manager | Save WebKit downloads | Medium | Untrusted response body/filename |
| CI/release | Build, test, package, publish | Medium | GitHub Actions; repository code |

## 2. Trust boundaries and zones

### Zone A — untrusted page content

Page script controls everything the bridge observes. A page can forge any
message shape, post directly to `window.webkit.messageHandlers.relay`, make
huge payloads, or try prompt injection against the agent reading page text.
Enforcement:

- iframe messages are dropped (`BridgeHandler.did_receive` checks `isMainFrame`);
- native `ingest_json` drops >2 MiB before parsing and caps title/URL, console,
  network, markdown, DOM text/attrs, and node counts (`MAX_*` in `hub.rs`);
- URLs are one shared policy: `http(s)` or exactly `about:blank`
  (`relay_bridge::is_openable_url`);
- agents are told page output is untrusted data, not instructions.

### Zone B — local same-user processes

Any process running as the user can connect to `mcp.sock` and drive the browser,
including the user's logged-in sessions. This is the intended trust level for a
local coding agent, not a sandbox. Enforcement/controls:

- socket 0600 in a 0700 directory; a live socket is never stolen;
- user Stop (`agents_paused`) fails every tool before page access;
- per-host Allow/Read-only/Block is enforced in the MCP before resolving/mutating;
- `eval_js` is off by default and additionally needs Allow;
- every tool writes a lean audit row (tool/host/tab/outcome/time, never typed
  text or page content); typed text/cookies are redacted or policy-gated.

### Zone C — profile data at rest

`profile.sqlite` and sidecars are created 0600 in a 0700 directory. They contain
history, bookmarks, settings, saved session, extension registry, agent policies,
and the audit log. `relay export` writes 0600 JSON without passwords. Clearing
history checkpoints the WAL and vacuums deleted rows.

### Zone D — extensions and downloads

Extensions are code the user installed. WebKit executes them with every
permission/host pattern the manifest requested; install prints the requested
list and states that installing is blanket consent. CRX download has a 30 s
timeout and 256 MiB compressed cap; parsing bounds-checks headers; unpack
refuses >256 MiB decompressed and cleans staging. CRX3 signatures are **not**
verified: integrity rests on HTTPS to Google plus the header `crx_id` matching
the requested id; packages without an id are refused. Downloads are saved with a
sanitized basename under `~/Downloads/Relay` and are never executed.

### Zone E — passwords and imported credentials

`relay import-passwords` parses a plaintext Chrome CSV, writes internet
passwords to the macOS login Keychain, and deletes the CSV only when every entry
saved. `Credential`'s `Debug` redacts the password. Passwords are never written
to `profile.sqlite` or included in export. A future autofill/save UI (issue #57)
must keep values out of the bridge snapshot, MCP, and logs, and bind fills to
the page origin.

### Zone F — CI/release

CI runs on GitHub-hosted runners with least privilege and uploads only release
artifacts and a scoped diagnostics allowlist (`Relay*.ips` plus a job-local
`.relay-home` log directory). The macOS job sets `RELAY_HOME` inside the
workspace so a self-hosted runner's real profile is never read. Release
publishing is a separate `contents: write` job.

## 3. STRIDE analysis

### Spoofing

- A subframe could pretend to be the main page: dropped by `isMainFrame`.
- A local process could claim to be another MCP client: the socket has no
  per-client auth; same-user access is the accepted boundary. The advertised
  client name is self-asserted and used for the per-client grant and audit, not
  as authentication.
- A CRX could declare another id: `parse` extracts `crx_id`; `unpack` refuses a
  mismatch or a package without a CRX3 id.

### Tampering

- Page can send malformed patches/snapshots: duplicate ids, cycles, huge trees,
  and oversized fields are rejected/truncated natively; patch cost is
  proportional to the patch.
- Page can change a moved element's id: `byId` cleanup waits for the coalesced
  flush and checks `isConnected`; detached ids are rejected by `need`.
- Local attacker can edit the profile: same-user file permissions are the
  boundary; export/clear are explicit user commands.

### Repudiation

- Agent actions write `agent_audit` rows (tool/host/tab/outcome/time), capped at
  5,000, with no page content or typed text. `relay audit` and
  `clear-data --audit` exist.
- Shell read tools, plans, denied applies, and successful applies are audited.
  An apply row carries the self-asserted MCP client name, plan id, result, and
  before/after hashes for each changed key; values, widget text, and page data
  are never stored. An unknown or expired plan id writes no audit row because
  nothing was applied.

### Information disclosure

- DOM/console/network snapshots can contain private page data; agents see what
  the user could see. Password/card/one-time-code values are redacted in the
  bridge; cookies are policy-gated and never audited. Screenshots can contain
  private pixels and are returned to the local agent only.
- `eval_js` can read page data only after explicit opt-in and Allow.
- Chrome import copies History + WAL/SHM to 0600 temp files and removes them;
  a concurrent Chrome writer can still race the copy.
- `relay export` excludes passwords; the Keychain CSV is deleted only after
  every entry saves.

### Denial of service

- Huge bridge messages/trees are bounded natively; console/network rings are
  capped; tabs sleep after inactivity (pinned/audible/agent-active tabs are
  exempt); downloads/CRX unpack are byte-capped.
- Shell plans are capped at 32 pending ids, expire after 60 s, and bound each
  setting, start-section list, diff, widget, and read output; plan/apply
  arguments reject unknown JSON fields.
- A page can still use CPU in its own WebKit process; Relay does not sandbox
  page script beyond WebKit's process model.

### Elevation of privilege

- `eval_js` is the highest-privilege agent tool: off by default, needs Allow,
  and is refused on Stop/Block/Read-only.
- Agent policy cannot be escalated by a tool argument; `open_tab` to a Block
  host creates nothing; `read_dom` has no direct-hub fallback.
- Shell writes are off by default and need a global switch or a per-client
  grant, checked again on every plan/apply. They are plan/apply only, bounded
  to allowlisted settings, named AA-tested palette tokens, ordered start
  sections, inspector/agents/off, validated shortcuts, and one plain-text
  widget; no shell tool takes HTML, a URL, a plugin, or a script, and unknown
  fields fail before validation. Revoking a grant is immediate.
- Extension permissions are blanket-granted at install (residual below).

## 4. Accepted risks (and the enforcing path)

| Accepted risk | Why accepted | Enforced by |
|---|---|---|
| Same-user local process can drive the browser | Intended for local agents | `mcp.sock` 0600/0700; no remote listener |
| Agents can read pages/cookies under policy | The user attached them | per-host policy; Stop; redaction; audit |
| `eval_js` can run page code | Needed for hard sites | off by default; Allow only; audit |
| Screenshots may contain private pixels | User asked for a screenshot | visible-tab only; local socket; not stored |
| CRX3 signatures not verified | Download is HTTPS from Google | id match; bounds; docs/security |
| Extension install grants all requested permissions | WebKit blanket grant | install prints full report; no withholding yet |
| Chrome import can race a running Chrome | Copy is a snapshot | WAL/SHM included; best effort |
| Downloads are not malware-scanned | User chose the file | sanitized destination; no auto-execute |
| Shell tools can change the browser after a grant | Owner asked for agent customization | off by default; per-client grant and live revoke; plan/apply; allowlists; audit |

## 5. Residual risks and owned issues

- Shell writes are declarative only. Sandboxed HTML widgets remain unbuilt and
  need their own issue plus an isolation review before they are attempted.
- The macOS visual observation of Agent-sidebar Grant/Revoke and live menu
  rebinding is still an owner check; Linux tests cover the logic and code path.
- **#54** download progress/cancel/retry and keyboard/VoiceOver access.
- **#57** explicit login fill/save; must keep values outside bridge/MCP/logs.
- **#64** GPUI accessibility tree; VoiceOver is not available today.

## 6. Review status

This model was rewritten against the code on 2026-11-23. It has **not** had an
independent security review; `.security/config.json` records the owner and
pending status. Private reports use the enabled GitHub private advisory form
(also named in `SECURITY.md`); `security_reviewed_on`/`reviewer` must be
completed before public launch.
