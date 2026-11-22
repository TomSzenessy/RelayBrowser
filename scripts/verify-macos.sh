#!/usr/bin/env bash
# One command an owner runs on a Mac: full tests, real-WebKit smoke, bridge
# tests, release build, Relay.app, signature check, doctor, and a report.
# Usage: bash scripts/verify-macos.sh [--no-open]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
open_app=1
[[ "${1:-}" == "--no-open" ]] && open_app=0

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "verify-macos: this must run on macOS; use 'cargo test --workspace' on other hosts" >&2
  exit 2
fi
for tool in sw_vers xcodebuild cargo rustup; do
  command -v "$tool" >/dev/null || { echo "verify-macos: missing $tool" >&2; exit 2; }
done

version="$(sw_vers -productVersion)"
major="${version%%.*}"
minor="${version#*.}"; minor="${minor%%.*}"
if (( major < 13 || (major == 13 && minor < 3) )); then
  echo "verify-macos: macOS $version is older than 13.3" >&2
  exit 2
fi

mkdir -p dist
report="$root/dist/macos-verification.txt"
: > "$report"
exec > >(tee -a "$report") 2>&1

say() { printf '\n== %s\n' "$*"; }
say "host"
sw_vers
xcodebuild -version
xcrun --sdk macosx --show-sdk-version
rustc --version
cargo --version
sysctl -n hw.ncpu | sed 's/^/cores: /'

say "cargo test --workspace"
cargo test --workspace --locked

say "real-WebKit smoke"
cargo run --locked -p relay-webkit --example smoke

say "bridge suite in Playwright WebKit"
if command -v npm >/dev/null; then
  (cd js && npm ci --ignore-scripts --no-audit --no-fund && npx playwright install webkit && npm test)
else
  echo "SKIP: npm not installed; the macOS CI job runs this suite"
fi

say "release build and bundle"
cargo build --release --locked -p relay-ui
bash scripts/bundle-macos.sh

say "doctor"
"$root/dist/Relay.app/Contents/MacOS/relay" doctor

say "result"
echo "All checks passed. Report: $report"
echo "Agents: claude mcp add relay -- \"$root/dist/Relay.app/Contents/MacOS/relay\" mcp"
if (( open_app )); then
  open "$root/dist/Relay.app"
fi
