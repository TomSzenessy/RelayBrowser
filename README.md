# Relay

<!-- repoctl:description -->
A fast, keyboard-first macOS browser in Rust (GPUI) on native WebKit, where one injected bridge powers both a native Inspector and a built-in MCP server for AI agents.
<!-- /repoctl:description -->
<!-- repoctl:project-readme -->
> Project initialized: **relay** (`desktop`)

Relay is a Mac browser for people who debug with coding agents. It does not ship
a browser engine: every tab is the macOS WebKit you already trust, so video
decoding, security patches and 120fps scrolling are Apple's job. Relay adds the
parts WebKit does not give you:

- a keyboard-first GPUI shell (⌘K palette, tabs down the side or across the
  top, native menu bar, splits, link hints);
- one ~41 KB injected bridge that streams DOM, network and console state into a
  single Rust store;
- a native Inspector and a local MCP server that read that **same** store, so
  Claude Code, Codex, OpenCode or Zed can answer "why is checkout failing?"
  from structured state instead of screenshots;
- Chrome bookmark/history/extension import, native content blocking, tab
  sleeping, and the real WebKit Web Inspector one shortcut away.

## Status

Early development, built in the open. Honest evidence as of this commit:

| Part | Evidence |
|---|---|
| Bridge (`js/bridge.js`) | Playwright WebKit 26.0 in Linux CI: 25 tests for DOM snapshots/patches, fetch/XHR bodies, console, errors, click/type/highlight, hints, 2FA detection, mute, and the async web-app manifest/icon and favicon ops |
| Shared state, MCP server, Chrome import, CRX, rules, sleep, theme contrast, CLI | `cargo test --workspace` in CI (279 test functions), including the real `relay mcp` stdio proxy, `config`/`export`/`clear-data`/`doctor` CLI, and WCAG AA contrast tests |
| macOS `WKWebView` engine + bridge | Type-checked for Apple Silicon (`scripts/mac-check.sh`); `.github/workflows/macos.yml` runs the workspace tests, real-WebKit `smoke` example, bridge suite, and `Relay.app` packaging on `macos-15` |
| GPUI window | Builds and links on Linux CI and macOS CI; visual polish is still judged on a Mac |
| `.app` bundle & DMG | `scripts/bundle-macos.sh` packages `Relay.app` (with `assets/AppIcon.icns`) and `Relay-macOS.dmg`; CI uploads both as artifacts and publishes `macos-latest` on GitHub Releases |

See [`docs/relay.md`](./docs/relay.md) for the architecture,
[`docs/privacy.md`](./docs/privacy.md) for local data and agent boundaries, and
[`docs/product/features.csv`](./docs/product/features.csv) for the live feature
matrix. Extensions need macOS 15.4 and are experimental: WebKit loads a
WebExtensions subset, many Chrome-only APIs are shimmed, and the Extensions
page shows each load error. Passkeys need Apple's restricted browser
entitlement; an ad-hoc build says so in Settings, and
[`docs/passkeys.md`](./docs/passkeys.md) gives the signed path.

## Quick start

macOS 13.3 or newer, Rust 1.100, and full Xcode (GPUI compiles Metal). Then:

```bash
cargo run --release -p relay-ui            # opens Relay
cargo run --release -p relay-ui -- https://example.com
```

Connect an agent to the running browser:

```bash
cargo build --release -p relay-ui
claude mcp add relay -- "$PWD/target/release/relay" mcp
```

`⌘K` opens everything: URLs, searches, tabs, history, bookmarks and commands.
`⌘T` new tab, `⌘W` close, `⌘⇧T` reopen, `⌘L` address, `⌘,` Settings, `⌘⌥I`
Inspector, `⌘\\` split, `⌘J` link hints, `⌘⌥U` WebKit's own Web Inspector,
`⌘⇧C` copy a clean URL, `⌘⇧M` copy the page as Markdown, `⌘D` bookmark, `⌘⌥P` pin (⌘W then sleeps a
pinned tab instead of closing it), `⌘F` find, `⌘=` / `⌘-` / `⌘0` zoom (remembered
per site), `⇧⌘R` reading mode, `⇧⌘P` picture-in-picture, `⇧⌘J` downloads, `⌘Y` History, `⇧⌘B` Bookmarks, `⇧⌘.` pause/resume all agents, `⌘⌥A` cycle agent access on the current site (Allow / Read only / Block); The Inspector sidebar has **Agent** and **Inspector** tabs and a × collapse button (⌥⌘I toggles it); Settings → Connect an agent copies the MCP config for Claude Code, Codex, OpenCode, Cursor, or Zed; Settings → Keyboard shortcuts records new keys (conflicts and bare keys are refused) and Settings → Appearance picks System/Light/Dark and an accent; tabs and the start page show each site's favicon (search, open, delete, and move a
bookmark to a folder).

The tab strip folds with `⌘S` and moves between the left, top, and right edge
with `⇧⌘S`; the choice is remembered. `⌘⌥B` hides the bottom bar. Settings
has ten pages (General, Appearance, Tabs, Browsing, Passwords, Privacy,
Extensions, Agents, Shortcuts, About), including a bookmarks bar, default
zoom, cookie-banner and site-data controls, and independent fill/save/autofill
switches. Every row shows a `×` on hover (and on the
active tab), and a middle click closes it too. Relay installs a native macOS
menu bar, so those shortcuts keep working while the page — not Relay — has
keyboard focus, and `Edit` (`⌘X`/`⌘C`/`⌘V`/`⌘A`) reaches the page's own editing
commands.

With no tab open, the new-tab page shows Recent, Frequent, installed Apps, and
Bookmarks; `relay config set start_sections "apps,recent"` sets their order,
and `none` hides them. `⌘K` on that page opens the same palette as everywhere
else.

### Install a site as an app

Open a site that publishes a web app manifest and choose **Install This Site as
an App** in `⌘K` (or File → Install This Site as an App). Relay writes a small
`.app` to `~/Applications` with the site's name and icon (Relay's icon when the
manifest has no usable PNG); launching it runs the
installed Relay with `--app <start-url>` — one WKWebView, no tab strip, no
Chrome runtime, no second MCP socket, and the same cookies and session as your
browser. Remove it with the matching palette command, which asks for a second
click and only deletes bundles Relay created.

Tests:

```bash
cargo test --workspace          # crates, UI logic, WCAG contrast, CLI, MCP stdio proxy
make check                      # repository gates
npm --prefix js test            # bridge in Playwright's WebKit (needs its browser)
```

On a Mac, one command runs everything and leaves a report:

```bash
bash scripts/verify-macos.sh            # tests, real-WebKit smoke, bridge suite,
                                        # Relay.app, signature check, doctor
bash scripts/verify-macos.sh --no-open  # same without launching the app
```

`relay doctor` prints the version, engine availability, data and socket paths,
profile counts, effective settings, and the exact `claude mcp add` command —
include its output in a bug report. `relay config`, `relay export [FILE]`, and
`relay clear-data --history --yes` manage local settings, backups, and erasure.
Open tabs come back asleep on relaunch (`relay config set restore_session off`
to disable); clearing history also erases the saved session, and `--session`
clears only that. Custom `!bangs`: `relay config set bangs "gh=https://example.com/s?q=%s"`.

## Working on this repository

People and agents start with [`AGENTS.md`](./AGENTS.md): `make start`, `make next`, `make done`; the documents
are indexed in [`docs/README.md`](./docs/README.md). The tooling comes from the kit at https://github.com/TomSzenessy/AI_TEMPLATE;
`make kit-update` brings its fixes.

## Everyday commands

Every command is a `make` target that runs the Python tool in `tools/`. The
list below is printed by `make help` and written here by `make sync` from the
same declarations, so the two can never disagree; editing this block by hand
fails `make check`.

Three words are used throughout the tools, so here they are first:

- A **capability** is one file that adds exactly one thing to the repository: a
  `skill` (repeated instructions an agent follows), an `agent` (a delegated role
  with its own access level and model choice), a `doc` (an owned document under
  `docs/`), a `rule` (one instruction agents must obey), a `check` (a test the
  tools run), a `command` (a new `make` target), an `mcp` route (a connection to
  an outside tool the agent may call, over the Model Context Protocol), or a
  `pack` (a named group of these that stays switched off until you turn it on).
- **Ceremony** is how much review and evidence a change needs: `low`, `normal`,
  or `high`. It follows where the change lands, not how many lines it touches.
- The **kit** is the shared tooling this repository ships and every project
  keeps: `tools/`, the `Makefile` targets, the git hooks, and `.agents/`.

<!-- repoctl:commands -->
```bash
EVERY SESSION (this is all most tasks need)
  make start                         Print the session brief: branch, handover, map, next step, what needs attention
  make next                          The single next step toward a complete product
  make where Q="login form"          Find files, symbols, owning docs, indexed doc triggers, rules, capabilities, read-only resources, and past failures
  make ui-review                     Screenshot the UI (phone/desktop, light/dark) for a judged review
  make done                          Before saying "done": heal derived files, gates, all tests

EXTEND THE SYSTEM (one path for every kind of capability)
  make new KIND=skill|agent|doc|rule|check|command|mcp|pack NAME=x DESC="what; when"
                                     Create any capability with valid metadata and wire it in
  make similar Q="release notes"     Does a similar capability (any kind, any pack) already exist? Optional CODE=1 adds consent-gated semantic code results (#115)
  make capabilities                  Every capability by kind and pack, plus tools, agent hosts, and MCP routes

WHEN NEEDED
  make risk                          How much ceremony this change needs (low, normal, high)
  make kit-report TITLE="..." BODY=path TYPE=... PRIORITY=... TOPIC=... | REASON="..."
                                     Draft kit feedback locally for owner review, or record why it is local
  make upstream [ID=<id>] [CHECK=1]  List pinned upstream records, print commit-pinned raw URLs for one, or CHECK=1 compare pins with the remote (network)
  make kit-publish TITLE="..." BODY=.agent/kit-reports/<draft>.md TYPE=... PRIORITY=... TOPIC=...
                                     File a reviewed kit-report draft on the template repository (the owner's step)
  make ci-status                     The CI result of what you pushed: green, red, pending, or why it cannot tell
  make ci-local [GROUP="verify"] [CHECK=1|LIST=1|PLAN=1|BLESS=1] [ALLOW_SKIP=1] [SLOW=1] [DIRTY=1]
                                     Run every locally reproducible workflow job on this exact tree before a push (PASS/FAIL/SKIP; repoctl exits 3 when incomplete)
  make ci-doctor                     Show the tools each local CI job needs, the detected versions, and the declared minimums
  make pull-first [CHECK_ONLY=1]     Before a run others rely on: refuse a dirty or unpushed tree, pull new commits (exit 3: restart)
  make ready                         Merge the default branch, run make done once, push once, then mark the PR ready
  make integrate BRANCHES="feature/a feature/b"
                                     Merge finished branches into one gated branch and open one PR carrying every Fixes #N
  make handover                      Write HANDOVER.md before pausing (git facts pre-filled)
  make map                           One-screen repository map
  make garden                        Full rot report: every check, advisory ones included
  make sync                          Regenerate derived files after editing their sources
  make issue BODY=path TITLE="..." TYPE=... PRIORITY=... AREA=... TOPIC=...
                                     Validate and file a duplicate-aware issue (or Local-WAL drafts)
  make review-packet ISSUE_FILE=path Print a fresh-agent critic brief for an issue
  make incident TITLE="..." SUMMARY="..."
                                     Create a private incident regression record

PROJECT SETUP AND MAINTENANCE
  make init NAME=my-project KIND=web [OWNER=you] [--answers file] [--guided] [--consent] [--reshape]
                                     Turn the template into your project
  make -f <kit>/Makefile adopt NAME=x KIND=web OWNER=you
                                     Bring this kit into an existing repository (--root) without overwriting it
  make kit-update [KIT=<template checkout>] [KIT_REF=<sha>]
                                     Pull template fixes into this project (keeps your changes)
  make check                         Every blocking check of the enabled packs
  make doctor                        Check initialization and the readiness of the declared phase
  make readiness                     Enforce public-launch evidence gates
  make inventory                     Show detected and declared product surfaces
  make resources                     Show the validated read-only resource router
  make skill-digest SKILL_PATH=.agents/skills/name
                                     Hash a reviewed skill directory for provenance
  make validate BODY=path            Validate a canonical issue body
  make labels                        Create or update the canonical GitHub issue labels
  make github-sync                   Apply project.toml description, topics, and template flag to GitHub
  make verify                        Tests, every surface's verification, and doctor (CI runs this)
  make test                          The kit's and every skill's unit tests
  make test-future                   The suite 800 days ahead: catches checks that rot with the calendar

Packs switched off hide: eval, trial (make capabilities shows how to enable them)
```
<!-- /repoctl:commands -->

**Once per session.** `make start` prints where you are: the branch, the last
handover note, a one-screen map of the repository, the single next step, and
anything that needs attention. It also installs the git commit gate.
`make where Q="checkout flow"` answers "where does this live?" with file paths,
code symbols, the document that owns each path, and past failures, so run it
before searching by hand. `make next` names the one step that moves a product
forward. `make done` is the gate before you call work finished: it rewrites the
generated files, runs the checks, and runs the tests.

**Before you edit.** `make risk` prints `low`, `normal`, or `high` -- the
ceremony this branch's change needs, decided from where the change lands.
`make sync` rewrites every file that is built from other files -- this command
list, the Makefile targets, the documentation index -- so run it after editing
whatever feeds them, or the commit gate refuses the change.

**When you add something to the repository.** `make similar Q="release notes"`
searches every kind of capability before you write a duplicate. When nothing
similar exists, `make new KIND=... NAME=... DESC="what it does; when to use it"`
writes that file with its metadata already wired in; `KIND` is one of the eight
kinds listed above (`repoctl new --help` prints them). `make capabilities` lists
everything the project already has.

**When something looks stale.** `make garden` runs every check, the advisory
ones included, and each finding names the fix that clears it. `make handover`
writes `HANDOVER.md` with the git facts filled in before you pause. `make map`
prints the repository map on its own.

**Before you file or hand off work.** `make issue` checks an issue body for
duplicates and files it on GitHub; with no remote configured it records it
locally as a `Local-WAL` (a write-ahead log: the issue is written down before
any code changes, so an interrupted session loses nothing).
`make review-packet ISSUE_FILE=path` prints a brief a fresh agent can judge the
work against.

## Layout

| Path | Role |
|---|---|
| `AGENTS.md` | The rules every agent session follows, plus a table that points to the right document for each kind of task (`CLAUDE.md` and `GEMINI.md` import it rather than repeat it). |
| `project.toml`, `resources.toml` | The facts the tools read instead of prose: which surfaces the project has, byte budgets, which changes count as risky, which groups of files are switched on, and which outside documents may be read. |
| `.agents/` | Everything you add to the project, one file each: skills, agent roles, rules, checks, commands, MCP routes, packs, and the fresh-agent benchmark. |
| `.claude/`, `.mcp.json` | Generated from `.agents/` for each agent tool; never edited by hand. |
| `docs/` | One owner document per concern; [`docs/README.md`](./docs/README.md) is the generated index. |
| `tools/` | `repoctl` (standard-library Python 3.11+) and the small focused modules it is split into. |
| `.githooks/`, `.github/` | The commit gate, CI, the weekly gardener, and issue forms. |

Live work belongs in GitHub Issues -- one issue per problem, carrying the
evidence that it was fixed -- not in a Markdown backlog on the side. See
[`docs/ISSUE_TEMPLATE.md`](./docs/ISSUE_TEMPLATE.md) and
[`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Requirements

Python 3.11+ and git. Optional: the GitHub CLI (`gh`) for issues and
`make github-sync`, and Node for the Playwright MCP route. Project toolchains
are declared per surface in `project.toml`.

## Before publishing a real project

Select a license, name accountable owners, configure branch protection and
secret scanning, replace legal placeholders with verified facts, and attach
runtime, provider, and counsel evidence to the launch issue. Start at
[`docs/production.md`](./docs/production.md).

The research behind these defaults is in the template repository
([baselines](https://github.com/TomSzenessy/AI_TEMPLATE/blob/main/docs/research/agentic-repository-baselines.md)); `make init`
removes it from new projects and points this link at the template.
