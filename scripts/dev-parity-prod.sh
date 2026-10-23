#!/usr/bin/env bash
# dev-parity-prod.sh -- PRD-mcphost-dev-emulator AC8 (Live).
#
# Publishes a tool directory to a throwaway tenant on prod (bare
# $BASE/mcp, implicit signup), calls it with `host.tool_call`, runs the same
# call through `mcphost dev --no-sandbox --call`, and compares the error
# envelope: `data.error_code`, `data.line` and `data.source_line` must be
# equal. Prints `parity: ok code=<code> line=<line>` on success; the tenant is
# offboarded on exit. Needs bash, curl, python3 and the mcphost binary.
#
# Usage: dev-parity-prod.sh <tool-dir>
# Env:   MCPHOST_BASE_URL (default https://mcphost.dev), MCPHOST_BIN (default mcphost).
# e.g.   dev-parity-prod.sh tests/fixtures/parity/type-error-14
# exit: 0 parity, 1 mismatch or failure, 2 usage, 3 missing tool.
set -euo pipefail

BASE="${MCPHOST_BASE_URL:-https://mcphost.dev}"
BASE="${BASE%/}"
BIN="${MCPHOST_BIN:-mcphost}"
DIR="${1:-}"
if [ -z "$DIR" ] || [ ! -f "$DIR/tool.toml" ]; then
  echo "usage: dev-parity-prod.sh <tool-dir>   (a directory with tool.toml)" >&2
  exit 2
fi
for tool in curl python3 "$BIN"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "dev-parity-prod.sh: missing required tool: $tool" >&2
    exit 3
  fi
done

KEY=""

# rpc <tenant_key|-> <tool> <args-json>: prints the JSON-RPC reply's `error`
# or `result` object as {"error": ..} / {"result": ..} on stdout.
rpc() {
  local key="$1" tool="$2" args="$3" body resp
  body=$(python3 -c '
import json, sys
tool, args, key = sys.argv[1], json.loads(sys.argv[2]), sys.argv[3]
if key != "-":
    args["tenant_key"] = key
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "dev-parity-prod", "version": "1"}}}}))
' "$tool" "$args" "$key")
  resp=$(curl -sS --max-time 30 -X POST "$BASE/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'MCP-Protocol-Version: 2026-07-28' \
    -H 'Mcp-Method: tools/call' \
    -H "Mcp-Name: $tool" \
    -H 'x-mcphost-synthetic: script:dev-parity-prod' \
    --data-binary "$body")
  printf '%s' "$resp" | python3 -c '
import json, sys
msgs = []
for line in sys.stdin.read().splitlines():
    line = line.strip()
    if line.startswith("data:"):
        line = line[5:].strip()
    if line.startswith("{"):
        try:
            msgs.append(json.loads(line))
        except ValueError:
            pass
msg = next((m for m in reversed(msgs) if "result" in m or "error" in m), None)
if msg is None:
    sys.exit(1)
print(json.dumps({k: msg[k] for k in ("result", "error") if k in msg}))
'
}

# pick <json> <python-expr over d>: prints the value, empty when absent.
pick() {
  printf '%s' "$1" | python3 -c '
import json, sys
d = json.load(sys.stdin)
try:
    v = eval(sys.argv[1])
except Exception:
    v = None
print("" if v is None else v)
' "$2"
}

fail() {
  echo "parity: error reason: $1"
  exit 1
}

cleanup() {
  [ -z "$KEY" ] || rpc "$KEY" host.self.offboard '{}' >/dev/null 2>&1 || true
}
trap cleanup EXIT

PAYLOAD=$("$BIN" tool pack "$DIR") || fail "pack"
NAME=$(pick "$PAYLOAD" 'd["name"]')

SIGNUP=$(rpc - signup '{"name":"dev-parity","handoff":false}') || fail "signup unreachable"
KEY=$(pick "$SIGNUP" 'd["result"]["structuredContent"]["key"]')
[ -n "$KEY" ] || fail "signup bad_response"

PUBLISHED=$(rpc "$KEY" host.tool_publish "$PAYLOAD") || fail "publish unreachable"
[ -z "$(pick "$PUBLISHED" 'd.get("error")')" ] || fail "publish $(pick "$PUBLISHED" 'd["error"]["data"]["error_code"]')"

HOST=$(rpc "$KEY" host.tool_call "$(python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1], "args": {}}))' "$NAME")") \
  || fail "call unreachable"
LOCAL=$("$BIN" dev "$DIR" --no-sandbox --call "$NAME" '{}' || true)

H_CODE=$(pick "$HOST" 'd["error"]["data"]["error_code"]')
H_LINE=$(pick "$HOST" 'd["error"]["data"]["line"]')
H_SRC=$(pick "$HOST" 'd["error"]["data"]["source_line"]')
L_CODE=$(pick "$LOCAL" 'd["error"]["data"]["error_code"]')
L_LINE=$(pick "$LOCAL" 'd["error"]["data"]["line"]')
L_SRC=$(pick "$LOCAL" 'd["error"]["data"]["source_line"]')

[ -n "$H_CODE" ] || fail "host call returned no error envelope"
if [ "$H_CODE" = "$L_CODE" ] && [ "$H_LINE" = "$L_LINE" ] && [ "$H_SRC" = "$L_SRC" ]; then
  echo "parity: ok code=$H_CODE line=$H_LINE"
else
  echo "parity: MISMATCH host=($H_CODE,$H_LINE,$H_SRC) local=($L_CODE,$L_LINE,$L_SRC)"
  exit 1
fi
