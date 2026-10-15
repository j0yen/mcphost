#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Share a file" (PRD-mcphost-blob-put-get-signed-url
# part b AC 4). Against $1 (or $MCPHOST_URL): sign up a fresh tenant, put the
# fixture PDF, GET the signed url and compare sha256, then mint a 2 s url,
# wait, and expect 410. Prints `blob_put=ok url=200 sha256=match expired=410`
# as its last line and exits 0 iff every check passed.
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
SYNTHETIC_HEADER="${SYNTHETIC_HEADER:-file-share-proof}"
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


FIXTURE="../../tests/fixtures/blobput/report.pdf"

resp=$(mcp_call "signup" '{"name": "file-share-proof"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
KEY=$(json_str "$(structured_of "$resp")" "key")
check "signup returned a key" "$([[ -n "$KEY" ]] && echo 1 || echo 0)"

CONTENT=$(base64 -w0 "$FIXTURE")
WANT=$(sha256sum "$FIXTURE" | cut -d' ' -f1)
put_struct=$(structured_of "$(mcp_call "host.blob.put" "{\"name\": \"report.pdf\", \"content_base64\": \"$CONTENT\"}" "$KEY")")
put_ok=$(has_error "$put_struct" && echo 0 || echo 1)
check "blob.put succeeded" "$put_ok"
ID=$(json_str "$put_struct" "id")
URL=$(json_str "$put_struct" "url")
[[ -z "$URL" ]] && URL=$(json_str "$put_struct" "data.url")
[[ -z "$ID" ]] && ID=$(json_str "$put_struct" "data.id")

tmp=$(mktemp)
code=$(curl -sS -o "$tmp" -w '%{http_code}' --max-time 30 "$URL" 2>/dev/null || echo 000)
got=$(sha256sum "$tmp" | cut -d' ' -f1)
rm -f "$tmp"
check "url answers 200" "$([[ "$code" == "200" ]] && echo 1 || echo 0)"
check "sha256 matches the fixture" "$([[ "$got" == "$WANT" ]] && echo 1 || echo 0)"

short_struct=$(structured_of "$(mcp_call "host.blob.url" "{\"id\": \"$ID\", \"ttl_s\": 2}" "$KEY")")
SHORT=$(json_str "$short_struct" "url")
[[ -z "$SHORT" ]] && SHORT=$(json_str "$short_struct" "data.url")
sleep 4
exp=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 "$SHORT" 2>/dev/null || echo 000)
check "expired url answers 410" "$([[ "$exp" == "410" ]] && echo 1 || echo 0)"

sha_word=$([[ "$got" == "$WANT" ]] && echo match || echo mismatch)
put_word=$([[ "$put_ok" == 1 ]] && echo ok || echo fail)
echo "blob_put=${put_word} url=${code} sha256=${sha_word} expired=${exp}"
[[ "$FAILURES" -eq 0 ]]
