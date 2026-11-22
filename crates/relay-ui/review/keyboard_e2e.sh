#!/usr/bin/env bash
# Behavioural end-to-end check of the real shell (no pixels needed): drive the
# GPUI window with keystrokes under Xvfb, then read the browser's own state
# back over MCP (`relay mcp` -> list_tabs). Linux: Ctrl is the ⌘ modifier.
set -euo pipefail
bin=${1:-target/debug/relay}
home=$(mktemp -d); export RELAY_HOME="$home" DISPLAY=:94
Xvfb :94 -screen 0 1440x900x24 >/dev/null 2>&1 & xvfb=$!
trap 'kill $app $xvfb 2>/dev/null || true; rm -rf "$home"' EXIT
sleep 1
"$bin" >/dev/null 2>&1 & app=$!
for _ in $(seq 60); do xdotool search --name Relay >/dev/null 2>&1 && break; sleep 0.5; done
win=$(xdotool search --name Relay | head -1)
xdotool windowfocus --sync "$win" 2>/dev/null || true
keys() { xdotool key --window "$win" "$@"; sleep 0.3; }
type_() { xdotool type --window "$win" --delay 30 "$1"; sleep 0.3; }

tabs() {
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_tabs","arguments":{}}}' \
    | { cat; sleep 1.5; } | timeout 5 "$bin" mcp | grep '"id":2' | python3 -c 'import sys,json; r=json.loads(sys.stdin.read()); print(json.loads(r["result"]["content"][0]["text"])["tabs"] and json.dumps([t["url"] for t in json.loads(r["result"]["content"][0]["text"])["tabs"]]) or "[]")'
}
fail() { echo "FAIL: $*"; exit 1; }

[ "$(tabs)" = "[]" ] || fail "fresh profile should have no tabs"
keys ctrl+k; type_ "docs.rs/gpui"; keys Return
keys ctrl+t; type_ "rust gpui"; keys Return
got=$(tabs); echo "after palette: $got"
[ "$got" = '["https://docs.rs/gpui", "https://duckduckgo.com/?q=rust+gpui"]' ] || fail "palette did not open both tabs: $got"
keys ctrl+w; got=$(tabs); echo "after close: $got"
[ "$got" = '["https://docs.rs/gpui"]' ] || fail "ctrl+w did not close the active tab: $got"
keys ctrl+shift+t; got=$(tabs); echo "after reopen: $got"
[ "$got" = '["https://docs.rs/gpui", "https://duckduckgo.com/?q=rust+gpui"]' ] || fail "ctrl+shift+t did not reopen: $got"
keys ctrl+k; type_ "zz"; keys Escape; got=$(tabs)
[ "$got" = '["https://docs.rs/gpui", "https://duckduckgo.com/?q=rust+gpui"]' ] || fail "escape should cancel the palette: $got"
echo "PASS: keyboard e2e"
