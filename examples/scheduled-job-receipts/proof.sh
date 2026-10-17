#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of PRD-mcphost-scheduled-job-receipts AC 8.
# One fresh tenant: publish an echo tool, schedule it, fire it three times with
# `host.trigger.fire`, then read `host.schedule_receipts` once. Prints
# `receipts=3 streak_failed=0` and exits 0 iff the summary agrees.
#
# Usage: bash proof.sh [MCP_URL]   (default $MCPHOST_URL, else https://mcphost.dev/mcp)
# Requires: bash, curl, python3. Deliberately has no jq dependency.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${1:-${MCPHOST_URL:-https://mcphost.dev/mcp}}"
# MCPHOST_PROVE_DRY=1: print the plan and exit 0 before any network call.
[[ -n "${MCPHOST_PROVE_DRY:-}" ]] && { echo "plan: scheduled-job-receipts proof against $MCPHOST_URL"; exit 0; }

mcp_call() {
  # mcp_call <tool> <args_json> [bearer_key] [extra_header_name] [extra_header_value]
  local tool="$1" args="$2" key="${3:-}" hname="${4:-}" hval="${5:-}" body raw
  body=$(python3 - "$tool" "$args" <<'PY'
import json, sys
print(json.dumps({
    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {
        "name": sys.argv[1],
        "arguments": json.loads(sys.argv[2]),
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "scheduled-job-receipts-proof", "version": "1.0.0"},
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
  raw=$(curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" "${hdrs[@]}" -d "$body")
  # A binding call (signup) may answer as text/event-stream: keep the last data line.
  python3 - "$raw" <<'PYEOF'
import sys
raw = sys.argv[1]
if raw.lstrip().startswith("data:"):
    last = None
    for line in raw.splitlines():
        if line.startswith("data:"):
            last = line[5:].lstrip()
    print(last if last is not None else raw)
else:
    print(raw)
PYEOF
}

# structured <raw_json> [dotted.path] -- prints the structured result (or the value at path).
structured() {
  python3 - "$1" "${2:-}" <<'PY'
import json, sys
body = json.loads(sys.argv[1])
if "error" in body:
    print(""); sys.exit(0)
r = body.get("result") or {}
d = r.get("structuredContent")
if d is None:
    c = r.get("content") or []
    d = json.loads(c[0]["text"]) if c and "text" in c[0] else None
for p in [p for p in sys.argv[2].split(".") if p]:
    d = d.get(p) if isinstance(d, dict) else None
print(d if isinstance(d, str) or d is None else json.dumps(d))
PY
}

die() { echo "FAIL: $1"; exit 1; }

signup=$(mcp_call "signup" '{"name": "scheduled-job-receipts-proof"}' "" "x-mcphost-synthetic" "recipe:scheduled-job-receipts")
KEY=$(structured "$signup" key)
[[ -n "$KEY" ]] || die "signup returned no key"

pub=$(mcp_call "host.tool_publish" '{"name":"probe","kind":"echo","spec":{"schema":{"type":"object"}}}' "$KEY")
[[ -n "$(structured "$pub")" ]] || die "publish probe refused"

set_resp=$(mcp_call "host.trigger.set" '{"tool":"probe","kind":"schedule","schedule":"*/5 * * * *"}' "$KEY")
TRIGGER_ID=$(structured "$set_resp" id)
[[ -n "$TRIGGER_ID" ]] || die "trigger.set returned no id"

fire_args=$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1]}))' "$TRIGGER_ID")
for _ in 1 2 3; do
  [[ -n "$(structured "$(mcp_call "host.trigger.fire" "$fire_args" "$KEY")")" ]] || die "trigger.fire refused"
done

# Receipts land when each run ends: poll up to ~20 s for all three.
recv_args=$(python3 -c 'import json,sys; print(json.dumps({"trigger_id": sys.argv[1]}))' "$TRIGGER_ID")
RUNS=0
for _ in $(seq 1 100); do
  got=$(structured "$(mcp_call "host.schedule_receipts" "$recv_args" "$KEY")")
  RUNS=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1] or "{}"); print((d.get("summary") or {}).get("runs", 0))' "$got")
  [[ "$RUNS" -ge 3 ]] && break
  sleep 0.2
done
STREAK=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1] or "{}"); print((d.get("summary") or {}).get("streak_failed", "?"))' "$got")

echo "receipts=${RUNS} streak_failed=${STREAK}"
[[ "$RUNS" == "3" && "$STREAK" == "0" ]] && exit 0
exit 1
