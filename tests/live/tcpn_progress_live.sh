#!/usr/bin/env bash
# tcpn_progress_live.sh -- PRD-mcphost-tools-call-progress-notifications AC7.
#
# A fresh tenant signs up with the synthetic header, calls
# host.probe.progress {steps: 3, step_ms: 1000} with _meta.progressToken, and
# the script checks that 3 notifications/progress frames precede the result,
# the first within 1500 ms of the request start, that /protocol/frames.json
# lists tools/call:with_progress_token, and that admin healthz's
# progress_7d.calls_with_token.synthetic rises by one within 70 s
# (HEALTHZ_CACHE_TTL_SECS 60 + 10). The tenant is offboarded on EXIT.
# Admin healthz is read with $MCPHOST_ADMIN_KEY, else with the key sourced
# from /etc/mcphost/env (run this on mcphost-1); the key is never printed.
# Needs bash, curl, python3.
#
# Usage: MCPHOST_URL=https://mcphost.dev/mcp tcpn_progress_live.sh
# stdout (success): `first_frame_ms=<n> frames=3 contract=present`
# stdout (failure): `status: error reason: <step>`
# exit: 0 ok, 1 failure, 2 first frame late (check flush_interval in
# /etc/caddy/sites/mcphost.caddy), 3 counter did not rise within the 60 s
# HEALTHZ_CACHE_TTL_SECS + 10 s bound, 64 usage or missing tool.
set -euo pipefail

URL="${MCPHOST_URL:-}"
if [ -z "$URL" ]; then
  echo "usage: MCPHOST_URL=<https://host/mcp> tcpn_progress_live.sh" >&2
  exit 64
fi
BASE="${URL%/}"
BASE="${BASE%/mcp}"
for tool in curl python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "tcpn_progress_live.sh: missing required tool: $tool" >&2; exit 64; }
done

KEY=""
ADMIN_KEY="${MCPHOST_ADMIN_KEY:-}"
if [ -z "$ADMIN_KEY" ] && [ -r /etc/mcphost/env ]; then
  ADMIN_KEY=$(set -a; . /etc/mcphost/env >/dev/null 2>&1; printf '%s' "${MCPHOST_ADMIN_KEY:-}")
fi

fail() {
  echo "status: error reason: $1"
  exit 1
}

# rpc <tool> <args-json> [extra-header]: the tool's structured result on stdout.
rpc() {
  python3 - "$URL" "$1" "$2" "${3:-}" "$KEY" <<'PY'
import http.client, json, sys, urllib.parse
url, tool, args, extra, key = sys.argv[1:6]
args = json.loads(args)
if key:
    args["tenant_key"] = key
u = urllib.parse.urlparse(url)
conn = (http.client.HTTPSConnection if u.scheme == "https" else http.client.HTTPConnection)(u.netloc, timeout=20)
headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
           "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": tool}
if extra:
    k, v = extra.split(":", 1)
    headers[k] = v.strip()
body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args, "_meta": {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {},
    "io.modelcontextprotocol/clientInfo": {"name": "tcpn-progress-live", "version": "1"}}}}
conn.request("POST", u.path or "/mcp", json.dumps(body), headers)
raw = conn.getresponse().read().decode()
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
if msg is None or "error" in msg or msg["result"].get("isError"):
    print(tool + " failed")
    sys.exit(1)
print(json.dumps(msg["result"].get("structuredContent") or {}))
PY
}

offboard() {
  [ -n "$KEY" ] || return 0
  rpc host.self.offboard '{}' >/dev/null 2>&1 || echo "tcpn_progress_live.sh: offboard failed" >&2
  return 0
}
trap offboard EXIT

# synthetic_with_token <admin_key>: the progress_7d.calls_with_token.synthetic count.
synthetic_with_token() {
  [ -n "$ADMIN_KEY" ] || fail "admin key unavailable (set MCPHOST_ADMIN_KEY or run on mcphost-1)"
  printf 'header = "Authorization: Bearer %s"\n' "$ADMIN_KEY" \
    | curl -sS --max-time 15 -K - "$BASE/healthz" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["progress_7d"]["calls_with_token"].get("synthetic", 0))'
}

SIGNUP=$(rpc signup '{"name":"tcpn-progress-live","handoff":false}' "x-mcphost-synthetic: tcpn-progress-live") || fail "signup"
KEY=$(printf '%s' "$SIGNUP" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("key",""))')
[ -n "$KEY" ] || fail "signup bad_response"

BEFORE=$(synthetic_with_token) || fail "healthz before"

STREAM=$(python3 - "$URL" "$KEY" <<'PY'
import http.client, json, sys, time, urllib.parse
url, key = sys.argv[1:3]
u = urllib.parse.urlparse(url)
conn = (http.client.HTTPSConnection if u.scheme == "https" else http.client.HTTPConnection)(u.netloc, timeout=30)
body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "host.probe.progress",
    "arguments": {"steps": 3, "step_ms": 1000, "tenant_key": key}, "_meta": {"progressToken": "tcpn-live",
    "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {},
    "io.modelcontextprotocol/clientInfo": {"name": "tcpn-progress-live", "version": "1"}}}}
t0 = time.monotonic()
conn.request("POST", u.path or "/mcp", json.dumps(body), {"Content-Type": "application/json",
    "Accept": "application/json, text/event-stream", "MCP-Protocol-Version": "2026-07-28",
    "Mcp-Method": "tools/call", "Mcp-Name": "host.probe.progress"})
resp = conn.getresponse()
first_ms, frames, order = None, 0, []
buf = b""
while True:
    chunk = resp.read1(4096)
    if not chunk:
        break
    buf += chunk
    while b"\n\n" in buf:
        block, buf = buf.split(b"\n\n", 1)
        for line in block.decode().splitlines():
            if not line.startswith("data:"):
                continue
            try:
                m = json.loads(line[5:].strip())
            except ValueError:
                continue
            if m.get("method") == "notifications/progress":
                frames += 1
                order.append("progress")
                if first_ms is None:
                    first_ms = int((time.monotonic() - t0) * 1000)
            elif "result" in m or "error" in m:
                order.append("result")
ok = frames == 3 and order[-1:] == ["result"] and (resp.getheader("Content-Type") or "").startswith("text/event-stream")
print(json.dumps({"first_ms": first_ms, "frames": frames, "ok": ok}))
PY
) || fail "probe call"
FIRST_MS=$(printf '%s' "$STREAM" | python3 -c 'import json,sys; print(json.load(sys.stdin)["first_ms"])')
FRAMES=$(printf '%s' "$STREAM" | python3 -c 'import json,sys; print(json.load(sys.stdin)["frames"])')
STREAM_OK=$(printf '%s' "$STREAM" | python3 -c 'import json,sys; print(json.load(sys.stdin)["ok"])')
[ "$STREAM_OK" = "True" ] || fail "stream frames=$FRAMES (want 3 progress frames then the result as text/event-stream)"
if [ "$FIRST_MS" = "None" ] || [ "$FIRST_MS" -ge 1500 ]; then
  echo "status: error reason: first frame at ${FIRST_MS}ms (>= 1500); check flush_interval in /etc/caddy/sites/mcphost.caddy"
  exit 2
fi

CONTRACT=absent
if curl -sS --max-time 15 "$BASE/protocol/frames.json" | grep -q 'tools/call:with_progress_token'; then
  CONTRACT=present
fi
[ "$CONTRACT" = "present" ] || fail "frames.json lacks tools/call:with_progress_token"

DEADLINE=$(( $(date +%s) + 70 ))
while :; do
  AFTER=$(synthetic_with_token) || fail "healthz after"
  [ "$AFTER" -gt "$BEFORE" ] && break
  if [ "$(date +%s)" -ge "$DEADLINE" ]; then
    echo "status: error reason: calls_with_token.synthetic stayed at $AFTER for 70 s; the admin healthz cache is HEALTHZ_CACHE_TTL_SECS (60 s, src/oauth_stats.rs)"
    exit 3
  fi
  sleep 5
done

echo "first_frame_ms=$FIRST_MS frames=$FRAMES contract=$CONTRACT"
