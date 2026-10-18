#!/usr/bin/env bash
# capabilities-wire-check.sh -- PRD-mcphost-capabilities-generated-from-dispatch R6.
# Reads GET <base_url>/capabilities.json and, for every method of every
# `implemented: false` row plus every `server_to_client` method, POSTs
#   {"jsonrpc":"2.0","id":1,"method":M,"params":{}}
# keyless with NO Mcp-Method header (what a landscape client sends) to
# <base_url>/mcp. A reply is `labelled` when it is a -32601 whose
# `error.data.help_url` carries `/help/`, `bare` when it is a -32601 without.
# Prints `methods=N labelled=K bare=B`; exits 0 iff B == 0 and K == N.
#
# Usage: scripts/capabilities-wire-check.sh <base_url>
set -uo pipefail

BASE="${1:-}"
if [ -z "$BASE" ]; then
  echo "usage: capabilities-wire-check.sh <base_url>" >&2
  exit 2
fi
BASE="${BASE%/}"

DOC="$(curl -fsS --max-time 30 "$BASE/capabilities.json")" || {
  echo "capabilities-wire-check: GET $BASE/capabilities.json failed" >&2
  echo "methods=0 labelled=0 bare=0"
  exit 1
}

METHODS="$(printf '%s' "$DOC" | jq -r '
  [ .primitives[] | ((select(.implemented == false) | .methods[]), (.server_to_client // [])[]) ]
  | unique | .[]')" || exit 1

n=0
labelled=0
bare=0
while IFS= read -r m; do
  [ -n "$m" ] || continue
  n=$((n + 1))
  reply="$(curl -sS --max-time 30 -X POST "$BASE/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$m\",\"params\":{}}")" || reply=""
  # An SSE-framed reply carries the JSON on its `data:` line.
  json="$(printf '%s' "$reply" | sed -n 's/^data: *//p' | tail -n 1)"
  [ -n "$json" ] || json="$reply"
  verdict="$(printf '%s' "$json" | jq -r '
    if (.error.code == -32601) then
      (if ((.error.data.help_url // "") | test("/help/")) then "labelled" else "bare" end)
    else "other" end' 2>/dev/null || echo other)"
  case "$verdict" in
    labelled) labelled=$((labelled + 1)) ;;
    bare) bare=$((bare + 1)); echo "bare: $m" >&2 ;;
    *) echo "unexpected reply for $m: $reply" >&2 ;;
  esac
done <<<"$METHODS"

echo "methods=$n labelled=$labelled bare=$bare"
[ "$bare" -eq 0 ] && [ "$labelled" -eq "$n" ] && [ "$n" -gt 0 ]
