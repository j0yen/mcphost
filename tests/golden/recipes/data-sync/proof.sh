#!/usr/bin/env bash
# GENERATED from recipes/data-sync.toml by `cargo xtask recipes` -- do not hand-edit.
# Recipe calls (budget: 8 calls, within 95 s):
#   1. host.sync.create
#   2. host.sync.run_now
#   3. host.sync.get
#   4. host.sync.run_now
#   5. host.sync.get
#   6. host.sync.list
#   7. host.sync.delete
# proof.sh -- runnable end-to-end proof of "Data sync" (see www/llms.txt).
# One fresh tenant against $MCPHOST_URL: signs up (header-stamped synthetic),
# registers a CSV source with host.sync.create, runs it twice with
# host.sync.run_now and reads each receipt with host.sync.get (the first must
# write every CSV row, the second must report no change), lists it with
# host.sync.list, then deletes it with host.sync.delete so no */15 schedule is
# left running on a throwaway tenant. The host fetches the CSV on its
# scheduler tick, so each get waits for a tick (waited_s= is the seconds since
# the matching run_now).
#
# Requires: bash, curl, python3. Deliberately has no jq dependency.
#
# Env vars:
#   MCPHOST_URL       the endpoint to run the recipe against.
#                     Default: https://mcphost.dev/mcp
#   DATA_SYNC_CSV_URL the CSV the sync points at. The host refuses its own
#                     domain (host_not_allowed), so the default is the
#                     committed examples/data-sync/orders.csv on GitHub.
#   DATA_SYNC_ROWS    data rows the first receipt must report as rows_added.
#                     Default: 12, the data rows of examples/data-sync/orders.csv.
#   SYNTHETIC_HEADER  the x-mcphost-synthetic label the signup carries.
#                     Default: recipe:data-sync. Set to the empty string it
#                     REFUSES (exit 2) against a non-loopback MCPHOST_URL:
#                     a signup with no label would be counted as a real user.
#
# Exit code 0 iff every check below passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
# MCPHOST_PROVE_DRY=1: print the plan and exit 0 before any network call (the runner-contract test sets it).
[[ -n "${MCPHOST_PROVE_DRY:-}" ]] && { echo "plan: data-sync proof against $MCPHOST_URL"; exit 0; }
# Seconds between re-issues, and the step's retry_s, of a retry_until_expect step (generated).
RETRY_UNTIL_EXPECT_POLL_S=5
RETRY_UNTIL_EXPECT_DEADLINE_S=40
DATA_SYNC_CSV_URL="${DATA_SYNC_CSV_URL:-https://raw.githubusercontent.com/j0yen/mcphost/main/examples/data-sync/orders.csv}"
DATA_SYNC_ROWS="${DATA_SYNC_ROWS:-12}"
# `-` not `:-`: unset takes the default label, an explicit empty value stays empty.
SYNTHETIC_HEADER="${SYNTHETIC_HEADER-recipe:data-sync}"

# Refuse a bare signup against anything but a loopback host. Any `@` is refused
# too: `http://127.0.0.1:80@evil.example/mcp` matches the loopback prefix below
# but its host is evil.example.
if [[ -z "$SYNTHETIC_HEADER" ]] \
  && { [[ "$MCPHOST_URL" == *@* ]] \
       || ! [[ "$MCPHOST_URL" =~ ^https?://(localhost|127\.[0-9]+\.[0-9]+\.[0-9]+|\[::1\])([:/]|$) ]]; }; then
  echo "SYNTHETIC_HEADER empty: refusing a bare signup against ${MCPHOST_URL}" >&2
  exit 2
fi

FAILURES=0
START_EPOCH=$(python3 -c 'import time; print(time.time())')

# The sync this run creates is deleted on any exit once it exists: a kill
# between create and delete must not leave a schedule running on the tenant.
SYNC_ID=""
OWNER_KEY=""
SYNC_DELETED=0
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  if [[ -n "$SYNC_ID" && -n "$OWNER_KEY" && "$SYNC_DELETED" != "1" ]]; then
    mcp_call "host.sync.delete" "$(python3 -c 'import json,sys; print(json.dumps({"sync_id": sys.argv[1]}))' "$SYNC_ID")" "$OWNER_KEY" >/dev/null 2>&1 || true
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

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
                "name": "data-sync-proof",
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
  # A call that binds this connection (signup) emits
  # notifications/tools/list_changed before its own result, which upgrades
  # that one response to a text/event-stream one (one "data: <json>" line
  # per message, blocks separated by a blank line) -- normalize back to the
  # plain final JSON-RPC message so every caller below reads one JSON object.
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

# json_get <json> <dotted.path> -- the value at that path (list indexes are
# numbers); strings raw, booleans as true/false, empty when missing or null.
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
for p in [p for p in sys.argv[2].split(".") if p]:
    if isinstance(d, list):
        try:
            d = d[int(p)]
        except (ValueError, IndexError):
            d = None
    elif isinstance(d, dict):
        d = d.get(p)
    else:
        d = None
    if d is None:
        break
if d is None:
    print("")
elif isinstance(d, bool):
    print("true" if d else "false")
elif isinstance(d, (dict, list)):
    print(json.dumps(d))
else:
    print(d)
PY
}

has_error() {
  python3 - "$1" <<'PY'
import json, sys
body = json.loads(sys.argv[1])
sys.exit(0 if ("error" in body or (body.get("result") or {}).get("isError")) else 1)
PY
}

# ---- the recipe -------------------------------------------------------------

# 1. Fresh tenant, labelled so this proof's own signup is distinguishable
#    from real recipe traffic.
owner_resp=$(mcp_call "signup" '{"name": "data-sync-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_get "$owner_struct" "tenant")
OWNER_KEY=$(json_get "$owner_struct" "key")
check "owner_signed_up" "$([[ -n "$OWNER_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"

# 2. Register the CSV source. The host validates the url but does not fetch it.
create_args=$(python3 -c 'import json,sys; print(json.dumps({"source": {"kind": "csv_url", "ref": sys.argv[1]}, "table": "orders", "mode": "pull"}))' "$DATA_SYNC_CSV_URL")
create_resp=$(mcp_call "host.sync.create" "$create_args" "$OWNER_KEY")
check "create_succeeds" "$(has_error "$create_resp" && echo 0 || echo 1)"
create_struct=$(structured_of "$create_resp")
SYNC_ID=$(json_get "$create_struct" "sync_id")
SYNC_TABLE=$(json_get "$create_struct" "table")
SYNC_STATUS=$(json_get "$create_struct" "status")
NEXT_RUN=$(json_get "$create_struct" "next_run_unix")
EXISTING=$(json_get "$create_struct" "existing")
check "create_returns_sync_id" "$([[ -n "$SYNC_ID" ]] && echo 1 || echo 0)"
check "create_returns_next_run_unix" "$([[ -n "$NEXT_RUN" ]] && echo 1 || echo 0)"
check "create_returns_status" "$([[ -n "$SYNC_STATUS" ]] && echo 1 || echo 0)"
check "create_table_is_orders" "$([[ "$SYNC_TABLE" == "orders" ]] && echo 1 || echo 0)"
check "create_is_new" "$([[ "$EXISTING" == "false" ]] && echo 1 || echo 0)"
echo "host.sync.create: sync_id=${SYNC_ID} table=${SYNC_TABLE} status=${SYNC_STATUS} next_run_unix=${NEXT_RUN} existing=${EXISTING}"

# poll_sync_get <label> <want_change_detected> <want_rows_added> <run_now_epoch>
# Re-issues host.sync.get until the receipt it reads is the one the matching
# run_now produced and every expect below holds (retry_until_expect, for
# ${RETRY_UNTIL_EXPECT_DEADLINE_S} s, every ${RETRY_UNTIL_EXPECT_POLL_S} s), then leaves the
# receipt fields in GET_* and the seconds since that run_now in GET_WAITED.
poll_sync_get() {
  local label="$1" want_change="$2" want_rows="$3" since="$4"
  local deadline=$(( $(date +%s) + RETRY_UNTIL_EXPECT_DEADLINE_S ))
  local get_args get_resp get_struct
  get_args=$(python3 -c 'import json,sys; print(json.dumps({"sync_id": sys.argv[1]}))' "$SYNC_ID")
  while :; do
    get_resp=$(mcp_call "host.sync.get" "$get_args" "$OWNER_KEY")
    get_struct=$(structured_of "$get_resp")
    GET_ERR=0; has_error "$get_resp" && GET_ERR=1
    GET_TABLE=$(json_get "$get_struct" "table")
    GET_RUN_ID=$(json_get "$get_struct" "last_run.run_id")
    GET_STARTED=$(json_get "$get_struct" "last_run.started_unix")
    GET_STATUS=$(json_get "$get_struct" "last_run.status")
    GET_ROWS=$(json_get "$get_struct" "last_run.rows_added")
    GET_CHANGED=$(json_get "$get_struct" "last_run.change_detected")
    GET_WHY=""
    if [[ "$GET_ERR" == "1" ]]; then GET_WHY="call errored"
    elif [[ "$GET_TABLE" != "orders" ]]; then GET_WHY="table is '${GET_TABLE}', want orders"
    elif [[ -z "$GET_RUN_ID" ]]; then GET_WHY="last_run.run_id is absent"
    elif [[ -z "$GET_STARTED" ]]; then GET_WHY="last_run.started_unix is absent"
    elif [[ "$GET_STATUS" != "ok" ]]; then GET_WHY="last_run.status is '${GET_STATUS}', want ok"
    elif [[ "$GET_CHANGED" != "$want_change" ]]; then GET_WHY="last_run.change_detected is '${GET_CHANGED}', want ${want_change}"
    elif [[ "$GET_ROWS" != "$want_rows" ]]; then GET_WHY="last_run.rows_added is '${GET_ROWS}', want ${want_rows}"
    fi
    [[ -z "$GET_WHY" ]] && break
    if [[ "$(date +%s)" -ge "$deadline" ]]; then
      echo "host.sync.get ${label}: still failing after ${RETRY_UNTIL_EXPECT_DEADLINE_S} s, last expect failure: ${GET_WHY}" >&2
      break
    fi
    sleep "$RETRY_UNTIL_EXPECT_POLL_S"
  done
  GET_WAITED=$(( $(date +%s) - since ))
}

# 3. Make the sync due now. The trigger tick claims it and sync::run writes
#    one receipt; two run_now before a tick coalesce, so each is issued once.
RUN1_EPOCH=$(date +%s)
run_resp=$(mcp_call "host.sync.run_now" "$(python3 -c 'import json,sys; print(json.dumps({"sync_id": sys.argv[1]}))' "$SYNC_ID")" "$OWNER_KEY")
check "run_now_succeeds" "$(has_error "$run_resp" && echo 0 || echo 1)"
RUN_NEXT=$(json_get "$(structured_of "$run_resp")" "next_run_unix")
check "run_now_returns_next_run_unix" "$([[ -n "$RUN_NEXT" ]] && echo 1 || echo 0)"
echo "host.sync.run_now: next_run_unix=${RUN_NEXT}"

# 4. First receipt: the 12-row CSV was written, so rows_added is its data-row
#    count and a change was detected.
poll_sync_get "(1st)" "true" "$DATA_SYNC_ROWS" "$RUN1_EPOCH"
check "get_succeeds" "$([[ "$GET_ERR" == "0" ]] && echo 1 || echo 0)"
check "get_table_is_orders" "$([[ "$GET_TABLE" == "orders" ]] && echo 1 || echo 0)"
check "get_last_run_has_run_id" "$([[ -n "$GET_RUN_ID" ]] && echo 1 || echo 0)"
check "get_last_run_has_started_unix" "$([[ -n "$GET_STARTED" ]] && echo 1 || echo 0)"
check "get_last_run_status_is_ok" "$([[ "$GET_STATUS" == "ok" ]] && echo 1 || echo 0)"
check "get_last_run_rows_added_is_csv_rows" "$([[ "$GET_ROWS" == "$DATA_SYNC_ROWS" ]] && echo 1 || echo 0)"
check "get_last_run_change_detected" "$([[ "$GET_CHANGED" == "true" ]] && echo 1 || echo 0)"
[[ -n "$GET_WHY" ]] && echo "FAILED EXPECT (1st host.sync.get): ${GET_WHY}"
FIRST_RUN_ID="$GET_RUN_ID"
echo "host.sync.get: table=${GET_TABLE} last_run.run_id=${GET_RUN_ID} last_run.started_unix=${GET_STARTED} last_run.status=${GET_STATUS} rows_added=${GET_ROWS} change_detected=${GET_CHANGED} waited_s=${GET_WAITED}"

# 5. Run it again against the same, unchanged CSV: the second receipt must read
#    "no change" (status ok, nothing added) and be a different run.
RUN2_EPOCH=$(date +%s)
run2_resp=$(mcp_call "host.sync.run_now" "$(python3 -c 'import json,sys; print(json.dumps({"sync_id": sys.argv[1]}))' "$SYNC_ID")" "$OWNER_KEY")
check "run_now_again_succeeds" "$(has_error "$run2_resp" && echo 0 || echo 1)"
RUN2_NEXT=$(json_get "$(structured_of "$run2_resp")" "next_run_unix")
check "run_now_again_returns_next_run_unix" "$([[ -n "$RUN2_NEXT" ]] && echo 1 || echo 0)"
echo "host.sync.run_now: next_run_unix=${RUN2_NEXT}"

poll_sync_get "(2nd)" "false" "0" "$RUN2_EPOCH"
check "get2_succeeds" "$([[ "$GET_ERR" == "0" ]] && echo 1 || echo 0)"
check "get2_last_run_status_is_ok" "$([[ "$GET_STATUS" == "ok" ]] && echo 1 || echo 0)"
check "get2_last_run_rows_added_is_zero" "$([[ "$GET_ROWS" == "0" ]] && echo 1 || echo 0)"
check "get2_last_run_change_detected_is_false" "$([[ "$GET_CHANGED" == "false" ]] && echo 1 || echo 0)"
check "get2_run_id_differs_from_get1" "$([[ -n "$GET_RUN_ID" && "$GET_RUN_ID" != "$FIRST_RUN_ID" ]] && echo 1 || echo 0)"
check "get2_expects_hold" "$([[ -z "$GET_WHY" ]] && echo 1 || echo 0)"
[[ -n "$GET_WHY" ]] && echo "FAILED EXPECT (2nd host.sync.get): ${GET_WHY}"
echo "host.sync.get: table=${GET_TABLE} last_run.run_id=${GET_RUN_ID} last_run.started_unix=${GET_STARTED} last_run.status=${GET_STATUS} rows_added=${GET_ROWS} change_detected=${GET_CHANGED} waited_s=${GET_WAITED}"

# 6. It is the tenant's one sync.
list_resp=$(mcp_call "host.sync.list" '{}' "$OWNER_KEY")
check "list_succeeds" "$(has_error "$list_resp" && echo 0 || echo 1)"
LIST_STRUCT=$(structured_of "$list_resp")
LIST_TABLE=$(json_get "$LIST_STRUCT" "syncs.0.table")
LIST_ID=$(json_get "$LIST_STRUCT" "syncs.0.sync_id")
check "list_first_table_is_orders" "$([[ "$LIST_TABLE" == "orders" ]] && echo 1 || echo 0)"
check "list_first_sync_is_the_created_one" "$([[ -n "$SYNC_ID" && "$LIST_ID" == "$SYNC_ID" ]] && echo 1 || echo 0)"
echo "host.sync.list: syncs[0].table=${LIST_TABLE}"

# 7. Delete it: a proof must not leave a schedule running on a throwaway tenant.
delete_resp=$(mcp_call "host.sync.delete" "$(python3 -c 'import json,sys; print(json.dumps({"sync_id": sys.argv[1]}))' "$SYNC_ID")" "$OWNER_KEY")
check "delete_succeeds" "$(has_error "$delete_resp" && echo 0 || echo 1)"
DELETED=$(json_get "$(structured_of "$delete_resp")" "deleted")
check "delete_reports_deleted" "$([[ "$DELETED" == "true" ]] && echo 1 || echo 0)"
[[ "$DELETED" == "true" ]] && SYNC_DELETED=1
echo "host.sync.delete: deleted=${DELETED}"

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
