#!/usr/bin/env bash
# email-probe.sh -- probe the mcphost email inbound endpoint
#
# Verbs:
#   attach <fixture>   Send a fake-signed inbound mail with one part whose
#                      content is the fixture file, then read back the inbox
#                      and call host.blob.get on the first attachment's blob_id.
#                      Prints: blob_id=blob_<prefix> bytes=<n>
#
# Environment (all required):
#   MCPHOST_URL        base URL of the mcphost instance  (e.g. https://mcphost.dev)
#   MCPHOST_API_KEY    bearer token for the tenant
#   MCPHOST_NAMESPACE  tenant namespace used as the inbound path segment
#
# Environment (optional):
#   MCPHOST_EMAIL_INBOUND_SECRET  sent as X-Mcphost-Inbound-Secret; a caller
#                      outside the fleet's source addresses needs it
#
# Example (Live AC8 evidence):
#   set -a; . ~/.config/mcphost/joe-test.env; set +a
#   bash ~/wintermute/mcphost/scripts/email-probe.sh attach lines.csv

set -euo pipefail

verb="${1:-}"
fixture="${2:-}"

if [[ "$verb" != "attach" || -z "$fixture" ]]; then
    echo "usage: email-probe.sh attach <fixture>" >&2
    exit 1
fi

: "${MCPHOST_URL:?MCPHOST_URL is required}"
: "${MCPHOST_API_KEY:?MCPHOST_API_KEY is required}"
: "${MCPHOST_NAMESPACE:?MCPHOST_NAMESPACE is required}"

if [[ ! -f "$fixture" ]]; then
    echo "fixture file not found: $fixture" >&2
    exit 1
fi

filename="$(basename "$fixture")"
content_type="text/csv"
case "$filename" in
    *.pdf)  content_type="application/pdf" ;;
    *.png)  content_type="image/png" ;;
    *.jpg|*.jpeg) content_type="image/jpeg" ;;
    *.txt)  content_type="text/plain" ;;
esac

content_b64="$(base64 < "$fixture" | tr -d '\n')"
provider_id="probe-$(date +%s)-$$"

payload="$(cat <<EOF
{
  "provider_message_id": "$provider_id",
  "from": "probe@example.com",
  "subject": "email-probe attach $filename",
  "parts": [
    {
      "filename": "$filename",
      "content_type": "$content_type",
      "content_base64": "$content_b64",
      "inline": false
    }
  ]
}
EOF
)"

# POST to the inbound endpoint
inbound_out="$(mktemp)"
trap 'rm -f "$inbound_out"' EXIT
secret_args=()
if [[ -n "${MCPHOST_EMAIL_INBOUND_SECRET:-}" ]]; then
    secret_args=(-H "X-Mcphost-Inbound-Secret: ${MCPHOST_EMAIL_INBOUND_SECRET}")
fi
http_status="$(curl -s -o "$inbound_out" -w "%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    ${secret_args[@]+"${secret_args[@]}"} \
    -d "$payload" \
    "${MCPHOST_URL}/email/inbound/${MCPHOST_NAMESPACE}")"

if [[ "$http_status" != "200" ]]; then
    echo "inbound POST failed: HTTP $http_status" >&2
    cat "$inbound_out" >&2
    exit 1
fi

# One MCP tools/call over streamable HTTP: the reply is JSON or an SSE
# `data:` line; prints the tool's structured result as JSON.
mcp_call() {
    local name="$1" args="$2" id="$3"
    curl -s -X POST \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        -H "Authorization: Bearer ${MCPHOST_API_KEY}" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":${id},\"method\":\"tools/call\",\"params\":{\"name\":\"${name}\",\"arguments\":${args}}}" \
        "${MCPHOST_URL}/mcp" |
        python3 -c '
import sys, json
raw = sys.stdin.read()
lines = [l[5:].strip() for l in raw.splitlines() if l.startswith("data:")]
resp = json.loads(lines[-1] if lines else raw)
result = resp["result"]
print(json.dumps(result.get("structuredContent") or json.loads(result["content"][0]["text"])))
'
}

# Read the inbox, then the first attachment's bytes
inbox_json="$(mcp_call host.msg.inbox '{}' 1)" || { echo "inbox read failed" >&2; exit 1; }

blob_id="$(echo "$inbox_json" | python3 -c '
import sys, json
for m in json.load(sys.stdin).get("messages", []):
    for att in m.get("data", {}).get("attachments", []):
        if att.get("blob_id"):
            print(att["blob_id"])
            sys.exit(0)
')"

if [[ -z "$blob_id" ]]; then
    echo "no blob_id found in inbox" >&2
    echo "$inbox_json" >&2
    exit 1
fi

blob_json="$(mcp_call host.blob.get "{\"id\":\"${blob_id}\",\"as\":\"base64\"}" 2)" ||
    { echo "blob read failed" >&2; exit 1; }

byte_count="$(echo "$blob_json" | python3 -c '
import sys, json, base64
print(len(base64.b64decode(json.load(sys.stdin)["content_base64"])))
')"

# The id is printed as a `blob_` reference; the part after it is the id.
echo "blob_id=blob_${blob_id#blob_} bytes=${byte_count}"
