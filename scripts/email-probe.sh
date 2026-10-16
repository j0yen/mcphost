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
http_status="$(curl -s -o /tmp/email-probe-inbound.json -w "%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    -d "$payload" \
    "${MCPHOST_URL}/email/inbound/${MCPHOST_NAMESPACE}")"

if [[ "$http_status" != "200" ]]; then
    echo "inbound POST failed: HTTP $http_status" >&2
    cat /tmp/email-probe-inbound.json >&2
    exit 1
fi

# Read the inbox via MCP tools call
inbox_json="$(curl -s -X POST \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${MCPHOST_API_KEY}" \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host.msg.inbox","arguments":{}}}' \
    "${MCPHOST_URL}/mcp")"

blob_id="$(echo "$inbox_json" | python3 -c "
import sys, json
resp = json.load(sys.stdin)
msgs = resp['result']['content'][0]['text']
data = json.loads(msgs)
for m in data.get('messages', []):
    for att in m.get('data', {}).get('attachments', []):
        bid = att.get('blob_id')
        if bid:
            print(bid)
            sys.exit(0)
print('')
" 2>/dev/null)"

if [[ -z "$blob_id" ]]; then
    echo "no blob_id found in inbox" >&2
    echo "$inbox_json" | python3 -m json.tool >&2
    exit 1
fi

# Fetch the blob bytes
blob_json="$(curl -s -X POST \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${MCPHOST_API_KEY}" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"mcphost.blob.get\",\"arguments\":{\"blob_id\":\"${blob_id}\"}}}" \
    "${MCPHOST_URL}/mcp")"

byte_count="$(echo "$blob_json" | python3 -c "
import sys, json, base64
resp = json.load(sys.stdin)
text = resp['result']['content'][0]['text']
data = json.loads(text)
b64 = data.get('content_base64', '')
print(len(base64.b64decode(b64)))
" 2>/dev/null)"

echo "blob_id=${blob_id} bytes=${byte_count}"
