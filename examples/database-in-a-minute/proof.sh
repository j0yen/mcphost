#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Give Claude a database in one
# minute" (see www/llms.txt). One fresh tenant against $MCPHOST_URL:
# creates the `expenses` table with `host.table.create`, loads
# fixture.csv (1,000 rows) in batches of 200 with `host.table.append`,
# attaches one `host.table.model_set` description, and asks the
# equality/range/count/GROUP BY/LIKE questions as plain SQL through
# `host.table.query` -- no published tool, no `where` grammar
# (PRD-mcphost-table-context-and-sql-passthrough requirement 5 / AC8).
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
  local raw
  raw=$(curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" "${hdrs[@]}" -d "$body")
  # PRD-mcphost-one-next-tool requirement 3: a call that binds this
  # connection (signup) now emits notifications/tools/list_changed before
  # its own result, which upgrades that one response from a plain JSON
  # body to a text/event-stream one (one "data: <json>" line per message,
  # blocks separated by a blank line) -- normalize back to the plain
  # final JSON-RPC message here so every caller below keeps reading a
  # single JSON object either way.
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

json_num() {
  # json_num <json> <dotted.path> -- prints the raw number at that path
  # (rendered without a trailing .0 for whole floats), or empty if
  # missing/not-a-number.
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
for p in [p for p in sys.argv[2].split(".") if p]:
    d = d.get(p) if isinstance(d, dict) else None
if isinstance(d, bool) or not isinstance(d, (int, float)):
    print("")
elif isinstance(d, float) and d.is_integer():
    print(int(d))
else:
    print(d)
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
#    from real recipe traffic.
owner_resp=$(mcp_call "signup" '{"name": "database-in-a-minute-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_KEY=$(json_str "$owner_struct" "key")
check "owner_signed_up" "$([[ -n "$OWNER_KEY" ]] && echo 1 || echo 0)"

# 2. Declare the table -- requirement 5.
table_resp=$(mcp_call "host.table.create" '{"name": "expenses", "columns": {"id": "integer", "category": "text", "amount": "real", "day": "integer"}, "primary_key": "id"}' "$OWNER_KEY")
check "table_create" "$(has_error "$table_resp" && echo 0 || echo 1)"

# 3. Batched append of the 1,000-row fixture -- requirement 5.
APPEND_FAILURES=0
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
  batch_resp=$(mcp_call "host.table.append" "$batch_args" "$OWNER_KEY")
  if has_error "$batch_resp"; then
    APPEND_FAILURES=$((APPEND_FAILURES + 1))
    echo "APPEND_BATCH_OFFSET_${offset}_ERROR=${batch_resp}"
  fi
done
check "batched_append_five_calls_of_200_succeed" "$([[ "$APPEND_FAILURES" -eq 0 ]] && echo 1 || echo 0)"

# 4. One table-level note -- requirement 1/5.
model_resp=$(mcp_call "host.table.model_set" '{"table": "expenses", "key": "description", "value": "Household spending, one row per purchase."}' "$OWNER_KEY")
check "model_set_description" "$(has_error "$model_resp" && echo 0 || echo 1)"

# 5. Equality question -- id=777's amount is a known fixture value.
eq_resp=$(mcp_call "host.table.query" '{"sql": "SELECT amount FROM expenses WHERE id = 777"}' "$OWNER_KEY")
check "equality_question_succeeds" "$(has_error "$eq_resp" && echo 0 || echo 1)"
eq_struct=$(structured_of "$eq_resp")
EQ_AMOUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("amount","") if rows else "")' "$eq_struct")
check "equality_question_matches_known_value" "$([[ "$EQ_AMOUNT" == "317.46" ]] && echo 1 || echo 0)"
echo "EQUALITY_AMOUNT=${EQ_AMOUNT}"

# 6. Range question -- rows with amount > 300 has a known count.
range_resp=$(mcp_call "host.table.query" '{"sql": "SELECT COUNT(*) AS n FROM expenses WHERE amount > 300"}' "$OWNER_KEY")
check "range_question_succeeds" "$(has_error "$range_resp" && echo 0 || echo 1)"
range_struct=$(structured_of "$range_resp")
RANGE_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("n","") if rows else "")' "$range_struct")
check "range_question_matches_known_value" "$([[ "$RANGE_COUNT" == "476" ]] && echo 1 || echo 0)"
echo "RANGE_COUNT=${RANGE_COUNT}"

# 7. Count question -- rows in category "produce" has a known count.
count_resp=$(mcp_call "host.table.query" "{\"sql\": \"SELECT COUNT(*) AS n FROM expenses WHERE category = 'produce'\"}" "$OWNER_KEY")
check "count_question_succeeds" "$(has_error "$count_resp" && echo 0 || echo 1)"
count_struct=$(structured_of "$count_resp")
CAT_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("n","") if rows else "")' "$count_struct")
check "count_question_matches_known_value" "$([[ "$CAT_COUNT" == "200" ]] && echo 1 || echo 0)"
echo "CATEGORY_COUNT=${CAT_COUNT}"

# 8. GROUP BY question -- sum of amount by category, one row per category,
#    the old six-operator grammar could not express this at all.
groupby_resp=$(mcp_call "host.table.query" '{"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category ORDER BY category"}' "$OWNER_KEY")
check "group_by_question_succeeds" "$(has_error "$groupby_resp" && echo 0 || echo 1)"
groupby_struct=$(structured_of "$groupby_resp")
GROUP_BY_ROW_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$groupby_struct")
check "group_by_question_returns_one_row_per_category" "$([[ "$GROUP_BY_ROW_COUNT" == "5" ]] && echo 1 || echo 0)"
echo "GROUP_BY_ROW_COUNT=${GROUP_BY_ROW_COUNT}"

# 9. LIKE question -- category names containing "prod", the old grammar had
#    no operator for this at all.
like_resp=$(mcp_call "host.table.query" "{\"sql\": \"SELECT COUNT(*) AS n FROM expenses WHERE category LIKE '%prod%'\"}" "$OWNER_KEY")
check "like_question_succeeds" "$(has_error "$like_resp" && echo 0 || echo 1)"
like_struct=$(structured_of "$like_resp")
LIKE_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("n","") if rows else "")' "$like_struct")
check "like_question_matches_known_value" "$([[ "$LIKE_COUNT" == "200" ]] && echo 1 || echo 0)"
echo "LIKE_COUNT=${LIKE_COUNT}"

# 10. The query log holds every question just asked -- requirement 3/4.
qlog_resp=$(mcp_call "host.table.query_log" '{"limit": 10}' "$OWNER_KEY")
check "query_log_reads" "$(has_error "$qlog_resp" && echo 0 || echo 1)"
qlog_struct=$(structured_of "$qlog_resp")
QLOG_ROW_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$qlog_struct")
check "query_log_holds_at_least_five_questions" "$([[ "$QLOG_ROW_COUNT" -ge 5 ]] && echo 1 || echo 0)"
echo "QUERY_LOG_ROW_COUNT=${QLOG_ROW_COUNT}"

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
