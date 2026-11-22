#!/usr/bin/env bash
# Cross-check the macOS-only crates from a non-Mac host. The Apple standard
# library comes from `rustup target add`; when that is unavailable the check
# says so and passes, because the real evidence is the macOS CI job
# (.github/workflows/macos.yml), not this type-check.
set -euo pipefail

target=aarch64-apple-darwin
libdir="$(rustc --print target-libdir --target "$target" 2>/dev/null || true)"
if ! ls "$libdir"/libcore-*.rlib >/dev/null 2>&1; then
  if command -v rustup >/dev/null 2>&1; then
    rustup target add "$target" >/dev/null
  else
    echo "SKIP: $target std is not installed and rustup is unavailable; the macOS CI job covers this code."
    exit 0
  fi
fi

exec cargo check --workspace --locked --examples --exclude relay-ui --target "$target"
