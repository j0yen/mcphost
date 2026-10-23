#!/usr/bin/env bash
# task-handoff-live.sh -- PRD-mcphost-task-handoff-live-script.
#
# Two implicit signups, then post -> wait -> claim -> complete -> wait over
# stateless JSON-RPC `tools/call` against <base_url>/mcp, and the latency from
# just before `host.task.post` to the poster's final `host.msg.wait` return.
# Needs only bash, curl and python3. Both tenants are offboarded on EXIT.
#
# Usage: task-handoff-live.sh [--json] <base_url>
# stdout (success): `status: done` / `latency_ms: N`   (or one JSON object)
# stdout (failure): `status: error reason: <step> <code>`
# exit: 0 done and under 3000 ms, 1 failure, 2 usage, 3 missing tool.
set -euo pipefail

MAX_LATENCY_MS=3000
JSON=0
BASE=""
for arg in "$@"; do
  case "$arg" in
    --json) JSON=1 ;;
    *) BASE="${arg%/}" ;;
  esac
done
if [ -z "$BASE" ]; then
  echo "usage: task-handoff-live.sh [--json] <base_url>" >&2
  exit 2
fi
for tool in curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "task-handoff-live.sh: missing required tool: $tool" >&2
    exit 3
  fi
done

KEY_A=""
KEY_B=""

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
        "io.modelcontextprotocol/clientInfo": {"name": "task-handoff-live", "version": "1"}}}}))
' "$tool" "$args" "$key") || { echo "$step bad_request"; return 1; }
  # Retry only when the TCP connect itself failed (curl exit 7): nothing was
  # sent, so a retry cannot duplicate a signup or a post. A loaded host can
  # refuse or drop a connect for a moment.
  local attempt rc=0
  for attempt in 1 2 3 4 5; do
    rc=0
    resp=$(curl -sS --max-time 30 -X POST "$BASE/mcp" \
      -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      -H 'MCP-Protocol-Version: 2026-07-28' \
      -H 'Mcp-Method: tools/call' \
      -H "Mcp-Name: $tool" \
      -H 'x-mcphost-synthetic: script:task-handoff-live' \
      --data-binary "$body" 2>/dev/null) || rc=$?
    [ "$rc" -eq 7 ] || break
    python3 -c 'import time; time.sleep(1)'
  done
  [ "$rc" -eq 0 ] || { echo "$step unreachable"; return 1; }
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
  local key
  for key in "$KEY_A" "$KEY_B"; do
    [ -n "$key" ] || continue
    rpc offboard "$key" host.self.offboard '{}' >/dev/null \
      || echo "task-handoff-live.sh: offboard failed for a tenant" >&2
  done
  return 0
}

fail() {
  echo "status: error reason: $1"
  exit 1
}

# Offboard runs after the result lines are printed, keeps the exit code.
trap 'offboard_all' EXIT

step_out=""
run() { step_out=$("$@") || fail "$step_out"; }

run rpc signup_a - signup '{"name":"handoff-live-a","handoff":false}'
KEY_A=$(field "$step_out" 'd["key"]')
run rpc signup_b - signup '{"name":"handoff-live-b","handoff":false}'
KEY_B=$(field "$step_out" 'd["key"]')
NS_B=$(field "$step_out" 'd["tenant"]')
[ -n "$KEY_A" ] && [ -n "$KEY_B" ] && [ -n "$NS_B" ] || fail "signup bad_response"

run rpc inbox "$KEY_A" host.msg.inbox '{}'
CURSOR=$(field "$step_out" 'd.get("next_cursor")')

T0=$EPOCHREALTIME
run rpc post "$KEY_A" host.task.post "{\"to\":\"$NS_B\",\"payload\":{\"n\":1},\"deadline_s\":120}"
TASK_ID=$(field "$step_out" 'd["task_id"]')
THREAD_ID=$(field "$step_out" 'd["thread_id"]')
[ -n "$TASK_ID" ] || fail "post bad_response"

run rpc wait_b "$KEY_B" host.msg.wait '{"timeout_s":10}'
GOT=$(field "$step_out" 'any(m.get("data", {}).get("task", {}).get("task_id") == "'"$TASK_ID"'" for m in d.get("messages", []))')
[ "$GOT" = "True" ] || fail "wait_b task_not_delivered"

run rpc claim "$KEY_B" host.task.claim "{\"task_id\":\"$TASK_ID\"}"
run rpc complete "$KEY_B" host.task.complete "{\"task_id\":\"$TASK_ID\",\"result\":{\"n\":1}}"

if [ -n "$CURSOR" ]; then
  WAIT_ARGS="{\"cursor\":\"$CURSOR\",\"timeout_s\":10}"
else
  WAIT_ARGS='{"timeout_s":10}'
fi
run rpc wait_a "$KEY_A" host.msg.wait "$WAIT_ARGS"
DONE=$(field "$step_out" 'any(m.get("data", {}).get("task", {}).get("status") == "done" and m["data"]["task"].get("result", {}).get("n") == 1 for m in d.get("messages", []))')
T1=$EPOCHREALTIME
[ "$DONE" = "True" ] || fail "wait_a task_not_done"

# EPOCHREALTIME is seconds.microseconds; strip the dot for integer math.
LATENCY_MS=$(( (${T1/./} - ${T0/./}) / 1000 ))
if [ "$LATENCY_MS" -ge "$MAX_LATENCY_MS" ]; then
  fail "latency ${LATENCY_MS}ms_over_${MAX_LATENCY_MS}ms"
fi

if [ "$JSON" -eq 1 ]; then
  printf '{"status":"done","latency_ms":%s,"task_id":"%s","thread_id":"%s"}\n' \
    "$LATENCY_MS" "$TASK_ID" "$THREAD_ID"
else
  echo "status: done"
  echo "latency_ms: $LATENCY_MS"
fi
