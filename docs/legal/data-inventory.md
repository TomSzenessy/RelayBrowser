# Data inventory and processing register

<!-- index: launch | Factual data-flow inventory | A data field, vendor, retention period, or transfer is added. -->

Project: `relay`
Date: `2026-11-22`
Reviewer: `TomSzenessy`

**Status:** working record — counsel review pending. Each row records verified
local-only product behavior as of `2026-11-22`; use `N/A — reason` when a field
does not apply. This document is not a formal Article 30 record or legal
conclusion.

| Activity / purpose | Data subjects | Data categories | Source | System / region | Recipient / role | Retention trigger and maximum | Deletion / export path | Legal basis / exception | Evidence / owner |
|---|---|---|---|---|---|---|---|---|---|
| Local browser profile (history, bookmarks, settings, saved session, site prefs, extension registry) | Local macOS user | Visited `http(s)` URLs, titles, visit counts/timestamps, bookmarks/folders, settings, open session tabs (URL, title, pinned, scroll; erased by clear-data via `Store::clear_session`), per-site prefs (host, zoom, flags, hidden selectors), installed extension metadata | User browsing in WKWebView; optional `relay import-chrome`; optional `relay install-extension` | Local device only (`$RELAY_HOME/profile.sqlite`, mode 0600 in 0700 dir; zero cloud sync, zero telemetry) | None (local user only; no Relay server) | Retained locally until cleared or profile removed | `relay export`, `relay clear-data --history --yes`, or remove `$RELAY_HOME` | `pending counsel review` | `crates/relay-extensions/src/store.rs`, `crates/relay-bridge/src/paths.rs`, `crates/relay-ui/src/main.rs` (`TomSzenessy`) |
| Website credential import and AutoFill | Local macOS user | Website origin URL, username, password | User-supplied Chrome CSV via `relay import-passwords` | Local macOS login Keychain via `Security.framework` (never stored in `profile.sqlite`; `bridge.js` never streams password input values) | None (local macOS Keychain only) | Retained in Keychain until user deletes entry; source CSV deleted immediately after full import | Delete entries in macOS Keychain Access / Passwords app | `pending counsel review` | `crates/relay-extensions/src/mac.rs`, `crates/relay-ui/src/main.rs`, `js/bridge.js` (`TomSzenessy`) |
| Live page inspection and automation for local MCP agents | Local macOS user (and rendered page content) | Live DOM (excluding password input values), console logs, network metadata and text bodies up to 4 KB, site cookies | Active WKWebView tabs via `js/bridge.js` and in-memory `SharedHub` | Local device only over `$RELAY_HOME/mcp.sock` (mode 0600 in 0700 dir); auto-paused on CAPTCHA/2FA; agents cannot close user tabs | User-attached local MCP client running as the same OS user (model-provider transfers governed by user's agent host) | In-memory for tab/session lifetime only; not persisted by Relay | Close tab, quit Relay, or disconnect local MCP client | `pending counsel review` | `js/bridge.js`, `crates/relay-bridge/src/hub.rs`, `crates/relay-mcp/src/lib.rs`, `docs/security.md` (`TomSzenessy`) |
| User-initiated Chrome Web Store extension install | Local macOS user | Requested extension ID and HTTPS client network metadata; unpacked extension files locally | Explicit `relay install-extension <id>` command (never automatic on Chrome import) | Outbound HTTPS fetch to Chrome Web Store; unpacked locally under `$RELAY_HOME/extensions/` | Google Chrome Web Store (third-party download host) | Local files kept until removed; remote logs per Google policy | Remove `$RELAY_HOME/extensions/<id>` and DB row, or remove `$RELAY_HOME` | `pending counsel review` | `crates/relay-extensions/src/crx.rs`, `crates/relay-ui/src/main.rs`, `docs/security.md` (`TomSzenessy`) |
| Direct web browsing and palette search | Local macOS user | Navigated URLs, web requests, and palette search queries (default DuckDuckGo, configurable via `search` setting) | User navigation in WKWebView and ⌘K palette | Direct device-to-site connections via system WebKit; Relay operates no proxy or telemetry backend | Visited websites and user-configured search provider | Per visited site / search provider; local WebKit store on device | Change `search` setting; `relay clear-data --history --yes`; remove `$RELAY_HOME` | `pending counsel review` | `crates/relay-ui/src/palette.rs`, `crates/relay-webkit/src/mac.rs` (`TomSzenessy`) |

## Browser and device state

| Name | Purpose | Provider | Type | Duration | Necessary or optional | Consent/withdrawal behavior | Evidence |
|---|---|---|---|---|---|---|---|
| `profile.sqlite` (`$RELAY_HOME/profile.sqlite`, mode 0600) | Local history, bookmarks, settings, extension registry | Relay (local device only; zero cloud sync) | local SQLite database | Until user clears or removes `$RELAY_HOME` | Strictly necessary for local profile | Export via `relay export`; clear history via `relay clear-data --history --yes`; remove `$RELAY_HOME` to erase | `crates/relay-extensions/src/store.rs`, `crates/relay-ui/src/main.rs` |
| `mcp.sock` (`$RELAY_HOME/mcp.sock`, mode 0600) | Local IPC socket for user-attached MCP agents | Relay (local user only) | Unix domain socket | Session lifetime while Relay runs | Optional local agent control | Only active while Relay runs; local OS user only; paused automatically on CAPTCHA/2FA | `crates/relay-bridge/src/paths.rs`, `crates/relay-mcp/src/lib.rs` |
| macOS login Keychain items | Imported website credentials for WebKit/Safari AutoFill | Apple macOS `Security.framework` (local device) | OS keychain item | Until user removes in Keychain/Passwords | Optional (`relay import-passwords` only) | User-initiated import; plaintext CSV removed on full success; delete via macOS Keychain Access / Passwords | `crates/relay-extensions/src/mac.rs`, `crates/relay-ui/src/main.rs` |
| `WKWebsiteDataStore` (visited site cookies/storage) | Session state for websites the user visits | Visited third-party sites (stored locally by macOS WebKit) | cookie/storage | Per visited site policy | Necessary for visited web apps | Site cookies are readable by attached local MCP agents over `mcp.sock`; user controls site navigation and local WebKit data | `crates/relay-webkit/src/mac.rs`, `crates/relay-mcp/src/lib.rs` |
| Relay telemetry / analytics / pixels | None verified — Relay ships zero analytics SDKs, pixels, crash uploaders, or cloud sync | N/A — local-only desktop app | none verified | N/A — none collected | N/A | N/A | `crates/relay-ui/src/main.rs`, `crates/relay-extensions/src/store.rs` |

## Rights and deletion drill

Relay has no hosted backend, user accounts, or cloud replicas; all product
profile data stays on the user's device under `$RELAY_HOME` and (for imported
passwords) in the macOS login Keychain. Users exercise export and deletion
directly via `relay export`, `relay clear-data --history --yes`, removing
`$RELAY_HOME`, and macOS Keychain Access / Passwords. Formal legal/counsel
review of rights notices and jurisdiction scope remains `pending counsel review`.
If hosted services or cloud sync are ever added, record the owner, authenticated
intake channel, identity-verification method, search scope across
stores/providers, response deadline basis, export format, secure delivery,
correction/restriction path, deletion target, retries/alerts, and physical
deletion evidence before launch.

## Agent and review data stores

Treat the following as possible data-processing stores even when they are not
part of the product database:

| Store | Possible contents | Owner / access | Retention and deletion evidence |
|---|---|---|---|
| Agent prompts, transcripts, and memory | User text, secrets accidentally included, project decisions | Host/provider; project owner | Host policy, redaction, deletion/export evidence |
| Issues, pull requests, and review packets | Intent, diffs, artifacts, vulnerability reports | Repository permissions | Issue retention, private advisory route, deletion evidence |
| CI logs and artifacts | Test output, build metadata, provider/deployment data | CI/provider | Log/artifact expiry and access review |
| Installed skills and provenance | Third-party instructions, scripts, source/revision, permissions | Project owner/host | Trust review, digest, update/rollback record |
| Support exports and analytics | User content, identifiers, usage events | Support/provider | Export minimization, retention, deletion path |

Record the factual role, provider, region, access boundary, and retention path;
do not infer a legal basis or compliance status from this table.

## Review triggers

Review this record when code/schema/config changes a data flow, a provider or
region changes, a new field is collected, a retention job changes, a rights
request/incident occurs, or the operator/jurisdiction changes. Link the current
issue and evidence rather than adding an open-task matrix here.
