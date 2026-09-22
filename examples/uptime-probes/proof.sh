#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Uptime probes with no server"
# (see www/llms.txt). One fresh tenant against $MCPHOST_URL: two tables,
# two targets (one healthy URL, one that returns 503), `probe` published
# and scheduled, `host.trigger.fire` twice instead of waiting 5 minutes,
# asserting two `checks` rows per target and that `status` marks the 503
# target down with a `down_since` while the healthy one shows 100%.
#
# Requires: bash, curl, python3. Deliberately has no jq dependency.
#
# Env vars:
#   MCPHOST_URL     the endpoint to run the recipe against.
#                   Default: https://mcphost.dev/mcp
#   HEALTHY_URL     a URL that returns 2xx. Default: https://httpbin.org/status/200
#   DOWN_URL        a URL that returns a non-2xx status. Default: https://httpbin.org/status/503
#
# Exit code 0 iff every check below passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
HEALTHY_URL="${HEALTHY_URL:-https://httpbin.org/status/200}"
DOWN_URL="${DOWN_URL:-https://httpbin.org/status/503}"
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
                "name": "uptime-probes-proof",
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

# wait_for_run <run_id> <key> -- polls host.runs.get until the run leaves
# queued/running, printing its final structured JSON.
wait_for_run() {
  local run_id="$1" key="$2"
  local args resp status
  args=$(python3 -c 'import json,sys; print(json.dumps({"run_id": sys.argv[1]}))' "$run_id")
  for _ in $(seq 1 200); do
    resp=$(mcp_call "host.runs.get" "$args" "$key")
    status=$(json_str "$(structured_of "$resp")" "status")
    if [[ "$status" != "queued" && "$status" != "running" ]]; then
      structured_of "$resp"
      return
    fi
    sleep 0.1
  done
  structured_of "$resp"
}

# ---- the recipe -------------------------------------------------------------

SYNTHETIC_HEADER="recipe:uptime-probes"

# 1. One fresh tenant, tagged so this proof's own signup is distinguishable
#    from real recipe traffic (requirement 5).
signup_resp=$(mcp_call "signup" '{"name": "uptime-probes-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
signup_struct=$(structured_of "$signup_resp")
NS=$(json_str "$signup_struct" "tenant")
KEY=$(json_str "$signup_struct" "key")
check "tenant_signed_up" "$([[ -n "$KEY" ]] && echo 1 || echo 0)"
echo "NS=${NS}"

# 2. The two tables.
targets_resp=$(mcp_call "host.state.table_create" '{"name": "targets", "schema": {"url": "text", "added_at": "real"}, "primary_key": "url"}' "$KEY")
check "targets_table_create" "$(has_error "$targets_resp" && echo 0 || echo 1)"

checks_resp=$(mcp_call "host.state.table_create" '{"name": "checks", "schema": {"url": "text", "code": "integer", "ms": "real", "at": "real"}}' "$KEY")
check "checks_table_create" "$(has_error "$checks_resp" && echo 0 || echo 1)"

# 3. Two targets: one healthy, one that returns 503.
insert_args=$(python3 -c 'import json,sys,time; print(json.dumps({"table": "targets", "rows": [{"url": sys.argv[1], "added_at": time.time()}, {"url": sys.argv[2], "added_at": time.time()}]}))' "$HEALTHY_URL" "$DOWN_URL")
insert_resp=$(mcp_call "host.state.insert" "$insert_args" "$KEY")
check "targets_inserted" "$(has_error "$insert_resp" && echo 0 || echo 1)"

# 4. Publish probe (python, network: public) and status (python).
PROBE_SRC=$(cat tools/probe.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"probe","kind":"python","spec":{"source": sys.argv[1], "network": "public"}}))' "$PROBE_SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$KEY")
check "publish_probe" "$(has_error "$publish_resp" && echo 0 || echo 1)"

STATUS_SRC=$(cat tools/status.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"status","kind":"python","spec":{"source": sys.argv[1]}}))' "$STATUS_SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$KEY")
check "publish_status" "$(has_error "$publish_resp" && echo 0 || echo 1)"

# 5. Schedule probe every 5 minutes (the free plan's floor), then fire it
#    twice instead of waiting 10 minutes -- a real user just waits.
set_resp=$(mcp_call "host.trigger.set" '{"tool": "probe", "kind": "schedule", "schedule": "*/5 * * * *"}' "$KEY")
check "trigger_set" "$(has_error "$set_resp" && echo 0 || echo 1)"
TRIGGER_ID=$(json_str "$(structured_of "$set_resp")" "id")

fire_args=$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1]}))' "$TRIGGER_ID")
for i in 1 2; do
  fire_resp=$(mcp_call "host.trigger.fire" "$fire_args" "$KEY")
  run_id=$(json_str "$(structured_of "$fire_resp")" "run_id")
  run_struct=$(wait_for_run "$run_id" "$KEY")
  run_status=$(json_str "$run_struct" "status")
  check "probe_fire_${i}_succeeds" "$([[ "$run_status" == "done" ]] && echo 1 || echo 0)"
done

# 6. Two checks rows per target -- AC2.
healthy_query_args=$(python3 -c 'import json,sys; print(json.dumps({"table": "checks", "where": "url = \x27%s\x27" % sys.argv[1]}))' "$HEALTHY_URL")
healthy_checks=$(structured_of "$(mcp_call "host.state.query" "$healthy_query_args" "$KEY")")
healthy_row_count=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$healthy_checks")
check "healthy_target_has_two_checks" "$([[ "$healthy_row_count" == "2" ]] && echo 1 || echo 0)"

down_query_args=$(python3 -c 'import json,sys; print(json.dumps({"table": "checks", "where": "url = \x27%s\x27" % sys.argv[1], "order_by": "at asc"}))' "$DOWN_URL")
down_checks=$(structured_of "$(mcp_call "host.state.query" "$down_query_args" "$KEY")")
down_row_count=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$down_checks")
check "down_target_has_two_checks" "$([[ "$down_row_count" == "2" ]] && echo 1 || echo 0)"
FIRST_DOWN_AT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("at","") if rows else "")' "$down_checks")

# 7. status marks the 503 target down with down_since == its first check;
#    the healthy target is 100% up with no down_since -- AC3.
status_resp=$(mcp_call "${NS}.status" '{}' "$KEY")
status_struct=$(structured_of "$status_resp")
DOWN_UP_PCT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); t=[x for x in d.get("targets") or [] if x.get("url")==sys.argv[2]]; print(t[0].get("up_pct_24h","") if t else "")' "$status_struct" "$DOWN_URL")
check "down_target_up_pct_is_0" "$([[ "$DOWN_UP_PCT" == "0" ]] && echo 1 || echo 0)"
DOWN_SINCE=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); t=[x for x in d.get("targets") or [] if x.get("url")==sys.argv[2]]; print(t[0].get("down_since","") if t else "")' "$status_struct" "$DOWN_URL")
check "down_target_down_since_matches_first_check" "$([[ -n "$DOWN_SINCE" && "$DOWN_SINCE" == "$FIRST_DOWN_AT" ]] && echo 1 || echo 0)"

HEALTHY_UP_PCT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); t=[x for x in d.get("targets") or [] if x.get("url")==sys.argv[2]]; print(t[0].get("up_pct_24h","") if t else "")' "$status_struct" "$HEALTHY_URL")
check "healthy_target_up_pct_is_100" "$([[ "$HEALTHY_UP_PCT" == "100" ]] && echo 1 || echo 0)"
HEALTHY_HAS_DOWN_SINCE=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); t=[x for x in d.get("targets") or [] if x.get("url")==sys.argv[2]]; print(1 if (t and "down_since" in t[0]) else 0)' "$status_struct" "$HEALTHY_URL")
check "healthy_target_has_no_down_since" "$([[ "$HEALTHY_HAS_DOWN_SINCE" == "0" ]] && echo 1 || echo 0)"

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
