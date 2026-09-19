#!/usr/bin/env bash
# proof.sh -- runnable end-to-end proof of "Share a tool, not a key"
# (see www/llms.txt). Two fresh tenants against $MCPHOST_URL: the owner
# publishes an http-kind tool that injects a secret into a request header
# to a paid-API stand-in, shares it to a group, and adds the caller. The
# caller calls the tool and gets the upstream's response, never the
# secret; the caller's `host.secret_list` is empty; `host.group.remove`
# revokes the caller on its next call while the owner keeps working.
#
# Requires: bash, curl, python3. Deliberately has no jq dependency.
#
# Env vars:
#   MCPHOST_URL   the endpoint to run the recipe against.
#                 Default: https://mcphost.dev/mcp
#   UPSTREAM_URL  URL of a paid-API stand-in the owner's tool calls. If
#                 unset, this script starts its own local echo server on
#                 127.0.0.1 and uses that -- which only works when
#                 MCPHOST_URL's host permits a loopback http upstream
#                 (e.g. a locally-run mcphost in test mode). Against real
#                 mcphost.dev, production SSRF policy refuses loopback and
#                 you must pass a public UPSTREAM_URL of your own.
#
# This script never prints the secret value it generates, in any check
# output, log line, or error message.
#
# Exit code 0 iff every check below passes.

set -uo pipefail
cd "$(dirname "$0")"

MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
FAILURES=0
START_EPOCH=$(python3 -c 'import time; print(time.time())')

MOCK_PID=""
MOCK_PORT_FILE=""
MOCK_LOG_FILE=""

cleanup() {
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" >/dev/null 2>&1
  [[ -n "$MOCK_PORT_FILE" ]] && rm -f "$MOCK_PORT_FILE"
  [[ -n "$MOCK_LOG_FILE" ]] && rm -f "$MOCK_LOG_FILE"
}
trap cleanup EXIT

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
                "name": "share-a-tool-proof",
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

# Reads a raw JSON-RPC response as $1 and prints its structured call
# result, or {"__error__": {...}} if the call errored.
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

# json_str <json> <dotted.path> -- prints the raw string at that path, or
# empty if missing/not-a-string.
json_str() {
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

contains_str() {
  # contains_str <haystack> <needle> -- exit 0 if needle is a substring
  python3 - "$1" "$2" <<'PY'
import sys
sys.exit(0 if sys.argv[2] in sys.argv[1] else 1)
PY
}

# ---- mock upstream (paid-API stand-in) -------------------------------------
# Logs the headers it actually received (proves real delegation) but never
# reflects them back in its response body (the caller must never see the
# secret via the tool's own response).

start_mock_upstream() {
  MOCK_LOG_FILE=$(mktemp)
  MOCK_PORT_FILE=$(mktemp)
  python3 - "$MOCK_PORT_FILE" "$MOCK_LOG_FILE" > /dev/null 2>&1 <<'PY' &
import http.server, json, sys

port_file, log_file = sys.argv[1], sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def _handle(self):
        headers = {k.lower(): v for k, v in self.headers.items()}
        with open(log_file, "a") as f:
            f.write(json.dumps({"path": self.path, "headers": headers}) + "\n")
        body = json.dumps({"upstream": "share-a-tool-mock", "ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self._handle()

    def do_POST(self):
        self._handle()

    def log_message(self, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
  MOCK_PID=$!
  for _ in $(seq 1 50); do
    [[ -s "$MOCK_PORT_FILE" ]] && break
    sleep 0.1
  done
  MOCK_UPSTREAM_URL="http://127.0.0.1:$(cat "$MOCK_PORT_FILE")/paid-api"
}

# Sets MOCK_PID/MOCK_LOG_FILE/MOCK_PORT_FILE/MOCK_UPSTREAM_URL directly
# rather than returning via a `$(...)` command substitution -- that forks
# a subshell, so those assignments would never reach this shell (the
# cleanup trap would then kill nothing and leak the server process, and
# the log-file checks below would read from an empty path).
MOCK_LOCAL=0
if [[ -z "${UPSTREAM_URL:-}" ]]; then
  start_mock_upstream
  UPSTREAM_URL="$MOCK_UPSTREAM_URL"
  MOCK_LOCAL=1
fi

# ---- the recipe -------------------------------------------------------------

SECRET_VALUE="sk_test_$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
SYNTHETIC_HEADER="recipe:share-a-tool"

# 1. Two fresh tenants, both tagged so this proof's own signups are
#    distinguishable from real recipe traffic (AC5).
owner_resp=$(mcp_call "signup" '{"name": "share-a-tool-owner"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
owner_struct=$(structured_of "$owner_resp")
OWNER_NS=$(json_str "$owner_struct" "tenant")
OWNER_KEY=$(json_str "$owner_struct" "key")

caller_resp=$(mcp_call "signup" '{"name": "share-a-tool-caller"}' "" "x-mcphost-synthetic" "$SYNTHETIC_HEADER")
caller_struct=$(structured_of "$caller_resp")
CALLER_NS=$(json_str "$caller_struct" "tenant")
CALLER_KEY=$(json_str "$caller_struct" "key")

check "both_tenants_signed_up" "$([[ -n "$OWNER_KEY" && -n "$CALLER_KEY" ]] && echo 1 || echo 0)"
echo "OWNER_NS=${OWNER_NS}"
echo "CALLER_NS=${CALLER_NS}"

# 2. Store the key once, encrypted, never returned.
secret_args=$(python3 -c 'import json,sys; print(json.dumps({"name": "upstream_key", "value": sys.argv[1]}))' "$SECRET_VALUE")
secret_resp=$(mcp_call "host.secret_set" "$secret_args" "$OWNER_KEY")
check "owner_secret_set" "$(has_error "$secret_resp" && echo 0 || echo 1)"

# 3. Publish the wrapper.
publish_args=$(python3 -c 'import json,sys; print(json.dumps({"name": "paid_api", "kind": "http", "spec": {"method": "GET", "url": sys.argv[1], "headers": {"Authorization": "Bearer {{ secret.upstream_key }}"}, "response": "json"}}))' "$UPSTREAM_URL")
publish_resp=$(mcp_call "host.tool_publish" "$publish_args" "$OWNER_KEY")
publish_struct=$(structured_of "$publish_resp")
QUALIFIED=$(json_str "$publish_struct" "name")
check "owner_tool_publish" "$([[ -n "$QUALIFIED" ]] && echo 1 || echo 0)"

# 4. Owner's own test call: proves the secret really reaches the upstream
#    (checked against the mock's own request log, never mcphost's
#    response) before anything is shared.
owner_test_resp=$(mcp_call "$QUALIFIED" '{}' "$OWNER_KEY")
check "owner_test_call_succeeds" "$(has_error "$owner_test_resp" && echo 0 || echo 1)"
if [[ "$MOCK_LOCAL" == "1" ]]; then
  check "owner_test_call_delivers_real_secret_to_upstream" "$(grep -qF "Bearer ${SECRET_VALUE}" "$MOCK_LOG_FILE" && echo 1 || echo 0)"
fi

# 5. Create a group for the team.
group_resp=$(mcp_call "host.group.create" '{"name": "shared_tool_team"}' "$OWNER_KEY")
check "owner_group_create" "$(has_error "$group_resp" && echo 0 || echo 1)"

# 6. Share the tool to that group.
share_resp=$(mcp_call "host.tool_share" '{"name": "paid_api", "visibility": "group", "group": "shared_tool_team"}' "$OWNER_KEY")
check "owner_tool_share" "$(has_error "$share_resp" && echo 0 || echo 1)"

# 7. Add the caller's tenant.
add_args=$(python3 -c 'import json,sys; print(json.dumps({"name": "shared_tool_team", "namespace": sys.argv[1]}))' "$CALLER_NS")
add_resp=$(mcp_call "host.group.add" "$add_args" "$OWNER_KEY")
check "owner_group_add_caller" "$(has_error "$add_resp" && echo 0 || echo 1)"

# 8. The caller calls the shared tool, no key of its own -- AC2.
caller_call_resp=$(mcp_call "$QUALIFIED" '{}' "$CALLER_KEY")
caller_call_ok=0
if ! has_error "$caller_call_resp"; then
  if [[ "$MOCK_LOCAL" == "0" ]] || contains_str "$caller_call_resp" "share-a-tool-mock"; then
    caller_call_ok=1
  fi
fi
check "caller_tool_call_returns_upstream_response" "$caller_call_ok"

# 9. The caller sees no secrets of the owner's -- AC3.
secret_list_resp=$(mcp_call "host.secret_list" '{}' "$CALLER_KEY")
secret_list_struct=$(structured_of "$secret_list_resp")
names_len=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(len(d.get("names") or []))' "$secret_list_struct")
check "caller_secret_list_empty" "$([[ "$names_len" == "0" ]] && echo 1 || echo 0)"

# 10. No response body the caller received ever carries the raw secret.
leak_free=1
contains_str "$caller_call_resp" "$SECRET_VALUE" && leak_free=0
contains_str "$secret_list_resp" "$SECRET_VALUE" && leak_free=0
check "no_secret_leak_in_caller_responses" "$leak_free"

# 11. Revoke the caller.
remove_resp=$(mcp_call "host.group.remove" "$add_args" "$OWNER_KEY")
check "owner_group_remove_caller" "$(has_error "$remove_resp" && echo 0 || echo 1)"

# 12. The caller's next call fails -- AC4.
caller_call_after_remove=$(mcp_call "$QUALIFIED" '{}' "$CALLER_KEY")
check "group_remove_causes_permission_error" "$(has_error "$caller_call_after_remove" && echo 1 || echo 0)"

# 13. The owner's own call still succeeds -- AC4.
owner_call_after_remove=$(mcp_call "$QUALIFIED" '{}' "$OWNER_KEY")
check "owner_call_still_succeeds_after_remove" "$(has_error "$owner_call_after_remove" && echo 0 || echo 1)"

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
