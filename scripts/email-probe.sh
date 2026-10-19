#!/usr/bin/env bash
# email-probe.sh -- probe the mcphost email channel
#
# Verbs:
#   attach <fixture>   Send a fake-signed inbound mail with one part whose
#                      content is the fixture file, then read back the inbox
#                      and call host.blob.get on the first attachment's blob_id.
#                      Prints: blob_id=blob_<prefix> bytes=<n>
#
#   send [--to <addr>] [--verify auto|dns|admin|none] [--subject <s>]
#                      PRD-mcphost-email-send-live-proof: sign up a throwaway
#                      tenant, verify its owner, send one mail, read the
#                      host.email.sends row back, offboard the tenant on every
#                      exit path. Needs only MCPHOST_URL; never reads
#                      MCPHOST_API_KEY or MCPHOST_NAMESPACE. Stdout, one
#                      key=value group per line:
#                        tenant=<namespace>
#                        verify=<dns|admin|none>
#                        status=sent id=em_<ulid> provider_message_id=<id>
#                        offboarded=<true|false>
#                      or status=<owner_unverified|quota_exceeded|provider_error|error> ...
#                      (provider_error carries error=<the row's error>).
#                      Exit: 0 sent, 1 any other verdict, 2 verify_path_unavailable.
#
# Environment for attach (all required):
#   MCPHOST_URL        base URL of the mcphost instance  (e.g. https://mcphost.dev)
#   MCPHOST_API_KEY    bearer token for the tenant
#   MCPHOST_NAMESPACE  tenant namespace used as the inbound path segment
#
# Environment for attach (optional):
#   MCPHOST_EMAIL_INBOUND_SECRET  sent as X-Mcphost-Inbound-Secret; a caller
#                      outside the fleet's source addresses needs it
#
# Environment for send:
#   MCPHOST_URL                    required
#   MCPHOST_ADMIN_KEY              enables --verify admin (admin.tenant_verify_owner)
#   MCPHOST_PROBE_DNS_ZONE         zone the dns path publishes its TXT record under
#   MCPHOST_PROBE_CF_TOKEN         Cloudflare token (Zone.DNS:Edit on that zone only)
#   MCPHOST_PROBE_CF_API           Cloudflare API base (default https://api.cloudflare.com/client/v4)
#   MCPHOST_PROBE_DNS_POLL_SECS    host.claim.dns.check interval (default 5)
#   MCPHOST_PROBE_DNS_TIMEOUT_SECS give up after this long (default 60)
#
# Example (Live AC8 evidence):
#   set -a; . ~/.config/mcphost/email-probe.env; set +a
#   bash ~/wintermute/mcphost/scripts/email-probe.sh send --to delivered@resend.dev

set -euo pipefail

# ---- send ------------------------------------------------------------------

PROBE_BASE=""
PROBE_KEY=""
PROBE_NS=""
DNS_ZONE_ID=""
DNS_RECORD_ID=""
OFFBOARDED=false

# rpc <step> <auth> <tool> <args-json>: one tools/call. <auth> is `-` (none),
# `tenant:<key>` (the tenant_key argument) or `admin` (bearer MCPHOST_ADMIN_KEY).
# Prints the tool's structured result on stdout; on failure prints
# `<step> <code>` and returns 1.
rpc() {
    local step="$1" auth="$2" tool="$3" args="$4" body resp
    local -a hdr=()
    [[ "$auth" == admin ]] && hdr=(-H "Authorization: Bearer ${MCPHOST_ADMIN_KEY:-}")
    body=$(python3 -c '
import json, sys
tool, args, auth = sys.argv[1], json.loads(sys.argv[2]), sys.argv[3]
if auth.startswith("tenant:"):
    args["tenant_key"] = auth[7:]
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "email-probe", "version": "1"}}}}))
' "$tool" "$args" "$auth") || { echo "$step bad_request"; return 1; }
    resp=$(curl -sS --max-time 30 -X POST "$PROBE_BASE/mcp" \
        -H 'Content-Type: application/json' \
        -H 'Accept: application/json, text/event-stream' \
        -H 'MCP-Protocol-Version: 2026-07-28' \
        -H 'Mcp-Method: tools/call' \
        -H "Mcp-Name: $tool" \
        ${hdr[@]+"${hdr[@]}"} \
        --data-binary "$body" 2>/dev/null) || { echo "$step unreachable"; return 1; }
    printf '%s' "$resp" | python3 -c '
import json, sys
step = sys.argv[1]
raw = sys.stdin.read()
msgs = []
for line in raw.splitlines():
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
def code_of(d):
    data = d.get("data") if isinstance(d, dict) else None
    if isinstance(data, dict) and data.get("error_code"):
        return str(data["error_code"])
    return None
if "error" in msg:
    print(step + " " + (code_of(msg["error"]) or "rpc_error"))
    sys.exit(1)
res = msg["result"]
sc = res.get("structuredContent")
if sc is None:
    try:
        sc = json.loads(res["content"][0]["text"])
    except Exception:
        sc = {}
if res.get("isError"):
    err = sc.get("error_code") or sc.get("code") or sc.get("error") or "tool_error"
    print(step + " " + str(err).replace("\n", " "))
    sys.exit(1)
print(json.dumps(sc))
' "$step"
}

# field <json> <python-expr over d>: prints the value, empty when absent.
field() {
    printf '%s' "$1" | python3 -c '
import json, sys
d = json.load(sys.stdin)
try:
    v = eval(sys.argv[1])
except Exception:
    v = None
print("" if v is None else v)
' "$2"
}

# cf <METHOD> <path> [body]: one Cloudflare API call; prints the response body.
cf() {
    local method="$1" path="$2" body="${3:-}"
    local -a data=()
    [[ -n "$body" ]] && data=(--data-binary "$body")
    curl -sS --max-time 30 -X "$method" \
        -H "Authorization: Bearer ${MCPHOST_PROBE_CF_TOKEN:-}" \
        -H 'Content-Type: application/json' \
        ${data[@]+"${data[@]}"} \
        "${MCPHOST_PROBE_CF_API:-https://api.cloudflare.com/client/v4}${path}"
}

# Runs on EVERY exit path (success, failed step, set -e abort, signal): drops
# the published TXT record, offboards the tenant, prints offboarded=<bool>.
send_cleanup() {
    local rc=$?
    trap - EXIT
    set +e
    if [[ -n "$DNS_RECORD_ID" && -n "$DNS_ZONE_ID" ]]; then
        cf DELETE "/zones/${DNS_ZONE_ID}/dns_records/${DNS_RECORD_ID}" >/dev/null 2>&1
    fi
    if [[ -n "$PROBE_KEY" ]] && rpc offboard "tenant:${PROBE_KEY}" host.self.offboard '{}' >/dev/null 2>&1; then
        OFFBOARDED=true
    fi
    echo "offboarded=${OFFBOARDED}"
    exit "$rc"
}

# send_fail <reason> [exit-code]: the verdict line for a failed step.
send_fail() {
    echo "status=error reason=${1//[$'\n ']/_}"
    exit "${2:-1}"
}

send_step=""
send_run() { send_step=$("$@") || send_fail "$send_step"; }

# The `dns` path: host.claim.dns.start, publish the TXT record, poll
# host.claim.dns.check until the owner is verified.
verify_dns() {
    local slug domain zones rec name value deadline poll
    slug="$(printf '%s' "$PROBE_NS" | tr 'A-Z_' 'a-z-')"
    domain="${slug}.${MCPHOST_PROBE_DNS_ZONE}"
    send_run rpc dns_start "tenant:${PROBE_KEY}" host.claim.dns.start "{\"domain\":\"${domain}\"}"
    rec="$send_step"
    name="$(field "$rec" 'd["record"]')"
    value="$(field "$rec" 'd["value"]')"
    zones="$(cf GET "/zones?name=${MCPHOST_PROBE_DNS_ZONE}")" || send_fail "cf_zone_lookup"
    DNS_ZONE_ID="$(field "$zones" 'd["result"][0]["id"]')"
    [[ -n "$DNS_ZONE_ID" ]] || send_fail "cf_zone_not_found"
    rec="$(cf POST "/zones/${DNS_ZONE_ID}/dns_records" \
        "$(python3 -c 'import json,sys; print(json.dumps({"type":"TXT","name":sys.argv[1],"content":sys.argv[2],"ttl":120}))' "$name" "$value")")" \
        || send_fail "cf_record_create"
    DNS_RECORD_ID="$(field "$rec" 'd["result"]["id"]')"
    [[ -n "$DNS_RECORD_ID" ]] || send_fail "cf_record_create"
    poll="${MCPHOST_PROBE_DNS_POLL_SECS:-5}"
    deadline=$(( $(date +%s) + ${MCPHOST_PROBE_DNS_TIMEOUT_SECS:-60} ))
    while :; do
        if rec="$(rpc dns_check "tenant:${PROBE_KEY}" host.claim.dns.check "{\"domain\":\"${domain}\"}")"; then
            break
        fi
        if (( $(date +%s) >= deadline )); then
            send_fail "dns_record_not_found"
        fi
        sleep "$poll"
    done
}

verify_admin() {
    local slug zone
    slug="$(printf '%s' "$PROBE_NS" | tr 'A-Z_' 'a-z-')"
    zone="${MCPHOST_PROBE_DNS_ZONE:-mcphost.dev}"
    send_run rpc admin_verify admin admin.tenant_verify_owner \
        "{\"tenant\":\"${PROBE_NS}\",\"email\":\"owner+${slug}@${zone}\"}"
}

# Prints the verdict line for the newest host.email.sends row; returns 0 only for `sent`.
send_verdict() {
    printf '%s' "$1" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("sends") or []
if not rows:
    print("status=error reason=no_send_row")
    sys.exit(1)
r = rows[0]
if r["status"] == "sent":
    print("status=sent id=%s provider_message_id=%s" % (r["id"], r.get("provider_message_id", "")))
    sys.exit(0)
line = "status=" + r["status"]
if r.get("error"):
    line += " error=" + r["error"]
print(line)
sys.exit(1)
'
}

cmd_send() {
    shift
    local to="delivered@resend.dev" verify="auto" subject="" eff dns_ok=false methods
    subject="[probe] send $(date +%s)"
    while (( $# )); do
        case "$1" in
            --to) to="${2:?--to needs an address}"; shift 2 ;;
            --verify) verify="${2:?--verify needs auto|dns|admin|none}"; shift 2 ;;
            --subject) subject="${2:?--subject needs a value}"; shift 2 ;;
            *) echo "usage: email-probe.sh send [--to <addr>] [--verify auto|dns|admin|none] [--subject <s>]" >&2; exit 1 ;;
        esac
    done
    case "$verify" in auto|dns|admin|none) ;; *)
        echo "usage: email-probe.sh send [--to <addr>] [--verify auto|dns|admin|none] [--subject <s>]" >&2; exit 1 ;;
    esac
    : "${MCPHOST_URL:?MCPHOST_URL is required}"
    PROBE_BASE="${MCPHOST_URL%/}"
    trap send_cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    send_run rpc signup - signup "{\"name\":\"email-probe-$(date +%s)-$$\",\"handoff\":true}"
    PROBE_NS="$(field "$send_step" 'd["tenant"]')"
    local handoff
    handoff="$(field "$send_step" 'd["handoff_token"]')"
    echo "tenant=${PROBE_NS}"
    send_run rpc redeem - host.redeem "{\"handoff_token\":\"${handoff}\"}"
    PROBE_KEY="$(field "$send_step" 'd["key"]')"
    [[ -n "$PROBE_KEY" ]] || send_fail "redeem bad_response"

    if [[ -n "${MCPHOST_PROBE_DNS_ZONE:-}" && -n "${MCPHOST_PROBE_CF_TOKEN:-}" ]]; then
        methods="$(curl -sS --max-time 15 "${PROBE_BASE}/claim/methods.json" 2>/dev/null || true)"
        [[ -n "$methods" ]] || methods='{"methods":[]}'
        if [[ "$(field "$methods" 'any(m["name"] == "dns" and m.get("available", True) for m in d["methods"])')" == True ]]; then
            dns_ok=true
        fi
    fi
    eff=""
    case "$verify" in
        auto)
            if $dns_ok; then eff=dns
            elif [[ -n "${MCPHOST_ADMIN_KEY:-}" ]]; then eff=admin
            fi ;;
        dns) if $dns_ok; then eff=dns; fi ;;
        admin) if [[ -n "${MCPHOST_ADMIN_KEY:-}" ]]; then eff=admin; fi ;;
        none) eff=none ;;
    esac
    if [[ -z "$eff" ]]; then
        send_fail "verify_path_unavailable" 2
    fi
    echo "verify=${eff}"
    case "$eff" in
        dns) verify_dns ;;
        admin) verify_admin ;;
    esac
    if [[ "$eff" != none ]]; then
        send_run rpc whoami "tenant:${PROBE_KEY}" host.whoami '{}'
        [[ "$(field "$send_step" 'd.get("owner_verified")')" == True ]] || send_fail "owner_not_verified"
    fi

    # The send's own answer is not the receipt: the row prod recorded is.
    rpc send "tenant:${PROBE_KEY}" host.email.send \
        "$(python3 -c 'import json,sys; print(json.dumps({"to":[sys.argv[1]],"subject":sys.argv[2],"text":"email-probe send proof"}))' "$to" "$subject")" >/dev/null || true
    send_run rpc sends "tenant:${PROBE_KEY}" host.email.sends '{"limit":1}'
    local verdict rc=0
    verdict="$(send_verdict "$send_step")" || rc=$?
    echo "$verdict"
    exit "$rc"
}

verb="${1:-}"

case "$verb" in
    send) cmd_send "$@" ;;
    attach) ;;
    *) echo "usage: email-probe.sh attach <fixture> | send [--to <addr>] [--verify auto|dns|admin|none] [--subject <s>]" >&2; exit 1 ;;
esac

fixture="${2:-}"

if [[ -z "$fixture" ]]; then
    echo "usage: email-probe.sh attach <fixture> | send [--to <addr>] [--verify auto|dns|admin|none] [--subject <s>]" >&2
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
