#!/usr/bin/env bash
# billing-selfcheck-ledger.sh -- PRD-mcphost-billing-selfcheck-no-ledger-row AC5/AC6.
#
# Proves the status feed's billing self-check no longer writes ledger rows:
# reads <base>/status.json (restart.last.at and the billing component state),
# then admin.billing_ledger at <base>/mcp since restart.last.at + 120 s and
# looks for event ids starting evt_status_self_check_.
#
# Usage: billing-selfcheck-ledger.sh <base-url>
# Env:   MCPHOST_ADMIN_KEY (required; sent on stdin to curl, never printed).
# Needs: bash, curl, jq (no skip: a missing tool exits 2).
# exit:  0 clean; 1 a self-check row is present; 2 key unset, usage, missing
#        tool or a call failed; 3 billing is not ok; 4 fewer than 180 s since
#        restart.last.at (too early to judge).
set -euo pipefail

PROTOCOL="2026-07-28"
PREFIX="evt_status_self_check_"

if [ $# -ne 1 ] || [ -z "$1" ]; then
  echo "usage: billing-selfcheck-ledger.sh <base-url>" >&2
  exit 2
fi
BASE="${1%/}"
for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "billing-selfcheck-ledger.sh: missing required tool: $tool" >&2; exit 2; }
done
if [ -z "${MCPHOST_ADMIN_KEY:-}" ]; then
  echo "billing-selfcheck-ledger.sh: MCPHOST_ADMIN_KEY is not set" >&2
  exit 2
fi

STATUS=$(curl -sS --max-time 20 "$BASE/status.json") \
  || { echo "billing-selfcheck-ledger.sh: GET /status.json failed" >&2; exit 2; }
printf '%s' "$STATUS" | jq -e . >/dev/null 2>&1 \
  || { echo "billing-selfcheck-ledger.sh: /status.json is not JSON" >&2; exit 2; }

BILLING_STATE=$(printf '%s' "$STATUS" | jq -r '[.components[]? | select(.name == "billing") | .state][0] // "missing"')
if [ "$BILLING_STATE" != "ok" ]; then
  echo "billing-selfcheck-ledger.sh: billing component state is $BILLING_STATE" >&2
  exit 3
fi

NOW=$(date +%s)
RESTART_AT=$(printf '%s' "$STATUS" | jq -r '.restart.last.at // empty')
if [ -n "$RESTART_AT" ]; then
  if [ $((NOW - RESTART_AT)) -lt 180 ]; then
    echo "billing-selfcheck-ledger.sh: only $((NOW - RESTART_AT)) s since restart.last.at; wait until 180 s" >&2
    exit 4
  fi
  SINCE=$((RESTART_AT + 120))
else
  SINCE=$((NOW - 600))
  echo "fallback-window: restart.last is null, since=$SINCE (now-600)"
fi

BODY=$(jq -cn --argjson since "$SINCE" --arg proto "$PROTOCOL" '{
  jsonrpc: "2.0", id: 1, method: "tools/call",
  params: {name: "admin.billing_ledger", arguments: {since: $since}, _meta: {
    "io.modelcontextprotocol/protocolVersion": $proto,
    "io.modelcontextprotocol/clientCapabilities": {},
    "io.modelcontextprotocol/clientInfo": {name: "billing-selfcheck-ledger", version: "1"}}}}')

# The key reaches curl on stdin, never argv or output.
REPLY=$(printf 'header = "Authorization: Bearer %s"\n' "$MCPHOST_ADMIN_KEY" | curl -sS --max-time 20 -X POST "$BASE/mcp" \
  -K - \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -H "mcp-protocol-version: $PROTOCOL" \
  -H 'mcp-method: tools/call' \
  -H 'mcp-name: admin.billing_ledger' \
  --data-binary "$BODY") \
  || { echo "billing-selfcheck-ledger.sh: admin.billing_ledger call failed" >&2; exit 2; }

# JSON or SSE reply: strip a `data:` prefix, keep the last JSON-RPC message.
MSG=$(printf '%s\n' "$REPLY" | sed -e 's/^data:[[:space:]]*//' | grep '^{' | tail -n 1 || true)
EVENTS=$(printf '%s' "$MSG" | jq -c 'if .error then empty
  else (.result.structuredContent.events // (try (.result.content[0].text | fromjson | .events) catch null)) end' 2>/dev/null || true)
if [ -z "$EVENTS" ] || [ "$EVENTS" = "null" ]; then
  echo "billing-selfcheck-ledger.sh: no ledger events in the reply" >&2
  exit 2
fi

COUNT=$(printf '%s' "$EVENTS" | jq --arg p "$PREFIX" '[.[] | select(.event_id | startswith($p))] | length')
if [ "$COUNT" -gt 0 ]; then
  echo "self-check rows since $SINCE: $COUNT"
  exit 1
fi
echo "self-check rows since $SINCE: 0"
