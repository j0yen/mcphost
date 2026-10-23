#!/usr/bin/env bash
# bound-call.sh -- PRD-mcphost-schema-bound-agreement AC7/AC8.
#
# Signs up ONE synthetic tenant through the anonymous `signup` tool (display
# name and MCP clientInfo name start `synthorg`), posts <verb> with <json> as
# its arguments to the prod /mcp with that tenant's key, prints the reply as
# ONE JSON object tagged `kind`, and offboards the tenant through
# host.self_offboard in an EXIT trap once a key exists. The key lives in one
# shell variable: it is never printed, never on a command line (curl reads the
# Authorization header from stdin). Any verb and argument set works.
#
# Usage: bound-call.sh <verb> '<json-arguments>'
#        bound-call.sh --selftest      offline: bodies, headers, reply parsing
# Env:   MCPHOST_BASE_URL (default https://mcphost.dev).
# stdout: {"kind":"error", ...error.data...}   a refusal; data.path/constraint/
#                                              expected read as an agent reads them
#         {"kind":"result", ...structuredContent...}
# e.g.   bound-call.sh host.agent.contacts_import '{"addresses":[]}' | jq -e '.path=="/addresses"'
# exit: 0 a reply was printed, 1 failure (no reply, or the offboard failed),
#       2 usage, 3 missing tool.
# shellcheck disable=SC2329  # finish runs via the EXIT trap
set -euo pipefail

BASE="${MCPHOST_BASE_URL:-https://mcphost.dev}"
BASE="${BASE%/}"
PROTOCOL="2026-07-28"
CLIENT_NAME="synthorg-bound-call"

KEY=""

# request_body <tool> <args-json>: the tools/call frame; clientInfo carries the
# synthetic `synthorg` name.
request_body() {
  python3 -c '
import json, sys
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": sys.argv[1], "arguments": json.loads(sys.argv[2]), "_meta": {
        "io.modelcontextprotocol/protocolVersion": sys.argv[3],
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": sys.argv[4], "version": "1"}}}}))
' "$1" "$2" "$PROTOCOL" "$CLIENT_NAME"
}

# reply_object: stdin is a JSON or SSE reply; prints one JSON object tagged
# `kind` (error: error.data, result: structuredContent), rc 1 when neither.
reply_object() {
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
    err = msg["error"] if isinstance(msg["error"], dict) else {}
    data = err.get("data")
    out = dict(data) if isinstance(data, dict) else {"message": err.get("message")}
    out["kind"] = "error"
else:
    res = msg["result"]
    sc = res.get("structuredContent") if isinstance(res, dict) else None
    if sc is None and isinstance(res, dict):
        try:
            sc = json.loads(res["content"][0]["text"])
        except Exception:
            sc = res
    out = dict(sc) if isinstance(sc, dict) else {"result": sc}
    out["kind"] = "error" if isinstance(res, dict) and res.get("isError") else "result"
print(json.dumps(out, sort_keys=True))
'
}

# post <key|-> <tool> <args-json>: prints the raw reply.
post() {
  local key="$1" tool="$2" args="$3" body
  body=$(request_body "$tool" "$args") || return 1
  if [ "$key" = "-" ]; then
    curl -sS --max-time 20 -X POST "$BASE/mcp" \
      -H 'content-type: application/json' \
      -H 'accept: application/json, text/event-stream' \
      -H "mcp-protocol-version: $PROTOCOL" \
      -H 'mcp-method: tools/call' \
      -H "mcp-name: $tool" \
      --data-binary "$body"
  else
    printf 'header = "Authorization: Bearer %s"\n' "$key" | curl -sS --max-time 20 -X POST "$BASE/mcp" \
      -K - \
      -H 'content-type: application/json' \
      -H 'accept: application/json, text/event-stream' \
      -H "mcp-protocol-version: $PROTOCOL" \
      -H 'mcp-method: tools/call' \
      -H "mcp-name: $tool" \
      --data-binary "$body"
  fi
}

selftest() {
  local body sse out
  body=$(request_body host.agent.contacts_import '{"addresses":[]}')
  python3 -c '
import json, sys
b = json.loads(sys.argv[1])
p = b["params"]
assert b["method"] == "tools/call" and p["name"] == "host.agent.contacts_import" and p["arguments"] == {"addresses": []}, b
m = p["_meta"]
assert m["io.modelcontextprotocol/protocolVersion"] == sys.argv[2], m
assert m["io.modelcontextprotocol/clientCapabilities"] == {}, m
assert m["io.modelcontextprotocol/clientInfo"]["name"].startswith("synthorg"), m
' "$body" "$PROTOCOL" || { echo "selftest: bad request body"; return 1; }
  sse=$'event: message\ndata: {"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"m","data":{"error_code":"args_invalid","path":"/addresses","constraint":"minItems"}}}\n'
  out=$(printf '%s' "$sse" | reply_object)
  [ "$out" = '{"constraint": "minItems", "error_code": "args_invalid", "kind": "error", "path": "/addresses"}' ] || { echo "selftest: bad error object: $out"; return 1; }
  out=$(printf '%s' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"imported":1}}}' | reply_object)
  [ "$out" = '{"imported": 1, "kind": "result"}' ] || { echo "selftest: bad result object: $out"; return 1; }
  out=$(printf '%s' '{"jsonrpc":"2.0","id":1,"result":{"isError":true,"structuredContent":{"error_code":"x"}}}' | reply_object)
  [ "$out" = '{"error_code": "x", "kind": "error"}' ] || { echo "selftest: bad isError object: $out"; return 1; }
  if printf '%s' 'not json' | reply_object >/dev/null 2>&1; then echo "selftest: accepted a non-reply"; return 1; fi
  echo "selftest: ok"
}

finish() {
  local rc=$?
  set +e
  if [ -n "$KEY" ]; then
    if post "$KEY" host.self_offboard '{}' 2>/dev/null | reply_object 2>/dev/null | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("kind") == "result" else 1)'; then
      echo "bound-call.sh: offboard ok" >&2
    else
      echo "bound-call.sh: offboard FAILED" >&2
      rc=1
    fi
    KEY=""
  fi
  exit "$rc"
}

if [ "${1:-}" = "--selftest" ] && [ $# -eq 1 ]; then
  command -v python3 >/dev/null 2>&1 || { echo "bound-call.sh: missing required tool: python3" >&2; exit 3; }
  selftest
  exit $?
fi
if [ $# -ne 2 ] || [ -z "$1" ]; then
  echo "usage: bound-call.sh <verb> '<json-arguments>' | --selftest" >&2
  exit 2
fi
VERB="$1"
ARGS="$2"
for tool in python3 curl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "bound-call.sh: missing required tool: $tool" >&2; exit 3; }
done
printf '%s' "$ARGS" | python3 -c 'import json, sys; assert isinstance(json.load(sys.stdin), dict)' 2>/dev/null \
  || { echo "bound-call.sh: arguments must be one JSON object" >&2; exit 2; }

trap 'finish' EXIT

SIGNUP=$(post - signup "{\"name\":\"synthorg-bound-call-$(date +%s)\",\"handoff\":false}" | reply_object) \
  || { echo "bound-call.sh: signup got no reply from $BASE" >&2; exit 1; }
KEY=$(printf '%s' "$SIGNUP" | python3 -c 'import json, sys; d = json.load(sys.stdin); print(d.get("key", "") if d.get("kind") == "result" else "")')
if [ -z "$KEY" ]; then
  echo "bound-call.sh: signup returned no key" >&2
  exit 1
fi

post "$KEY" "$VERB" "$ARGS" | reply_object || { echo "bound-call.sh: the call got no reply" >&2; exit 1; }
