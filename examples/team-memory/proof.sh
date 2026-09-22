#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Give your agents one memory"
# (see www/llms.txt). Three fresh tenants against $MCPHOST_URL: the owner
# creates a `memory` table, publishes `remember`/`recall` as python tools,
# shares both to a group, and adds two teammates B and C. B remembers a
# row; C recalls it and sees B as the writer; the owner removes C from the
# group and C's next recall is a permission error while B's still works.
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
                "name": "team-memory-proof",
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

SYNTHETIC_HEADER="recipe:team-memory"

# 1. Three fresh tenants, tagged so this proof's own signups are
#    distinguishable from real recipe traffic (requirement 4).
owner_resp=$(mcp_call "signup" '{"name": "team-memory-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_str "$owner_struct" "tenant")
OWNER_KEY=$(json_str "$owner_struct" "key")

b_resp=$(mcp_call "signup" '{"name": "team-memory-b"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
b_struct=$(structured_of "$b_resp")
B_NS=$(json_str "$b_struct" "tenant")
B_KEY=$(json_str "$b_struct" "key")

c_resp=$(mcp_call "signup" '{"name": "team-memory-c"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
c_struct=$(structured_of "$c_resp")
C_NS=$(json_str "$c_struct" "tenant")
C_KEY=$(json_str "$c_struct" "key")

check "three_tenants_signed_up" "$([[ -n "$OWNER_KEY" && -n "$B_KEY" && -n "$C_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"
echo "B_NS=${B_NS}"
echo "C_NS=${C_NS}"

# 2. Owner creates the shared table (mcphost does not yet expose the
#    calling tenant's own id inside a shared python tool -- the section's
#    load-bearing check: `remember` below takes a caller-supplied `who`
#    instead, the documented fallback).
table_resp=$(mcp_call "host.table.create" '{"name": "memory", "columns": {"key": "text", "text": "text", "tags": "json", "writer": "text", "at": "real"}}' "$OWNER_KEY")
check "owner_table_create" "$(has_error "$table_resp" && echo 0 || echo 1)"

# 3. Owner publishes remember/recall as python tools.
REMEMBER_SRC=$(cat tools/remember.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"remember","kind":"python","spec":{"source": sys.argv[1]}}))' "$REMEMBER_SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$OWNER_KEY")
REMEMBER_QUALIFIED=$(json_str "$(structured_of "$publish_resp")" "name")
check "owner_publish_remember" "$([[ -n "$REMEMBER_QUALIFIED" ]] && echo 1 || echo 0)"

RECALL_SRC=$(cat tools/recall.py)
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name":"recall","kind":"python","spec":{"source": sys.argv[1]}}))' "$RECALL_SRC")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$OWNER_KEY")
RECALL_QUALIFIED=$(json_str "$(structured_of "$publish_resp")" "name")
check "owner_publish_recall" "$([[ -n "$RECALL_QUALIFIED" ]] && echo 1 || echo 0)"

# 4. Group, shares, and both teammates added.
group_resp=$(mcp_call "host.group.create" '{"name": "team"}' "$OWNER_KEY")
check "owner_group_create" "$(has_error "$group_resp" && echo 0 || echo 1)"

share_resp=$(mcp_call "host.tool_share" '{"name": "remember", "visibility": "group", "group": "team"}' "$OWNER_KEY")
check "owner_share_remember" "$(has_error "$share_resp" && echo 0 || echo 1)"

share_resp=$(mcp_call "host.tool_share" '{"name": "recall", "visibility": "group", "group": "team"}' "$OWNER_KEY")
check "owner_share_recall" "$(has_error "$share_resp" && echo 0 || echo 1)"

add_b_args=$(python3 -c 'import json,sys; print(json.dumps({"name": "team", "namespace": sys.argv[1]}))' "$B_NS")
add_resp=$(mcp_call "host.group.add" "$add_b_args" "$OWNER_KEY")
check "owner_group_add_b" "$(has_error "$add_resp" && echo 0 || echo 1)"

add_c_args=$(python3 -c 'import json,sys; print(json.dumps({"name": "team", "namespace": sys.argv[1]}))' "$C_NS")
add_resp=$(mcp_call "host.group.add" "$add_c_args" "$OWNER_KEY")
check "owner_group_add_c" "$(has_error "$add_resp" && echo 0 || echo 1)"

# 5. C first remembers an *older* "deploy" row, then B remembers a newer
#    one -- AC3 needs two rows on the same key so "newest first" is an
#    actual ordering claim, not a single-row tautology.
older_remember_args=$(python3 -c 'import json,sys; print(json.dumps({"key": "deploy", "text": "deploy window used to be Monday 6pm", "tags": ["ops", "old"], "who": sys.argv[1]}))' "$C_NS")
older_remember_resp=$(mcp_call_ready "$REMEMBER_QUALIFIED" "$older_remember_args" "$C_KEY")
check "c_remember_older_deploy_succeeds" "$(has_error "$older_remember_resp" && echo 0 || echo 1)"

# Ensure the two rows land at strictly different `at` timestamps so the
# ordering below is unambiguous (not just a rowid tie-break).
sleep 0.2

# B remembers something -- AC2: the row's writer is B's own tenant id.
remember_args=$(python3 -c 'import json,sys; print(json.dumps({"key": "deploy", "text": "deploy window is Tuesday 9pm", "tags": ["ops"], "who": sys.argv[1]}))' "$B_NS")
remember_resp=$(mcp_call_ready "$REMEMBER_QUALIFIED" "$remember_args" "$B_KEY")
check "b_remember_succeeds" "$(has_error "$remember_resp" && echo 0 || echo 1)"
remember_struct=$(structured_of "$remember_resp")
REMEMBER_WRITER=$(json_str "$remember_struct" "writer")
check "b_remember_writer_is_b" "$([[ "$REMEMBER_WRITER" == "$B_NS" ]] && echo 1 || echo 0)"
echo "WRITER=${REMEMBER_WRITER}"

# AC2's Then is about the row the owner's `memory` table actually holds,
# not about what `remember` said: `remember.py` hands the caller's own
# `who` straight back, so the echoed `writer` above still reads as B even
# if nothing (or the wrong value) were stored. Re-read the row by the id
# `remember` reported, with the owner's own key, through `host.table.query`.
REMEMBER_ID=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); v=d.get("id"); print(v if isinstance(v,int) else -1)' "$remember_struct")
stored_args=$(python3 -c 'import json,sys; print(json.dumps({"sql": "SELECT writer FROM memory WHERE rowid = %d" % int(sys.argv[1])}))' "$REMEMBER_ID")
stored_resp=$(mcp_call "host.table.query" "$stored_args" "$OWNER_KEY")
stored_struct=$(structured_of "$stored_resp")
STORED_WRITER=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("writer","") if rows else "")' "$stored_struct")
check "b_remember_row_in_table_has_writer_b" "$([[ -n "$B_NS" && "$STORED_WRITER" == "$B_NS" ]] && echo 1 || echo 0)"
echo "STORED_WRITER=${STORED_WRITER}"

# 6. C recalls "deploy" -- AC3: with two rows on the same key, the newer
#    (B's) row must come back first and the older (C's) row second, which
#    only holds if recall.py's ORDER BY is genuinely newest-first.
recall_resp=$(mcp_call_ready "$RECALL_QUALIFIED" '{"query": "deploy"}' "$C_KEY")
check "c_recall_succeeds" "$(has_error "$recall_resp" && echo 0 || echo 1)"
recall_struct=$(structured_of "$recall_resp")
ROW_COUNT=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("rows") or []))' "$recall_struct")
check "c_recall_returns_both_deploy_rows" "$([[ "$ROW_COUNT" == "2" ]] && echo 1 || echo 0)"
FIRST_ROW_WRITER=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[0].get("writer","") if rows else "")' "$recall_struct")
check "c_recall_shows_b_as_writer" "$([[ "$FIRST_ROW_WRITER" == "$B_NS" ]] && echo 1 || echo 0)"
SECOND_ROW_WRITER=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); rows=d.get("rows") or []; print(rows[1].get("writer","") if len(rows) > 1 else "")' "$recall_struct")
check "c_recall_older_c_row_is_second" "$([[ "$SECOND_ROW_WRITER" == "$C_NS" ]] && echo 1 || echo 0)"

# 7. Owner removes C from the group -- AC4.
remove_resp=$(mcp_call "host.group.remove" "$add_c_args" "$OWNER_KEY")
check "owner_group_remove_c" "$(has_error "$remove_resp" && echo 0 || echo 1)"

# 8. C's next recall is a permission error; B's still succeeds -- AC4.
c_recall_after_remove=$(mcp_call "$RECALL_QUALIFIED" '{"query": "deploy"}' "$C_KEY")
check "c_recall_after_remove_is_permission_error" "$(has_error "$c_recall_after_remove" && echo 1 || echo 0)"

b_recall_after_remove=$(mcp_call "$RECALL_QUALIFIED" '{"query": "deploy"}' "$B_KEY")
check "b_recall_still_succeeds_after_c_removed" "$(has_error "$b_recall_after_remove" && echo 0 || echo 1)"

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
