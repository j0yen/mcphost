#!/usr/bin/env bash
# publish-visibility.sh -- PRD-mcphost-publish-visibility-and-tags AC8.
#
# One fresh synthetic tenant publishes the quickstart's python starter with
# visibility "public" and tags ["pubvis-live"], expects tested_version 1 in the
# response, then expects <base_url>/.well-known/mcp/catalog.json to list a tool
# carrying that tag. The tenant is offboarded on EXIT. Needs bash, curl, python3.
#
# Usage: publish-visibility.sh <base_url>
# stdout (success): `status: ok` / `tested_version: 1` / `catalog_tag: pubvis-live`
# stdout (failure): `status: error reason: <step> <code>`
# exit: 0 ok, 1 failure, 2 usage, 3 missing tool.
set -euo pipefail

BASE="${1:-}"
BASE="${BASE%/}"
if [ -z "$BASE" ]; then
  echo "usage: publish-visibility.sh <base_url>" >&2
  exit 2
fi
for tool in curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "publish-visibility.sh: missing required tool: $tool" >&2
    exit 3
  fi
done

KEY=""

# rpc <step> <tenant_key|-> <tool> <args-json>: prints the tool's structured
# result on stdout; on any failure prints `<step> <code>` on stdout, returns 1.
rpc() {
  local step="$1" key="$2" tool="$3" args="$4" body resp
  body=$(python3 -c '
import json, sys
tool, args, key = sys.argv[1], json.loads(sys.argv[2]), sys.argv[3]
if key != "-":
    args["tenant_key"] = key
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "publish-visibility-live", "version": "1"}}}}))
' "$tool" "$args" "$key") || { echo "$step bad_request"; return 1; }
  resp=$(curl -sS --max-time 15 -X POST "$BASE/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'MCP-Protocol-Version: 2026-07-28' \
    -H 'Mcp-Method: tools/call' \
    -H "Mcp-Name: $tool" \
    --data-binary "$body" 2>/dev/null) || { echo "$step unreachable"; return 1; }
  printf '%s' "$resp" | python3 -c '
import json, sys
step = sys.argv[1]
raw = sys.stdin.read()
msgs = []
for line in raw.splitlines():
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
    print(step + " bad_response")
    sys.exit(1)
def code_of(d):
    data = d.get("data") if isinstance(d, dict) else None
    if isinstance(data, dict) and data.get("error_code"):
        return str(data["error_code"])
    return None
if "error" in msg:
    print(step + " " + (code_of(msg["error"]) or "rpc_error"))
    sys.exit(1)
res = msg["result"]
sc = res.get("structuredContent")
if sc is None:
    try:
        sc = json.loads(res["content"][0]["text"])
    except Exception:
        sc = {}
if res.get("isError"):
    err = sc.get("error_code") or sc.get("code") or sc.get("error") or "tool_error"
    print(step + " " + str(err).replace("\n", " "))
    sys.exit(1)
print(json.dumps(sc))
' "$step"
}

# field <json> <python-expr over d>: prints the value, empty when absent.
field() {
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

offboard_all() {
  [ -n "$KEY" ] || return 0
  rpc offboard "$KEY" host.self.offboard '{}' >/dev/null \
    || echo "publish-visibility.sh: offboard failed" >&2
  return 0
}

fail() {
  echo "status: error reason: $1"
  exit 1
}

trap 'offboard_all' EXIT

step_out=""
run() { step_out=$("$@") || fail "$step_out"; }

run rpc signup - signup '{"name":"pubvis-live","handoff":false}'
KEY=$(field "$step_out" 'd["key"]')
[ -n "$KEY" ] || fail "signup bad_response"

run rpc quickstart "$KEY" host.quickstart '{"kind":"python"}'
PUBLISH_ARGS=$(field "$step_out" 'json.dumps(dict(d["starter_tool"]["publish_call"]["arguments"], visibility="public", tags=["pubvis-live"]))')
[ -n "$PUBLISH_ARGS" ] || fail "quickstart bad_response"

run rpc publish "$KEY" host.tool_publish "$PUBLISH_ARGS"
TESTED=$(field "$step_out" 'd.get("tested_version")')
[ "$TESTED" = "1" ] || fail "publish tested_version_${TESTED:-missing}"

CATALOG=$(curl -sS --max-time 15 "$BASE/.well-known/mcp/catalog.json") || fail "catalog unreachable"
LISTED=$(printf '%s' "$CATALOG" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(any("pubvis-live" in (t.get("tags") or []) for t in d.get("tools", [])))
') || fail "catalog bad_response"
[ "$LISTED" = "True" ] || fail "catalog tag_not_listed"

echo "status: ok"
echo "tested_version: $TESTED"
echo "catalog_tag: pubvis-live"
