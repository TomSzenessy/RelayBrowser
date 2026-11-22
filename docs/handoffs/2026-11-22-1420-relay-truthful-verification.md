# Handoff — Relay after its first green CI

<!-- index: operate | Handover record scaffold (local HANDOVER.md or committed handoff) | A fresh actor needs a focused continuation record. -->

- **Created (UTC):** `2026-11-22T14:42:00Z`
- **Next actor/session focus:** run the manual `macos` workflow on a real Mac runner and judge the GPUI window.
- **Sharing boundary:** public repository (`github.com/TomSzenessy/RelayBrowser`).

## Objective

Relay is a macOS browser: a GPUI shell over native `WKWebView`, one injected
bridge feeding both a native Inspector and a local MCP server for coding
agents. This session made its claims reproducible, fixed the defects
verification exposed, and got Linux CI green.

## Current state

- **Committed baseline:** `main` @ `9249634` plus this handover commit (pushed).
- **Working tree:** clean.
- **Done (measured):**
  - Linux CI is green on `ubuntu-24.04` (run `45033566781`): both matrix
    interpreters and the 800-day test pass.
  - 50 Rust tests, including relay-ui's unit tests and the real `relay mcp`
    stdio proxy, pass in CI and locally.
  - 8 Playwright WebKit 26.0 bridge tests pass in CI: DOM snapshots/patches,
    fetch/XHR status and bodies, console and uncaught errors,
    click/type/highlight, hints, 2FA detection.
  - `bash scripts/mac-check.sh` type-checks `relay-webkit`,
    `relay-extensions`, `relay-mcp` and the `smoke` example for
    `aarch64-apple-darwin`.
  - Product fixes: load failures with WebKit's reason and Retry; live window
    title; 0600 profile and WAL sidecars; sleep JPEG visible through a wake
    until the new DOM; MCP `wait_for`/`get_page_info`; clearer click/type
    results; truthful docs and matrix.
  - CI infrastructure: hosted Ubuntu 24.04 (free for this public repo),
    Node 22, Rust 1.100, GPUI Linux build libraries, Playwright WebKit with
    system deps, pinned workflow digests, cleanup steps.
- **Remaining (smallest next slice):** run `gh workflow run macos.yml` on a Mac
  (or attach a macOS self-hosted runner), fix the first real-WKWebView/compile
  failure, and attach the smoke output plus `Relay.app` as evidence.

## Evidence already collected

| Check / artifact | Result | Date/command | Evidence type |
|---|---|---|---|
| Linux CI | **measured**: success, both Python matrices | run `45033566781`, 2026-11-22 | `test` |
| `cargo test --workspace --locked` | **measured**: 50 passed | CI + local `1.100.0` | `test` |
| Playwright WebKit 26.0 bridge suite | **measured**: 8 pass, 0 fail | CI run `45033566781` | `test` |
| `bash scripts/mac-check.sh` | **measured**: Darwin type-check passed | CI + local | `test/type-check` |
| `cargo test -p relay-ui --test mcp_proxy` | **measured**: real binary handshake passed | local and CI | `test` |
| `cargo check -p relay-ui` | **measured**: passed (Linux) | local | `type-check` |
| macOS workflow | **hypothesis**: never ran; hosted macOS requests stay queued | runs `45021376984`, `45019294867` cancelled queued | `none` |
| WebKit smoke example | **hypothesis**: compiles; runtime unobserved | `cargo check --example smoke --target aarch64-apple-darwin` | `type-check only` |
| GPUI window pixels | **hypothesis**: never observed | none | `none` |

## Rejected — do not retry

| Tried | Result | Commit / record |
|---|---|---|
| Hosted `macos-14` runner | rejected before steps: configuration unsupported | run `45018506075` |
| Hosted `macos-13` runner | stays queued indefinitely for this account | runs `45021376984`, `45021565724` |
| `runs-on: ubuntu-latest` for CI | Ubuntu 26.04; Playwright 1.56 refuses WebKit | run `45033036639` |
| Self-hosted Linux runners for the Rust gate | disk full (`ash`, `chrono`, `x11rb` ENOSPC) | runs `45025716241`, `45032282701` |
| Zig as cross C compiler for relay-ui Darwin | `gpui_media` bindgen needs Apple SDK via `xcrun` | local attempt |
| Playwright WebKit on this Linux dev host | WPE EGL / GTK Xvfb unavailable without root | local attempt |

## Budget and how to re-evaluate

- **Scarce budget used / left:** macOS runner availability is the only costly
  gate left; Linux CI is free and green.
- **Re-evaluate with:** `gh run list --limit 3`; `cargo test --workspace
  --locked`; `bash scripts/mac-check.sh`; `gh workflow run macos.yml`.

## Decisions and constraints

- Public repository was the owner's explicit decision; a full-history secret
  scan preceded it.
- CI runs on GitHub-hosted `ubuntu-24.04`, pinned because Playwright 1.56 does
  not support 26.04. The macOS job stays manual until a Mac runner exists.
- `opt-level = 0` for dev dependencies keeps cold CI builds short; release is
  unchanged.
- The owner's public description says "fast", not "120fps"; the manifest and
  README match so `make sync` cannot revert it.

## Blockers and open questions

- **Blocker (owner/infrastructure):** no working macOS runner for this
  account. Hosted arm64 is rejected; Intel queues indefinitely. The engine,
  `.app` bundle and GPUI window therefore have no runtime evidence.
- **Open:** whether the WebKit smoke example's offscreen `WKWebView` and
  `takeSnapshot` behave on a real Mac; that is the first thing the macOS job
  will answer.
- **Open:** extensions require macOS 15.4; the owner's Mac version is unknown.

## Exact next action

1. `gh workflow run macos.yml && gh run watch <id> --exit-status`.
2. On failure, fix the first compile/runtime error, push, and rerun; on
   success, download `Relay-macOS.zip`, launch the app, and run
   `crates/relay-ui/review/keyboard_e2e.sh` plus a visual review.

## Relevant paths and issues

- [`VISION.md`](../../VISION.md) — owner brief and success evidence.
- [`docs/relay.md`](../../docs/relay.md) — architecture and six verification layers.
- [`docs/product/features.csv`](../../docs/product/features.csv) — live matrix.
- [`crates/relay-webkit/examples/smoke.rs`](../../crates/relay-webkit/examples/smoke.rs) — real-WebKit smoke.
- [`crates/relay-ui/tests/mcp_proxy.rs`](../../crates/relay-ui/tests/mcp_proxy.rs) — stdio proxy test.
- [`.github/workflows/macos.yml`](../../.github/workflows/macos.yml) — manual macOS job.
- [`scripts/mac-check.sh`](../../scripts/mac-check.sh) — Darwin type-check.
- Local-WAL-001 (`.agent/wal/Local-WAL-001.md`, ignored) — original build record.

## Verified skill suggestions

`none` — no new skill was reviewed or installed this session.

## Redaction and retention check

No secrets, credentials, tokens, raw personal data or full exports are present.
Safe for the public repository. Supersede once the macOS job has run and the
GPUI window has been reviewed.
