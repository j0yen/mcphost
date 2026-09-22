#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Give Claude a database in one
# minute" (see www/llms.txt). One fresh tenant against $MCPHOST_URL:
# creates the `expenses` table with `host.state.table_create`, loads
# fixture.csv (1,000 rows) in batches of 200 with `host.state.insert`,
# publishes `query` as a python tool, asks an equality/range/count
# question with known fixture answers, and confirms every row is present
# via a raw `host.state.query`.
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
                "name": "database-in-a-minute-proof",
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
  for _ in $(seq 1 100); do
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

SYNTHETIC_HEADER="recipe:database-in-a-minute"
BATCH_SIZE=200

# 1. Fresh tenant, tagged so this proof's own signup is distinguishable
#    from real recipe traffic (requirement 4 / AC5).
owner_resp=$(mcp_call "signup" '{"name": "database-in-a-minute-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_str "$owner_struct" "tenant")
OWNER_KEY=$(json_str "$owner_struct" "key")
check "owner_signed_up" "$([[ -n "$OWNER_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"

# 2. Create the table -- requirement 1.
table_resp=$(mcp_call "host.state.table_create" '{"name": "expenses", "schema": {"id": "integer", "category": "text", "amount": "real", "day": "integer"}, "primary_key": "id"}' "$OWNER_KEY")
check "table_create" "$(has_error "$table_resp" && echo 0 || echo 1)"

# 3. Batched insert of the 1,000-row fixture -- requirement 1/2, AC2.
INSERT_FAILURES=0
for offset in 0 200 400 600 800; do
  batch_args=$(python3 - "fixture.csv" "$offset" "$BATCH_SIZE" <<'PY'
import csv, json, sys

path, offset, size = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
with open(path, newline="") as f:
    rows = list(csv.DictReader(f))
batch = rows[offset:offset + size]
out = [
    {"id": int(r["id"]), "category": r["category"], "amount": float(r["amount"]), "day": int(r["day"])}
    for r in batch
]
print(json.dumps({"table": "expenses", "rows": out}))
PY
  )
  batch_resp=$(mcp_call "host.state.insert" "$batch_args" "$OWNER_KEY")
  if has_error "$batch_resp"; then
    INSERT_FAILURES=$((INSERT_FAILURES + 1))
    echo "INSERT_BATCH_OFFSET_${offset}_ERROR=${batch_resp}"
  fi
done
check "batched_insert_five_calls_of_200_succeed" "$([[ "$INSERT_FAILURES" -eq 0 ]] && echo 1 || echo 0)"

# 4. All rows present -- AC2: a raw host.state.query count = 1000.
raw_query_resp=$(mcp_call "host.state.query" '{"table": "expenses"}' "$OWNER_KEY")
check "host_state_query_succeeds" "$(has_error "$raw_query_resp" && echo 0 || echo 1)"
raw_struct=$(structured_of "$raw_query_resp")
RAW_ROW_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$raw_struct")
check "host_state_query_count_is_1000" "$([[ "$RAW_ROW_COUNT" == "1000" ]] && echo 1 || echo 0)"
echo "HOST_STATE_QUERY_ROW_COUNT=${RAW_ROW_COUNT}"

# 5. Publish `query` as a python tool -- requirement 1/3.
QUERY_SRC=$(cat tools/query.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"query","kind":"python","spec":{"source": sys.argv[1]}}))' "$QUERY_SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$OWNER_KEY")
QUERY_QUALIFIED=$(json_str "$(structured_of "$publish_resp")" "name")
check "owner_publish_query" "$([[ -n "$QUERY_QUALIFIED" ]] && echo 1 || echo 0)"

# 6. Equality question -- AC3: id=777's amount is a known fixture value.
eq_resp=$(mcp_call_ready "$QUERY_QUALIFIED" '{"where": [{"col": "id", "op": "=", "value": 777}]}' "$OWNER_KEY")
check "equality_question_succeeds" "$(has_error "$eq_resp" && echo 0 || echo 1)"
eq_struct=$(structured_of "$eq_resp")
EQ_AMOUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("amount","") if rows else "")' "$eq_struct")
check "equality_question_matches_known_value" "$([[ "$EQ_AMOUNT" == "317.46" ]] && echo 1 || echo 0)"
echo "EQUALITY_AMOUNT=${EQ_AMOUNT}"

# 7. Range question -- AC3: rows with amount > 300 has a known count.
range_resp=$(mcp_call "$QUERY_QUALIFIED" '{"where": [{"col": "amount", "op": ">", "value": 300}]}' "$OWNER_KEY")
check "range_question_succeeds" "$(has_error "$range_resp" && echo 0 || echo 1)"
range_struct=$(structured_of "$range_resp")
RANGE_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d.get("count", -1))' "$range_struct")
check "range_question_matches_known_value" "$([[ "$RANGE_COUNT" == "476" ]] && echo 1 || echo 0)"
echo "RANGE_COUNT=${RANGE_COUNT}"

# 8. Count question -- AC3: rows in category "produce" has a known count.
count_resp=$(mcp_call "$QUERY_QUALIFIED" '{"where": [{"col": "category", "op": "=", "value": "produce"}]}' "$OWNER_KEY")
check "count_question_succeeds" "$(has_error "$count_resp" && echo 0 || echo 1)"
count_struct=$(structured_of "$count_resp")
CAT_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d.get("count", -1))' "$count_struct")
check "count_question_matches_known_value" "$([[ "$CAT_COUNT" == "200" ]] && echo 1 || echo 0)"
echo "CATEGORY_COUNT=${CAT_COUNT}"

# 9. Guardrail -- AC4: LIKE and a raw SQL string are both refused, no rows.
like_resp=$(mcp_call_ready "$QUERY_QUALIFIED" '{"where": [{"col": "category", "op": "LIKE", "value": "%prod%"}]}' "$OWNER_KEY")
check "like_operator_rejected" "$(has_error "$like_resp" && echo 1 || echo 0)"

rawsql_resp=$(mcp_call "$QUERY_QUALIFIED" '{"where": "id = 1 OR 1=1"}' "$OWNER_KEY")
check "raw_sql_where_rejected" "$(has_error "$rawsql_resp" && echo 1 || echo 0)"

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
