#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "First run" (see www/llms.txt).
# One fresh tenant against $MCPHOST_URL: signs up, publishes `text_stats`
# as a python tool (reverses text, counts words -- no secrets, no network,
# no deps, publishes on the free plan), calls it with a known input, and
# asserts the real output.
#
# Requires: bash, curl, python3. Deliberately has no jq dependency.
#
# Env vars:
#   MCPHOST_URL   the endpoint to run the recipe against.
#                 Default: https://mcphost.dev/mcp
#
# Exit code 0 iff every check below passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
FAILURES=0
START_EPOCH=$(python3 -c 'import time; print(time.time())')

check() {
  local desc="$1" ok="$2"
  if [[ "$ok" == "1" ]]; then
    echo "CHECK ${desc}: PASS"
  else
    echo "CHECK ${desc}: FAIL"
    FAILURES=$((FAILURES + 1))
  fi
}

# ---- MCP JSON-RPC helpers (curl + python3) ---------------------------------

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
                "name": "first-run-proof",
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
  curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" "${hdrs[@]}" -d "$body"
}

# error_code_of <raw_json> -- prints error.data.error_code, or empty.
error_code_of() {
  python3 - "$1" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
err = d.get("error") or {}
print((err.get("data") or {}).get("error_code", ""))
PY
}

# mcp_call_ready <tool> <args_json> <key> -- retries while the tool's
# sandboxed environment is still building (`tool_building`), same "cold
# python tool" wait every sandboxed test in this repo needs (see
# `poll_until_ready` in tests/common/mod.rs).
mcp_call_ready() {
  local tool="$1" args="$2" key="$3"
  local resp
  for _ in $(seq 1 200); do
    resp=$(mcp_call "$tool" "$args" "$key")
    [[ "$(error_code_of "$resp")" != "tool_building" ]] && { echo "$resp"; return; }
    sleep 0.1
  done
  echo "$resp"
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
  # json_str <json> <dotted.path> -- prints the raw string at that path, or
  # empty if missing/not-a-string.
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

# ---- the recipe -------------------------------------------------------------

SYNTHETIC_HEADER="recipe:first-run"

# 1. Fresh tenant, tagged so this proof's own signup is distinguishable
#    from real recipe traffic.
owner_resp=$(mcp_call "signup" '{"name": "first-run-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_str "$owner_struct" "tenant")
OWNER_KEY=$(json_str "$owner_struct" "key")
check "owner_signed_up" "$([[ -n "$OWNER_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"

# 2. Publish text_stats as a python tool -- requirement 1, AC1: no secrets,
#    no network, no deps, publishes on the free plan.
SRC=$(cat tools/text_stats.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"text_stats","kind":"python","spec":{"source": sys.argv[1]}}))' "$SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$OWNER_KEY")
check "publish_succeeds" "$(has_error "$publish_resp" && echo 0 || echo 1)"
TOOL_KIND=$(json_str "$(structured_of "$publish_resp")" "kind")
check "published_kind_is_python" "$([[ "$TOOL_KIND" == "python" ]] && echo 1 || echo 0)"
echo "TOOL_KIND=${TOOL_KIND}"

# 3. Call it -- AC1: reversing "hello" returns a real answer, not an echo.
call_resp=$(mcp_call_ready "host.tool_test" '{"name": "text_stats", "args": {"text": "hello"}}' "$OWNER_KEY")
check "call_succeeds" "$(has_error "$call_resp" && echo 0 || echo 1)"
call_struct=$(structured_of "$call_resp")
REVERSED=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print((d.get("result") or {}).get("reversed",""))' "$call_struct")
WORDS=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print((d.get("result") or {}).get("words",""))' "$call_struct")
check "reversed_is_olleh" "$([[ "$REVERSED" == "olleh" ]] && echo 1 || echo 0)"
check "words_is_1" "$([[ "$WORDS" == "1" ]] && echo 1 || echo 0)"
echo "REVERSED=${REVERSED} WORDS=${WORDS}"

END_EPOCH=$(python3 -c 'import time; print(time.time())')
WALL_MS=$(python3 -c "print(int((${END_EPOCH} - ${START_EPOCH}) * 1000))")
echo "WALL_TIME_MS=${WALL_MS}"

if [[ "$FAILURES" -eq 0 ]]; then
  echo "RESULT: PASS"
  exit 0
else
  echo "RESULT: FAIL (${FAILURES} check(s) failed)"
  exit 1
fi
