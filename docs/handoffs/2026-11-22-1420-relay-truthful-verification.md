# Handoff — truthful verification for Relay

<!-- index: operate | Handover record scaffold (local HANDOVER.md or committed handoff) | A fresh actor needs a focused continuation record. -->

- **Created (UTC):** `2026-11-22T14:20:00Z`
- **Next actor/session focus:** get the Linux CI gate green end to end, then obtain the first macOS runtime evidence.
- **Sharing boundary:** public repository (`github.com/TomSzenessy/RelayBrowser`).

## Objective

Relay is a macOS browser: a GPUI shell over native `WKWebView`, one injected
bridge feeding both a native Inspector and a local MCP server for coding
agents. This session's objective was to make every claim about it reproducible
and to fix the product defects that verification exposed — not to add breadth.

## Current state

- **Committed baseline:** `main` at `4e43123` (pushed; the repository is public).
- **Working tree:** clean.
- **Done (verified):**
  - 50 Rust tests pass locally (`cargo test --workspace --locked`): bridge
    protocol/hub, MCP round-trip with a real MCP client, real `relay mcp`
    stdio-proxy handshake, Chrome import, CRX3, content rules, sleep policy,
    palette, inspector, hints, extensions store.
  - `cargo check -p relay-ui` passes on Linux (the GPUI shell, palette,
    inspector and macOS-only `web.rs` module are type-checked as far as the
    host allows).
  - `bash scripts/mac-check.sh` type-checks `relay-webkit`,
    `relay-extensions`, `relay-mcp` and the real-WebKit `smoke` example for
    `aarch64-apple-darwin`.
  - Product fixes: page load failures carry WebKit's reason with an in-pane
    Retry; the native window title follows the active page; the profile DB and
    its WAL sidecars are 0600 (history is private); a woken tab keeps its sleep
    JPEG visible until the bridge delivers a DOM; MCP gained `wait_for` and
    `get_page_info`; `click_element`/`type_text` report the element acted on.
  - `macos` workflow (manual) builds/tests the workspace, runs the WebKit
    smoke example, runs the Playwright bridge suite, and packages an
    ad-hoc-signed `Relay.app`; `scripts/bundle-macos.sh` is checked in.
  - README, `docs/relay.md`, `docs/security.md` and the feature matrix now
    state exactly which layer produced which evidence.
- **Remaining (smallest next slice):**
  1. CI run `45032282701` (Node 22 added) must be observed to completion; fix
     the next bridge-js failure (likely Playwright WebKit host libraries/EGL).
  2. Run the manual `macos` workflow on a real Mac runner; fix compile/run
     failures there; attach `Relay.app` and smoke output as evidence.
  3. Only then judge the GPUI window visually and complete the `shell`,
     `inspector`, `states`, `accessibility`, `identity` rows in
     `docs/product/features.csv`.

## Evidence already collected

| Check / artifact | Result | Date/command | Evidence type |
|---|---|---|---|
| `cargo test --workspace --locked` (local) | **measured**: 50 passed, 0 failed | 2026-11-22, local `1.100.0` | `test` |
| `cargo check -p relay-ui` (local) | **measured**: finished, no errors | 2026-11-22 | `test/type-check` |
| `bash scripts/mac-check.sh` | **measured**: Darwin type-check finished | 2026-11-22 | `test/type-check` |
| `cargo test -p relay-ui --test mcp_proxy` | **measured**: real binary initialize/tools/list_tabs passes | 2026-11-22 | `test` |
| Linux CI `cargo test --workspace --exclude relay-ui` | **measured**: portable suites pass; overall run red | run `45025716241`, `45027927713`, `450302...` | `test` |
| macOS workflow | **hypothesis**: never ran; macOS hosted requests stay queued | runs `45021376984`, `45019294867` cancelled after 25–45 min queued | `none` |
| WebKit smoke example | **hypothesis**: compiles; behaviour unobserved | `cargo check --example smoke --target aarch64-apple-darwin` | `type-check only` |
| Playwright bridge suite | **hypothesis**: WebKit tests exist but could not launch on this Linux host (missing GTK/WPE EGL/Xvfb data) | local attempt 2026-11-22 | `none` |

## Rejected — do not retry

| Tried | Result | Commit / record |
|---|---|---|
| GitHub-hosted `macos-14` runner | rejected before steps: configuration unsupported | run `45018506075` |
| GitHub-hosted `macos-13` runner | stays queued indefinitely for this account | runs `45021376984`, `45021565724` |
| Full `cargo test --workspace` on the two-core self-hosted Linux runner | fills the disk (`No space left on device` in `ash`/`x11rb`) | runs `45025716241`, `45027927713` |
| `cargo check -p relay-ui` in Linux CI | same disk limit after portable tests | run `45027927713` |
| Zig as cross C compiler for `relay-ui` Darwin check | `gpui_media` build script runs bindgen with `xcrun`/Apple SDK on macOS only; no SDK here | local attempt 2026-11-22 |
| Playwright WebKit on this Linux host | WPE needs EGL; GTK needs Xvfb + `/usr/bin/xkbcomp` (no root); Chromium fallback was not attempted | local attempt 2026-11-22 |

## Budget and how to re-evaluate

- **Scarce budget used / left:** one session; self-hosted runner disk and
  macOS-runner availability are the real constraints.
- **Re-evaluate the current state with:**
  `cargo test --workspace --locked`, then
  `cargo check -p relay-ui`, then `bash scripts/mac-check.sh`, then
  `gh run list --limit 3` and `gh run watch <id> --exit-status`.

## Decisions and constraints

- Public repository was an explicit owner decision (session reply, 2026-11-22);
  it was made only after a secret scan of the working tree and full git history.
- Linux CI is deliberately a subset: portable tests, a `cargo clean`, and the
  Darwin type-check. `relay-ui` linking and its tests live on a Mac/developer
  machine because the self-hosted runner's disk cannot hold gpui's tree.
- The macOS workflow is `workflow_dispatch` on purpose; its triggers were
  removed after repeated queue-only runs. Re-add a push trigger only after a
  macOS runner is attached.
- The owner changed the public description from "120fps" to "fast"; the
  manifest now matches so `make sync` cannot revert it.

## Blockers and open questions

- **Blocker (owner/infrastructure):** no working macOS runner for this
  account. Hosted arm64 is rejected; Intel stays queued. The engine, `.app`
  bundle and GPUI window therefore have no runtime evidence.
- **Open:** Linux CI's bridge-js surface needs Node (added in `4e43123`) and a
  working Playwright engine. If WebKit cannot launch there, decide between a
  documented Chromium cross-engine run on Linux plus WebKit on macOS, or
  declaring the surface not-local on Linux with the reason.
- **Open:** the self-hosted runner's disk is small; `target/` and the Rust/
  Node toolchains compete with the repository. Cleanup steps exist but cannot
  fix a host that is already full.

## Exact next action

1. `gh run watch 45032282701 --exit-status`; read the first failing step with
   `gh api --allow-escape-sequences repos/TomSzenessy/RelayBrowser/actions/jobs/<id>/logs`.
2. Fix that one failure, commit, push, and observe the next checkpoint. Do not
   add features while the default branch is red.

## Relevant paths and issues

- [`VISION.md`](../../VISION.md) — owner brief and success evidence.
- [`docs/relay.md`](../../docs/relay.md) — architecture and the verification ladder.
- [`docs/product/features.csv`](../../docs/product/features.csv) — the live matrix.
- [`scripts/mac-check.sh`](../../scripts/mac-check.sh) — Darwin type-check.
- [`crates/relay-webkit/examples/smoke.rs`](../../crates/relay-webkit/examples/smoke.rs) — real-WebKit smoke.
- [`crates/relay-ui/tests/mcp_proxy.rs`](../../crates/relay-ui/tests/mcp_proxy.rs) — stdio proxy test.
- [`.github/workflows/macos.yml`](../../.github/workflows/macos.yml) — manual macOS job.
- Local-WAL-001 (`.agent/wal/Local-WAL-001.md`, ignored) — original build record.

## Verified skill suggestions

`none` — no new skill was reviewed or installed this session.

## Redaction and retention check

No secrets, credentials, tokens, raw personal data or full exports are present.
The record is safe for the public repository. Delete or supersede it once the
macOS job and green Linux CI make it stale.
