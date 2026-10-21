#!/usr/bin/env bash
# GENERATED from recipes/webhook-inbox.toml by `cargo xtask recipes` -- do not hand-edit.
# Recipe calls (budget: 4 calls, within 90 s):
#   1. host.webhook_inbox.create
#   2. signed POST to the returned url
#   3. signed POST to the returned url
#   4. host.tool_call
#   5. host.msg.wait
# proof.sh -- runnable end-to-end proof of "Webhook inbox in one call"
# (see www/llms.txt). One fresh tenant against $MCPHOST_URL: one
# host.webhook_inbox.create call, one signed curl to the returned url, then
# host.tool_call and host.msg.wait both show the event.
#
# Requires: bash, curl, python3. Deliberately has no jq or openssl dependency.
#
# Env vars:
#   MCPHOST_URL     the endpoint to run the recipe against.
#                   Default: https://mcphost.dev/mcp
#   HOOK_BASE_URL   rewrite the returned url's scheme://host:port to this
#                   (e.g. when the server advertises a public https name but
#                   is reached on http://127.0.0.1:PORT). Default: unchanged.
#
# Exit code 0 iff every check below passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
# MCPHOST_PROVE_DRY=1: print the plan and exit 0 before any network call (the runner-contract test sets it).
[[ -n "${MCPHOST_PROVE_DRY:-}" ]] && { echo "plan: webhook-inbox proof against $MCPHOST_URL"; exit 0; }
HOOK_BASE_URL="${HOOK_BASE_URL:-}"
FAILURES=0
EVENT_ID="evt-fixture-1"

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
  # mcp_call <tool> <args_json> [bearer_key]
  local tool="$1" args="$2" key="${3:-}"
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
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {
                "name": "webhook-inbox-proof",
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
  local raw
  raw=$(curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" "${hdrs[@]}" -d "$body")
  # A call that binds the connection (signup) may answer as a
  # text/event-stream; normalize to the final JSON-RPC message.
  python3 - "$raw" <<'PYEOF'
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
PYEOF
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

# json_get <json> <dotted.path> -- prints the string/number at that path, or empty.
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
for p in [p for p in sys.argv[2].split(".") if p]:
    d = d.get(p) if isinstance(d, dict) else None
print(d if isinstance(d, (str, int, float)) else "")
PY
}

has_error() {
  python3 - "$1" <<'PY'
import json, sys
sys.exit(0 if "error" in json.loads(sys.argv[1]) else 1)
PY
}

# ---- the recipe -------------------------------------------------------------

# 1. One fresh tenant.
signup_resp=$(mcp_call "signup" '{"name": "webhook-inbox-proof"}')
signup_struct=$(structured_of "$signup_resp")
NS=$(json_get "$signup_struct" "tenant")
KEY=$(json_get "$signup_struct" "key")
check "tenant_signed_up" "$([[ -n "$KEY" ]] && echo 1 || echo 0)"

# 2. The one call.
create_resp=$(mcp_call "host.webhook_inbox.create" '{"name": "inbox"}' "$KEY")
create=$(structured_of "$create_resp")
URL=$(json_get "$create" "url")
SECRET=$(json_get "$create" "secret")
WAIT_ARGS=$(python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1]).get("wait", {}).get("args", {})))' "$create")
check "create_returns_url" "$([[ "$URL" == */hook/* ]] && echo 1 || echo 0)"
check "create_returns_secret" "$([[ -n "$SECRET" ]] && echo 1 || echo 0)"
check "create_returns_wait_tool" "$([[ "$(json_get "$create" "wait.tool")" == "host.msg.wait" ]] && echo 1 || echo 0)"

if [[ -n "$HOOK_BASE_URL" && -n "$URL" ]]; then
  URL="${HOOK_BASE_URL%/}/hook/${URL##*/hook/}"
fi

# 3. One signed POST, exactly as a sender would.
BODY="{\"id\":\"${EVENT_ID}\"}"
SIG=$(python3 -c 'import hashlib,hmac,sys; print(hmac.new(sys.argv[1].encode(), sys.argv[2].encode(), hashlib.sha256).hexdigest())' "$SECRET" "$BODY")
post_code=$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 30 -X POST "$URL" \
  -H "Content-Type: application/json" -H "X-Mcphost-Signature: sha256=${SIG}" -d "$BODY")
check "signed_post_accepted" "$([[ "$post_code" == "200" ]] && echo 1 || echo 0)"

bad_code=$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 30 -X POST "$URL" \
  -H "Content-Type: application/json" -d "$BODY")
check "unsigned_post_is_401" "$([[ "$bad_code" == "401" ]] && echo 1 || echo 0)"

# 4. The event, by tool call (the python reader's first call may still be
#    building its environment; retry while it says so).
FOUND=0
for _ in $(seq 1 100); do
  call_resp=$(mcp_call "host.tool_call" '{"name": "inbox"}' "$KEY")
  if has_error "$call_resp"; then
    sleep 0.2
    continue
  fi
  call=$(structured_of "$call_resp")
  FOUND=$(python3 - "$call" "$EVENT_ID" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
rows = d.get("rows") or (d.get("result") or {}).get("rows") or []
print(1 if any(r.get("id") == sys.argv[2] for r in rows) else 0)
PY
  )
  [[ "$FOUND" == "1" ]] && break
  sleep 0.2
done
check "tool_call_returns_the_event_row" "$FOUND"

# 5. The event, by host.msg.wait with the returned wait.args.
wait_resp=$(mcp_call "host.msg.wait" "$WAIT_ARGS" "$KEY")
wait_struct=$(structured_of "$wait_resp")
GOT_EVENT=$(python3 - "$wait_struct" "$EVENT_ID" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
msgs = d.get("messages") or []
print(1 if any((m.get("data") or {}).get("event_id") == sys.argv[2] for m in msgs) else 0)
PY
)
check "msg_wait_returns_the_event" "$GOT_EVENT"

if [[ "$FAILURES" -eq 0 ]]; then
  echo "RESULT: PASS"
  exit 0
else
  echo "RESULT: FAIL (${FAILURES} check(s) failed)"
  exit 1
fi
