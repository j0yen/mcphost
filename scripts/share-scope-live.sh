#!/usr/bin/env bash
# share-scope-live.sh -- PRD-mcphost-scoped-tool-share AC8.
#
# Two implicit signups (owner A, grantee B). A publishes a trivial tool and
# shares it to B through a group with scopes ["call"]; B calls it; A unshares;
# B calls again until it is refused `share_revoked`, and the milliseconds from
# the unshare's return to that refusal are measured. Stateless JSON-RPC
# `tools/call` against <base_url>/mcp; needs only bash, curl and python3. Both
# tenants are offboarded on EXIT and both keys are then proven dead.
#
# Usage: share-scope-live.sh [--json] <base_url>
# stdout (success): `call: ok` / `revoked_after_ms: N` / `code: share_revoked`
#                   / `tenants_left: 0`   (or one JSON object)
# stdout (failure): `status: error reason: <step> <code>` / `tenants_left: N`
# exit: 0 ok and revoked in under 1000 ms, 1 failure, 2 usage, 3 missing tool.
# shellcheck disable=SC2329  # finish and key_is_dead run via the EXIT trap
set -euo pipefail

MAX_REVOKE_MS=1000
POLL_DEADLINE_MS=5000
JSON=0
BASE=""
for arg in "$@"; do
  case "$arg" in
    --json) JSON=1 ;;
    *) BASE="${arg%/}" ;;
  esac
done
if [ -z "$BASE" ]; then
  echo "usage: share-scope-live.sh [--json] <base_url>" >&2
  exit 2
fi
for tool in curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "share-scope-live.sh: missing required tool: $tool" >&2
    exit 3
  fi
done

KEY_A=""
KEY_B=""
RESULT_MS=""

# rpc <step> <tenant_key|-> <tool> <args-json>: prints the tool's structured
# result on stdout; on any failure prints `<step> <code>` on stdout, returns 1.
# The code is always a bare [a-z0-9_] token, so a failure line carries no
# server text and no key material.
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
        "io.modelcontextprotocol/clientInfo": {"name": "share-scope-live", "version": "1"}}}}))
' "$tool" "$args" "$key") || { echo "$step bad_request"; return 1; }
  resp=$(curl -sS --max-time 15 -X POST "$BASE/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'MCP-Protocol-Version: 2026-07-28' \
    -H 'Mcp-Method: tools/call' \
    -H "Mcp-Name: $tool" \
    -H 'x-mcphost-synthetic: script:share-scope-live' \
    --data-binary "$body" 2>/dev/null) || { echo "$step unreachable"; return 1; }
  printf '%s' "$resp" | python3 -c '
import json, re, sys
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
def clean(code, default):
    code = str(code)
    return code if re.fullmatch(r"[a-z0-9_]{1,64}", code) else default
def code_of(d):
    data = d.get("data") if isinstance(d, dict) else None
    if isinstance(data, dict) and data.get("error_code"):
        return clean(data["error_code"], "rpc_error")
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
    print(step + " " + clean(err, "tool_error"))
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

# now_ms: integer milliseconds from EPOCHREALTIME (seconds.microseconds).
now_ms() {
  local t=$EPOCHREALTIME
  echo $(( ${t/./} / 1000 ))
}

fail() {
  echo "status: error reason: $1"
  exit 1
}

# key_is_dead <key>: a host.whoami with an offboarded key must be refused
# tenant_disabled / tenant_key_invalid; anything else counts as still alive.
key_is_dead() {
  local out
  out=$(rpc whoami "$1" host.whoami '{}') && return 1
  case "$out" in
    "whoami tenant_disabled"|"whoami tenant_key_invalid") return 0 ;;
    *) return 1 ;;
  esac
}

# Runs after the result is decided: offboards both tenants, proves both keys
# dead, prints the result lines, keeps (or raises) the exit code.
finish() {
  local rc=$? key left=0
  set +e
  for key in "$KEY_A" "$KEY_B"; do
    [ -n "$key" ] || continue
    rpc offboard "$key" host.self.offboard '{}' >/dev/null \
      || echo "share-scope-live.sh: offboard failed for a tenant" >&2
    key_is_dead "$key" || left=$((left + 1))
  done
  if [ "$rc" -eq 0 ]; then
    if [ "$left" -ne 0 ]; then
      echo "status: error reason: offboard tenants_left_${left}"
      rc=1
    elif [ "$JSON" -eq 1 ]; then
      printf '{"call":"ok","revoked_after_ms":%s,"code":"share_revoked","tenants_left":0}\n' "$RESULT_MS"
    else
      echo "call: ok"
      echo "revoked_after_ms: $RESULT_MS"
      echo "code: share_revoked"
      echo "tenants_left: 0"
    fi
  else
    echo "tenants_left: $left"
    [ "$left" -eq 0 ] || rc=1
  fi
  exit "$rc"
}
trap 'finish' EXIT

step_out=""
run() { step_out=$("$@") || fail "$step_out"; }

STAMP=$(date +%s)
run rpc signup_a - signup "{\"name\":\"share-scope-live-${STAMP}-a\",\"handoff\":false}"
KEY_A=$(field "$step_out" 'd["key"]')
NS_A=$(field "$step_out" 'd["tenant"]')
run rpc signup_b - signup "{\"name\":\"share-scope-live-${STAMP}-b\",\"handoff\":false}"
KEY_B=$(field "$step_out" 'd["key"]')
NS_B=$(field "$step_out" 'd["tenant"]')
[ -n "$KEY_A" ] && [ -n "$KEY_B" ] && [ -n "$NS_A" ] && [ -n "$NS_B" ] || fail "signup bad_response"

run rpc publish "$KEY_A" host.tool.publish '{"name":"ping","kind":"echo","spec":{"schema":{"type":"object"}}}'
run rpc group_create "$KEY_A" host.group.create '{"name":"crew"}'
run rpc group_add "$KEY_A" host.group.add "{\"name\":\"crew\",\"namespace\":\"$NS_B\"}"
run rpc share "$KEY_A" host.tool.share '{"name":"ping","visibility":"group","group":"crew","scopes":["call"]}'

TOOL="${NS_A}.ping"
run rpc call_b "$KEY_B" "$TOOL" '{}'

run rpc unshare "$KEY_A" host.tool.unshare '{"name":"ping"}'
T0=$(now_ms)
CODE=""
while :; do
  if out=$(rpc call_b_again "$KEY_B" "$TOOL" '{}'); then
    CODE="call_succeeded"
  else
    CODE="${out#call_b_again }"
    break
  fi
  if [ $(( $(now_ms) - T0 )) -ge "$POLL_DEADLINE_MS" ]; then
    fail "revoke still_callable_after_${POLL_DEADLINE_MS}ms"
  fi
  sleep 0.05
done
RESULT_MS=$(( $(now_ms) - T0 ))
[ "$CODE" = "share_revoked" ] || fail "revoke code_${CODE}"
if [ "$RESULT_MS" -ge "$MAX_REVOKE_MS" ]; then
  fail "revoke ${RESULT_MS}ms_over_${MAX_REVOKE_MS}ms"
fi
exit 0
