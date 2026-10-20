#!/usr/bin/env bash
# Run a hardware test suite against a real Indy through the in-OS MCP server.
#
# The lab configuration lives outside the repository:
#   ~/.config/irix7/lab.env
# with:
#   INDY_MCP_URL=http://indy-a.lab:7801/mcp
#   MCP_TOKEN=...
#
# This is a scaffold: it implements the MCP handshake and a few read-only
# checks. Extend it as the hardware milestones land.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

LAB_ENV="${IRIX7_LAB_ENV:-$HOME/.config/irix7/lab.env}"
if [ ! -f "$LAB_ENV" ]; then
  cat >&2 <<'EOF'
No lab configuration found.

Create ~/.config/irix7/lab.env with:

  INDY_MCP_URL=http://indy-a.lab:7801/mcp
  MCP_TOKEN=<the bearer token>

See docs/plan/hardware-lab.md for the rig, serial and power setup.
EOF
  exit 1
fi

# shellcheck disable=SC1090
source "$LAB_ENV"

: "${INDY_MCP_URL:?INDY_MCP_URL is not set}"
: "${MCP_TOKEN:?MCP_TOKEN is not set}"
: "${INDY_MACHINE:=indy-a}"
: "${INDY_SUITE:=smoke}"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$ROOT/artifacts/hardware/${INDY_MACHINE}-${INDY_SUITE}-${STAMP}"
mkdir -p "$OUT"

SESSION_ID=""
NEXT_ID=1

mcp_post() {
  local payload="$1"
  local headers="$OUT/headers.txt"
  local args=(
    -sS
    -D "$headers"
    -H "Authorization: Bearer $MCP_TOKEN"
    -H "Content-Type: application/json"
    -H "Accept: application/json, text/event-stream"
    -H "MCP-Protocol-Version: 2026-07-28"
    -X POST "$INDY_MCP_URL"
    --data "$payload"
  )
  if [ -n "$SESSION_ID" ]; then
    args+=(-H "Mcp-Session-Id: $SESSION_ID")
  fi
  curl "${args[@]}"
}

call_tool() {
  local name="$1"
  local args_json="$2"
  local id="$NEXT_ID"
  NEXT_ID=$((NEXT_ID + 1))
  local payload
  payload=$(printf '{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":"%s","arguments":%s}}' \
    "$id" "$name" "$args_json")
  mcp_post "$payload" > "$OUT/tool-${id}-${name//./-}.json"
  echo "  $name -> $OUT/tool-${id}-${name//./-}.json"
}

echo "== IRIX 7 hardware suite: $INDY_MACHINE / $INDY_SUITE"
echo "   endpoint: $INDY_MCP_URL"
echo "   output:   $OUT"

# Handshake.
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"irix7-hardware","version":"0.1.0"}}}'
mcp_post "$INIT" > "$OUT/initialize.json"
SESSION_ID=$(awk -F': ' 'tolower($1)=="mcp-session-id"{print $2}' "$OUT/headers.txt" | tr -d '\r' | tail -1)
echo "   session:  ${SESSION_ID:-<none>}"

mcp_post '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true

case "$INDY_SUITE" in
  smoke)
    call_tool "system.info" '{}'
    call_tool "shell.exec" '{"command":"uname","args":["-a"],"timeout_ms":10000}'
    ;;
  boot)
    call_tool "system.info" '{}'
    call_tool "serial.read" '{"length":4096,"timeout_ms":1000}'
    call_tool "screen.capture" '{"format":"png"}'
    ;;
  x11)
    call_tool "shell.exec" '{"command":"xterm","args":["-e","true"],"timeout_ms":10000}'
    call_tool "screen.capture" '{"format":"png"}'
    ;;
  xfs)
    call_tool "shell.exec" '{"command":"ls","args":["-la","/mnt/irix"],"timeout_ms":10000}'
    ;;
  full)
    call_tool "system.info" '{}'
    call_tool "shell.exec" '{"command":"uname","args":["-a"],"timeout_ms":10000}'
    call_tool "serial.read" '{"length":4096,"timeout_ms":1000}'
    call_tool "screen.capture" '{"format":"png"}'
    ;;
  *)
    echo "Unknown suite: $INDY_SUITE" >&2
    exit 2
    ;;
esac

echo "== done: $OUT"
