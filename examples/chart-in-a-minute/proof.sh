#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Chart in a minute" (see
# www/llms.txt). One fresh tenant against $MCPHOST_URL: creates the
# `expenses` table with `host.table.create`, loads fixture.csv (1,000
# rows) in batches of 200 with `host.table.append`, calls
# `host.table.chart` for "sum of amount by category", asserts the
# caption's largest-category fact equals the fixture's known value
# (pantry, 6599), and opens the share URL with curl expecting 200 and the
# spec inline.
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
                "name": "chart-in-a-minute-proof",
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

has_error() {
  python3 - "$1" <<'PY'
import json, sys
sys.exit(0 if "error" in json.loads(sys.argv[1]) else 1)
PY
}

# ---- the recipe -------------------------------------------------------------

SYNTHETIC_HEADER="recipe:chart-in-a-minute"
BATCH_SIZE=200

# 1. Fresh tenant, tagged so this proof's own signup is distinguishable
#    from real recipe traffic (requirement 4 / AC5 convention, same as
#    examples/database-in-a-minute/proof.sh).
owner_resp=$(mcp_call "signup" '{"name": "chart-in-a-minute-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_str "$owner_struct" "tenant")
OWNER_KEY=$(json_str "$owner_struct" "key")
check "owner_signed_up" "$([[ -n "$OWNER_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"

# 2. Create the real-SQL table -- requirement 1, same shape as the
#    database recipe's `expenses` table.
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

# 4. Ask for the picture -- requirement 3, AC2/AC3: one chart call, shared.
chart_resp=$(mcp_call "host.table.chart" '{"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category", "share": true}' "$OWNER_KEY")
check "chart_call_succeeds" "$(has_error "$chart_resp" && echo 0 || echo 1)"
chart_struct=$(structured_of "$chart_resp")

SCHEMA=$(json_str "$chart_struct" "schema")
check "chart_schema_is_chart_v1" "$([[ "$SCHEMA" == "chart.v1" ]] && echo 1 || echo 0)"

MARK=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d.get("recommendation",{}).get("mark",""))' "$chart_struct")
check "chart_recommends_bar" "$([[ "$MARK" == "bar" ]] && echo 1 || echo 0)"
echo "RECOMMENDED_MARK=${MARK}"

# AC3: the caption's largest-category fact equals the fixture's known
# value -- pantry (cat_idx 4, base 10+5*4=30) sums to 6599 over its 200
# rows, the same independent recomputation
# tests/support/chart_fixture.rs::expected_category_totals() proves.
LARGEST_FACT=$(python3 -c '
import json, sys
d = json.loads(sys.argv[1])
facts = d.get("caption", {}).get("facts", [])
for f in facts:
    if f.get("label", "").startswith("largest"):
        print(f.get("value"))
        break
else:
    print("")
' "$chart_struct")
check "largest_category_fact_matches_fixture" "$([[ "$LARGEST_FACT" == "6599.0" || "$LARGEST_FACT" == "6599" ]] && echo 1 || echo 0)"
echo "LARGEST_CATEGORY_FACT=${LARGEST_FACT}"

HEADLINE=$(json_str "$chart_struct" "caption.headline")
check "headline_names_pantry" "$([[ "$HEADLINE" == *"pantry"* ]] && echo 1 || echo 0)"
echo "CAPTION_HEADLINE=${HEADLINE}"

# 5. Share link -- requirement 6: opens with no auth, 200, spec inline.
SHARE_URL=$(json_str "$chart_struct" "share_url")
check "share_url_present" "$([[ -n "$SHARE_URL" ]] && echo 1 || echo 0)"
echo "SHARE_URL=${SHARE_URL}"

if [[ -n "$SHARE_URL" ]]; then
  SHARE_BODY=$(curl -sS --connect-timeout 5 --max-time 30 -w '\nHTTP_STATUS:%{http_code}' "$SHARE_URL")
  SHARE_STATUS=$(echo "$SHARE_BODY" | grep -o 'HTTP_STATUS:[0-9]*' | cut -d: -f2)
  check "share_url_returns_200" "$([[ "$SHARE_STATUS" == "200" ]] && echo 1 || echo 0)"
  check "share_page_has_spec_inline" "$(echo "$SHARE_BODY" | grep -q '"category"' && echo 1 || echo 0)"
else
  check "share_url_returns_200" "0"
  check "share_page_has_spec_inline" "0"
fi

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
