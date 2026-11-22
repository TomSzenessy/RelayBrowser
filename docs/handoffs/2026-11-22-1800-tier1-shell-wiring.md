# Handoff — Relay Tier 1 shell wiring (PR #40, draft)

<!-- index: operate | Handover record scaffold (local HANDOVER.md or committed handoff) | A fresh actor needs a focused continuation record. -->

- **Created (UTC):** `2026-11-22T18:00:00Z`
- **Next actor/session focus:** observe Tier 1 on a Mac from the PR's `Relay-macOS` artifact, mark draft PR #40 ready (that starts the Linux CI jobs, which skip drafts), merge, then #36 residuals and Tier 2.
- **Sharing boundary:** public repository (`github.com/TomSzenessy/RelayBrowser`).

## Objective

Wire the Tier 1 daily-driver conveniences (issue #39) into the GPUI shell and
WebKit engine on top of the store/bridge checkpoint `95bffa3`.

## Current state

- **Branch:** `feat/tier1-daily-driver` @ `8a1a6ee` plus this record. These commits
  are **local only**: the session host had no GitHub credentials and no `gh`.
  The work was done in a separate worktree because the main checkout's `.git`
  and two tracked files (`crates/relay-webkit/examples/smoke.rs`, the
  `2026-11-22-1420` handoff) were owned `opc:opc` and not readable/writable for
  the agent user, which blocks fast-forwarding `main` there.
- **Done on the branch (Refs #39):**
  - `2db7c78` `relay clear-data --session`; `--history`/default also clears the saved session (CLI test).
  - `6401078` lazy session restore: tabs saved on quit and on the 30 s tick; on launch they come back asleep (`Engine::restore_asleep`), only the active one loads; config key `restore_session` (default on).
  - `a03d0c1` ⌘⇧M copy page as Markdown (`Op::Markdown`), ⌘⇧C copy clean URL.
  - `be050ae` ⌘D bookmark toggle.
  - `0525113` ⌘⌥P pin (`Hub::set_pinned`, pinned block at top, tested); ⌘W sleeps a pinned tab.
  - `6cff7f0` ⌘F find bar (`Engine::find`, `WebEvent::Found`); ⌘=/⌘-/⌘0 per-host zoom (`SitePrefs.zoom`, applied on load).
  - `8a1a6ee` critic fixes: scroll restore only after a real load (`page_loaded_since`), `restore_session off` writes nothing and clears at launch, save on every quit path (`on_app_quit`), Settings → Clear history clears the session, new tabs land after the pinned block.

## Evidence collected

| Check | Result | Command |
|---|---|---|
| relay-ui tests | **measured**: 52 unit + 6 CLI + 1 MCP proxy pass | `cargo test --locked -p relay-ui` |
| relay-bridge tests | **measured**: pass incl. `pinned_tabs_form_a_block_at_the_top` | `cargo test --locked -p relay-bridge` |
| Darwin type-check | **measured**: pass (excludes relay-ui) | `bash scripts/mac-check.sh` |
| Local CI | **measured**: rust verify + test-future + garden pass; `npm test` failed only because host Node is v12 (no `node --test`); no JS changed | `make ci-local` with python3 → 3.12 |
| Independent critic | **ship-with-residuals** on `8a1a6ee` (first pass found 2 blockers, fixed) | critic role |
| macOS runtime | **not observed** — the macOS-only `imp` in `crates/relay-ui/src/web.rs` is not compiled on Linux | — |

## Update 2026-11-22 ~19:00 UTC

- Pushed; draft PR #40 (`Fixes #39`). All Tier 1 items are now on the branch: in addition to the list above, custom bangs (`relay config set bangs`), cookie-banner hiding (`CMP_FILTERS`), per-site blocking and compat-mode palette toggles, and a tab-bar audio marker.
- macOS-15 CI (manual dispatch; draft PR runs skip by design) passed on `b14aa7d` (run 45055235796) and `5a94fca` (run 45055894618): relay-ui compiled, 54 unit + 7 CLI + all crate tests, WebKit smoke sleep/wake live DOM, 5.8 MB DMG. A run on `23fb71d` was queued at handover.
- Second critic: `blocker` (reload before recompiled rules applied; unresolvable bang triggers) fixed in `23fb71d`, then `ship-with-residuals`. Residuals documented in `docs/relay.md`: compat misses cross-host redirects and loads twice; failed rule recompile only logs.
- Linux CI (`ci.yml`) has not run on this branch yet because the PR is a draft.

## Exact next action

1. From a host with GitHub access: `git push origin feat/tier1-daily-driver`, open the PR (`Fixes #39` only once Tier 1 is complete; else `Refs #39`), `make ci-status` until green, including `macos-15`.
2. On a Mac: relaunch with tabs open (restore asleep, scroll on click), ⌘F, ⌘=/⌘-/⌘0 across relaunch, ⌘⌥P + ⌘W, ⌘D, ⌘⇧M, ⌘⇧C; flip `docs/product/features.csv` rows to `yes` only with that evidence.
3. Remaining Tier 1: audio indicator in the tab bar, per-site blocking/compat mode, custom bangs from `settings("bangs")`, conservative CMP cookie-banner rules; then #36 residuals and Tiers 2–4 (see `2026-11-22-1653-tier1-checkpoint.md`).

## Residual risks (from the critic)

- macOS runtime unobserved for restore, scroll, find, zoom, pin, quit-save.
- `relay <url>` also wakes the restored active tab (extra awake hidden tab).
- No tests for the 30 s change detection or Find key handling.

## Redaction and retention check

No secrets, credentials, or personal data. Public-safe.
