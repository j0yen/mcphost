#!/usr/bin/env bash
# empty-call.sh -- PRD-mcphost-empty-call-refusal-stated-in-schema AC6/AC7.
#
# Posts `{}` as the arguments of <verb> to the prod /mcp with the operator key
# and prints the reply's `error.data` (or `result`) as one JSON object, so a
# refusal's `expected` / `path` can be read the way an agent reads it. The key
# is read over ssh from mcphost-1 into one shell variable and never printed.
# Needs bash, curl, python3, ssh to mcphost-1.
#
# Usage: empty-call.sh <verb>
#        empty-call.sh --selftest      offline: body, headers and reply parsing
# Env:   MCPHOST_BASE_URL (default https://mcphost.dev), MCPHOST_SSH_HOST
#        (default mcphost-1), MCPHOST_ENV_FILE (default /etc/mcphost/env).
# e.g.   empty-call.sh host.import | jq -e '.expected[0].required'
# exit: 0 ok, 1 failure, 2 usage, 3 missing tool.
set -euo pipefail

BASE="${MCPHOST_BASE_URL:-https://mcphost.dev}"
BASE="${BASE%/}"
SSH_HOST="${MCPHOST_SSH_HOST:-mcphost-1}"
ENV_FILE="${MCPHOST_ENV_FILE:-/etc/mcphost/env}"
PROTOCOL="2026-07-28"

# request_body <verb>: the tools/call frame with `{}` arguments.
request_body() {
  python3 -c '
import json, sys
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": sys.argv[1], "arguments": {}, "_meta": {
        "io.modelcontextprotocol/protocolVersion": sys.argv[2],
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "empty-call-live", "version": "1"}}}}))
' "$1" "$PROTOCOL"
}

# reply_data: stdin is a JSON or SSE reply; prints error.data (or result) as
# one JSON object, rc 1 when there is neither.
reply_data() {
  python3 -c '
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
if "error" in msg:
    out = msg["error"].get("data")
    out = out if isinstance(out, dict) else {"message": msg["error"].get("message")}
else:
    res = msg["result"]
    out = res.get("structuredContent", res) if isinstance(res, dict) else {"result": res}
print(json.dumps(out, sort_keys=True))
'
}

selftest() {
  local body sse out
  body=$(request_body host.import)
  python3 -c '
import json, sys
b = json.loads(sys.argv[1])
p = b["params"]
assert b["method"] == "tools/call" and p["name"] == "host.import" and p["arguments"] == {}, b
m = p["_meta"]
assert m["io.modelcontextprotocol/protocolVersion"] == sys.argv[2], m
assert m["io.modelcontextprotocol/clientCapabilities"] == {}, m
' "$body" "$PROTOCOL" || { echo "selftest: bad request body"; return 1; }
  sse=$'event: message\ndata: {"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"m","data":{"expected":[{"required":["run_id"]},{"required":["url"]}],"path":"/"}}}\n'
  out=$(printf '%s' "$sse" | reply_data)
  [ "$out" = '{"expected": [{"required": ["run_id"]}, {"required": ["url"]}], "path": "/"}' ] || { echo "selftest: bad error.data: $out"; return 1; }
  out=$(printf '%s' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"ok":true}}}' | reply_data)
  [ "$out" = '{"ok": true}' ] || { echo "selftest: bad result: $out"; return 1; }
  if printf '%s' 'not json' | reply_data >/dev/null 2>&1; then echo "selftest: accepted a non-reply"; return 1; fi
  echo "selftest: ok"
}

VERB="${1:-}"
if [ -z "$VERB" ] || [ $# -gt 1 ]; then
  echo "usage: empty-call.sh <verb> | --selftest" >&2
  exit 2
fi
for tool in python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "empty-call.sh: missing required tool: $tool" >&2; exit 3; }
done
if [ "$VERB" = "--selftest" ]; then
  selftest
  exit $?
fi
for tool in curl ssh; do
  command -v "$tool" >/dev/null 2>&1 || { echo "empty-call.sh: missing required tool: $tool" >&2; exit 3; }
done

# The key stays in KEY (never echoed, never on a command line: curl reads the
# header from stdin).
KEY=$(ssh "$SSH_HOST" "sudo grep -h '^MCPHOST_ADMIN_KEY=' '$ENV_FILE' | cut -d= -f2-" | tr -d '\r\n"') || KEY=""
if [ -z "$KEY" ]; then
  echo "empty-call.sh: could not read the operator key from $SSH_HOST:$ENV_FILE" >&2
  exit 1
fi

REPLY=$(printf 'header = "Authorization: Bearer %s"\n' "$KEY" | curl -sS --max-time 20 -X POST "$BASE/mcp" \
  -K - \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -H "mcp-protocol-version: $PROTOCOL" \
  -H 'mcp-method: tools/call' \
  -H "mcp-name: $VERB" \
  --data-binary "$(request_body "$VERB")") || { echo "empty-call.sh: $BASE unreachable" >&2; exit 1; }
KEY=""
printf '%s' "$REPLY" | reply_data || { echo "empty-call.sh: reply had neither error.data nor result" >&2; exit 1; }
