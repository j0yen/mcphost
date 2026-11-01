#!/usr/bin/env bash
# recipe-counter-live.sh -- PRD-mcphost-admin-funnel-real-surface-recipe-counter AC8.
#
# Run on the host serving the merged build, from the operator's checkout:
#   ssh mcphost-1 'bash -s' < scripts/ops/recipe-counter-live.sh
# Sources /etc/mcphost/env itself (the key is never printed), reads the funnel
# CLI's recipe counters, signs up ONE tenant whose MCP clientInfo name starts
# `synthorg`, resolves the `webhook` alias through host.quickstart once, reads
# the counters again, and offboards the tenant through host.self_offboard (the
# offboard also runs on any failure after the signup).
#
# stdout (success): `synthetic_delta=1 real_delta=0` / `offboard: ok`
# stdout (failure): `status: error reason: <step> <code>` and, when a tenant
#                   was created, `offboard: ok|failed`
# exit: 0 synthetic_delta=1 real_delta=0 and offboarded, 1 anything else,
#       2 missing prerequisite.
# shellcheck disable=SC2329  # finish runs via the EXIT trap
set -euo pipefail

set -a
# shellcheck disable=SC1091
. /etc/mcphost/env
set +a

BIN="${MCPHOST_BIN:-/opt/mcphost/bin/mcphost}"
BASE="${MCPHOST_PUBLIC_URL:-https://mcphost.dev}"
BASE="${BASE%/}"
for tool in curl python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "recipe-counter-live.sh: missing required tool: $tool" >&2; exit 2; }
done
[ -x "$BIN" ] || { echo "recipe-counter-live.sh: $BIN not executable" >&2; exit 2; }

KEY=""
OFFBOARD="not_needed"

# rpc <step> <tenant_key|-> <tool> <args-json>: prints the structured result on
# stdout; on failure prints `<step> <code>` (a bare [a-z0-9_] token) and returns 1.
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
        "io.modelcontextprotocol/clientInfo": {"name": "synthorg-recipe-counter-live", "version": "1"}}}}))
' "$tool" "$args" "$key") || { echo "$step bad_request"; return 1; }
  resp=$(curl -sS --max-time 20 -X POST "$BASE/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'MCP-Protocol-Version: 2026-07-28' \
    -H 'Mcp-Method: tools/call' \
    -H "Mcp-Name: $tool" \
    --data-binary "$body" 2>/dev/null) || { echo "$step unreachable"; return 1; }
  printf '%s' "$resp" | python3 -c '
import json, re, sys
step = sys.argv[1]
msgs = []
for line in sys.stdin.read().splitlines():
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
if "error" in msg:
    data = msg["error"].get("data") if isinstance(msg["error"], dict) else None
    code = data.get("error_code") if isinstance(data, dict) else None
    print(step + " " + clean(code or "rpc_error", "rpc_error"))
    sys.exit(1)
res = msg["result"]
sc = res.get("structuredContent")
if sc is None:
    try:
        sc = json.loads(res["content"][0]["text"])
    except Exception:
        sc = {}
if res.get("isError"):
    print(step + " " + clean(sc.get("error_code") or sc.get("code") or "tool_error", "tool_error"))
    sys.exit(1)
print(json.dumps(sc))
' "$step"
}

# counters: prints `<real_total> <synthetic_total>` from the funnel CLI.
counters() {
  "$BIN" funnel --json --days 7 | python3 -c '
import json, sys
d = json.load(sys.stdin)
real = sum(d["recipe_attempts_7d"].values())
print(real, d["recipe_attempts_synthetic_7d"])
'
}

fail() {
  echo "status: error reason: $1"
  exit 1
}

finish() {
  local rc=$?
  set +e
  if [ -n "$KEY" ]; then
    if rpc offboard "$KEY" host.self_offboard '{}' >/dev/null; then OFFBOARD="ok"; else OFFBOARD="failed"; fi
    echo "offboard: $OFFBOARD"
    [ "$OFFBOARD" = "ok" ] || rc=1
  fi
  exit "$rc"
}
trap 'finish' EXIT

read -r REAL_BEFORE SYN_BEFORE < <(counters) || fail "counters_before"

out=$(rpc signup - signup "{\"name\":\"recipe-counter-live-$(date +%s)\",\"handoff\":false}") || fail "$out"
KEY=$(printf '%s' "$out" | python3 -c 'import json, sys; print(json.load(sys.stdin).get("key", ""))')
[ -n "$KEY" ] || fail "signup bad_response"

out=$(rpc quickstart "$KEY" host.quickstart '{"kind":"webhook"}') || fail "$out"
printf '%s' "$out" | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("resolved_from") else 1)' \
  || fail "quickstart no_alias_resolution"

read -r REAL_AFTER SYN_AFTER < <(counters) || fail "counters_after"

SYN_DELTA=$((SYN_AFTER - SYN_BEFORE))
REAL_DELTA=$((REAL_AFTER - REAL_BEFORE))
echo "synthetic_delta=$SYN_DELTA real_delta=$REAL_DELTA"
[ "$SYN_DELTA" -eq 1 ] && [ "$REAL_DELTA" -eq 0 ] || fail "delta_mismatch"
