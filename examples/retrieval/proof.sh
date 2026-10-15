#!/usr/bin/env bash
# proof.sh -- runnable proof that a named index follows its sources
# (PRD-mcphost-index-refresh-on-change AC 8).
#
#   examples/retrieval/proof.sh --refresh-check
#
# Against $MCPHOST_URL, as the joe-test tenant: re-puts one probe document
# (`hidx-probe/probe.md`) with fresh text under the `hidx-probe` index
# (created on the first run), polls `host.index.status` and prints
# `behind_by=<n>` each time the number changes -- `behind_by=1` right after
# the put, `behind_by=0` once the indexer has followed, within 30 s -- then
# queries for the new text and checks the hit comes back.
#
# The tenant key is read from ~/.config/mcphost/joe-test.env (the first of
# MCPHOST_KEY, MCPHOST_API_KEY, MCPHOST_JOE_TEST_KEY that it sets) and is
# never printed.
#
# Requires: bash, curl, python3.
# Env vars:
#   MCPHOST_URL   Default: https://mcphost.dev/mcp
#   MCPHOST_ENV   Default: ~/.config/mcphost/joe-test.env
#
# Exit code 0 iff every check passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
MCPHOST_ENV="${MCPHOST_ENV:-$HOME/.config/mcphost/joe-test.env}"
INDEX="hidx-probe"
DOC="hidx-probe/probe.md"
DEADLINE_S=30
FAILURES=0

check() {
  if [[ "$2" == "1" ]]; then echo "CHECK $1: PASS"; else echo "CHECK $1: FAIL"; FAILURES=$((FAILURES + 1)); fi
}

if [[ "${1:-}" != "--refresh-check" ]]; then
  echo "usage: proof.sh --refresh-check" >&2
  exit 2
fi

if [[ ! -r "$MCPHOST_ENV" ]]; then
  echo "proof.sh: cannot read $MCPHOST_ENV" >&2
  exit 2
fi
# shellcheck disable=SC1090
set -a; . "$MCPHOST_ENV"; set +a
KEY="${MCPHOST_KEY:-${MCPHOST_API_KEY:-${MCPHOST_JOE_TEST_KEY:-}}}"
if [[ -z "$KEY" ]]; then
  echo "proof.sh: $MCPHOST_ENV sets none of MCPHOST_KEY, MCPHOST_API_KEY, MCPHOST_JOE_TEST_KEY" >&2
  exit 2
fi

# mcp_call <tool> <args_json> -- prints the final JSON-RPC message.
mcp_call() {
  local tool="$1" args="$2" body raw
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
            "io.modelcontextprotocol/clientInfo": {"name": "retrieval-proof", "version": "1.0.0"},
        },
    },
}))
PY
  )
  raw=$(curl -sS --connect-timeout 5 --max-time 30 -X POST "$MCPHOST_URL" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "MCP-Protocol-Version: 2026-07-28" \
    -H "Mcp-Method: tools/call" \
    -H "Mcp-Name: ${tool}" \
    -H "Authorization: Bearer ${KEY}" \
    -d "$body")
  python3 - "$raw" <<'PYEOF'
import sys
raw = sys.argv[1]
if raw.lstrip().startswith("data:"):
    last = None
    for block in raw.split("\n\n"):
        for line in block.splitlines():
            if line.startswith("data:"):
                last = line[len("data:"):].lstrip()
    print(last if last is not None else raw)
else:
    print(raw)
PYEOF
}

# field_of <raw_json> <python-expr over d = structuredContent> -- prints the value.
field_of() {
  python3 - "$1" "$2" <<'PY'
import json, sys
body = json.loads(sys.argv[1])
if "error" in body:
    print("ERROR:" + str((body["error"].get("data") or {}).get("error_code", "")))
    sys.exit(0)
d = (body.get("result") or {}).get("structuredContent") or {}
print(eval(sys.argv[2]))
PY
}

behind_of() {
  field_of "$1" "sum(s.get('behind_by', 0) for s in d.get('sources', []))"
}

TOKEN="probe-$(date +%s)-$RANDOM"
TEXT="hidx-probe refresh check. The marker word for this run is ${TOKEN}."

# First run: the index does not exist yet.
status=$(mcp_call host.index.status "{\"name\": \"$INDEX\"}")
if [[ "$(field_of "$status" "0")" == ERROR:* ]]; then
  mcp_call host.docs.put "{\"name\": \"$DOC\", \"content\": \"hidx-probe seed document.\"}" > /dev/null
  mcp_call host.index.create "{\"name\": \"$INDEX\", \"sources\": [{\"kind\": \"doc\", \"prefix\": \"hidx-probe/\"}]}" > /dev/null
  for _ in $(seq 1 "$DEADLINE_S"); do
    [[ "$(field_of "$(mcp_call host.index.status "{\"name\": \"$INDEX\"}")" "d.get('pending', 1)")" == "0" ]] && break
    sleep 1
  done
fi

# The refresh check: re-put, then watch behind_by.
put=$(mcp_call host.docs.put "{\"name\": \"$DOC\", \"content\": \"$TEXT\"}")
check "re-put the probe document" "$([[ "$(field_of "$put" "'name' in d")" == "True" ]] && echo 1 || echo 0)"

START=$(date +%s)
last=""
reached_zero=0
saw_one=0
while (( $(date +%s) - START <= DEADLINE_S )); do
  b=$(behind_of "$(mcp_call host.index.status "{\"name\": \"$INDEX\"}")")
  if [[ "$b" != "$last" ]]; then
    echo "behind_by=$b"
    last="$b"
  fi
  [[ "$b" == "1" ]] && saw_one=1
  if [[ "$b" == "0" && "$saw_one" == "1" ]]; then reached_zero=1; break; fi
  sleep 1
done
check "behind_by=1 then behind_by=0 within ${DEADLINE_S}s" "$reached_zero"

hit=$(mcp_call host.index.query "{\"name\": \"$INDEX\", \"query\": \"$TOKEN\"}")
check "the query for the new text returns it" \
  "$([[ "$(field_of "$hit" "any('$TOKEN' in r.get('text', '') for r in d.get('results', []))")" == "True" ]] && echo 1 || echo 0)"

if (( FAILURES == 0 )); then echo "retrieval proof: PASS"; exit 0; fi
echo "retrieval proof: ${FAILURES} check(s) failed"
exit 1
