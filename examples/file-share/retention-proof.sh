#!/usr/bin/env bash
# retention-proof.sh -- runnable end-to-end proof of "Share a file that
# expires on its own" (PRD-mcphost-blob-retention-and-quota AC 8). Against
# $1 (or $MCPHOST_URL): sign up a fresh tenant, put a 300 KB blob with
# ttl_s 120, read data.quota from the put, and ask `host.blob.gc
# {dry_run: true, within_s: 300}` which must list exactly that blob as
# expiring. Prints `quota=ok expiring=1 gc_dry_run=ok` as its last line
# and exits 0 iff every check passed.
#
# Requires: bash, curl, python3. Deliberately has no jq dependency.

set -uo pipefail
cd "$(dirname "$0")"

if [[ $# -ge 1 && -n "${1:-}" ]]; then
  case "$1" in
    */mcp) MCPHOST_URL="$1" ;;
    *) MCPHOST_URL="${1%/}/mcp" ;;
  esac
fi
MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
SYNTHETIC_HEADER="${SYNTHETIC_HEADER:-retention-proof}"
FAILURES=0

check() {
  local desc="$1" ok="$2"
  if [[ "$ok" == "1" ]]; then
    echo "CHECK ${desc}: PASS"
  else
    echo "CHECK ${desc}: FAIL"
    FAILURES=$((FAILURES + 1))
  fi
}

mcp_call() {
  # mcp_call <tool> <args_json> [bearer_key] [extra_header_name] [extra_header_value]
  local tool="$1" args="$2" key="${3:-}" hname="${4:-}" hval="${5:-}"
  local body
  body=$(python3 - "$tool" "$args" <<'PY'
import json, sys
print(json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
        "name": sys.argv[1],
        "arguments": json.loads(sys.argv[2]),
        # SEP-2575: this host runs every request stateless (no
        # `initialize` handshake to remember client context from), so
        # every `tools/call` must carry it itself.
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {
                "name": "share-a-tool-proof",
                "version": "1.0.0",
            },
        },
    },
}))
PY
  )
  local -a hdrs=(
    -H "Content-Type: application/json"
    -H "Accept: application/json, text/event-stream"
    -H "MCP-Protocol-Version: 2026-07-28"
    -H "Mcp-Method: tools/call"
    -H "Mcp-Name: ${tool}"
  )
  [[ -n "$key" ]] && hdrs+=(-H "Authorization: Bearer ${key}")
  [[ -n "$hname" ]] && hdrs+=(-H "${hname}: ${hval}")
  local raw
  raw=$(curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" "${hdrs[@]}" -d "$body")
  # PRD-mcphost-one-next-tool requirement 3: a call that binds this
  # connection (signup) now emits notifications/tools/list_changed before
  # its own result, which upgrades that one response from a plain JSON
  # body to a text/event-stream one (one "data: <json>" line per message,
  # blocks separated by a blank line) -- normalize back to the plain
  # final JSON-RPC message here so every caller below (structured_of,
  # json_str, has_error) keeps reading a single JSON object either way.
  python3 - "$raw" <<'PY'
import sys
raw = sys.argv[1]
if raw.lstrip().startswith("data:"):
    last = None
    for block in raw.split("\n\n"):
        for line in block.splitlines():
            if line.startswith("data: "):
                last = line[len("data: "):]
            elif line.startswith("data:"):
                last = line[len("data:"):]
    print(last if last is not None else raw)
else:
    print(raw)
PY
}

structured_of() {
  python3 - "$1" <<'PY'
import json, sys
body = json.loads(sys.argv[1])
if "error" in body:
    print(json.dumps({"__error__": body["error"]}))
    sys.exit(0)
result = body.get("result") or {}
sc = result.get("structuredContent")
if sc is not None:
    print(json.dumps(sc))
    sys.exit(0)
content = result.get("content") or []
if content and isinstance(content[0], dict) and "text" in content[0]:
    print(content[0]["text"])
    sys.exit(0)
print("null")
PY
}

json_str() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
for p in [p for p in sys.argv[2].split(".") if p]:
    d = d.get(p) if isinstance(d, dict) else None
print(d if isinstance(d, str) else "")
PY
}

has_error() {
  python3 - "$1" <<'PY'
import json, sys
sys.exit(0 if "error" in json.loads(sys.argv[1]) else 1)
PY
}

# 1. A fresh tenant, tagged so this proof's own signup is distinguishable
#    from real recipe traffic.
resp=$(mcp_call "signup" '{"name": "file-share-proof"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
struct=$(structured_of "$resp")
KEY=$(json_str "$struct" "key")
check "signup returned a key" "$([[ -n "$KEY" ]] && echo 1 || echo 0)"

# 2. Put a 300 KB blob with ttl_s 120; the response carries data.quota.
CONTENT=$(python3 -c 'import base64, os; print(base64.b64encode(os.urandom(300 * 1024)).decode())')
put_resp=$(mcp_call "host.blob.put" "{\"name\": \"report.pdf\", \"mime\": \"application/pdf\", \"ttl_s\": 120, \"content_base64\": \"$CONTENT\"}" "$KEY")
put_struct=$(structured_of "$put_resp")
check "blob.put succeeded" "$(has_error "$put_struct" && echo 0 || echo 1)"
quota_ok=$(python3 - "$put_struct" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
q = (d.get("data") or {}).get("quota") or {}
m = q.get("blob_bytes_max") or {}
print(1 if all(k in m for k in ("used", "limit", "remaining")) and m["used"] > 0 else 0)
PY
)
check "data.quota.blob_bytes_max has used/limit/remaining" "$quota_ok"

# 3. gc dry run lists exactly the blob expiring within 300 s.
gc_resp=$(mcp_call "host.blob.gc" '{"dry_run": true, "within_s": 300}' "$KEY")
gc_struct=$(structured_of "$gc_resp")
expiring=$(python3 - "$gc_struct" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
e = d.get("expiring")
print(len(e) if isinstance(e, list) else (e if isinstance(e, int) else d.get("count", 0)))
PY
)
check "gc dry_run lists the expiring blob" "$([[ "$expiring" == "1" ]] && echo 1 || echo 0)"
check "gc dry_run answered without error" "$(has_error "$gc_struct" && echo 0 || echo 1)"

if [[ "$FAILURES" -eq 0 ]]; then
  echo "quota=ok expiring=${expiring} gc_dry_run=ok"
  exit 0
fi
echo "quota=$([[ "$quota_ok" == 1 ]] && echo ok || echo fail) expiring=${expiring} gc_dry_run=$(has_error "$gc_struct" && echo fail || echo ok) failures=${FAILURES}"
exit 1
