#!/usr/bin/env bash
# proof.sh -- runnable proof of "one call turns documents into a named index"
# (PRD-mcphost-index-create-from-sources-b AC 4, Live).
#
#   examples/retrieval/proof.sh --create-check [URL]
#
# Against URL (default https://mcphost.dev/mcp, or $MCPHOST_URL) as the
# joe-test tenant: put two documents, create index `hidx-probe` over them
# with one host.index.create call, poll host.index.status for <= 30 s, then
# delete the index and the documents. The last stdout line is
# `chunks>=2 pending=0` and the exit code is 0 iff the index built.
#
# The tenant key is read from ~/.config/mcphost/joe-test.env (a KEY=value
# file: MCPHOST_KEY, MCPHOST_API_KEY or KEY) and is never printed.
# Requires: bash, python3 (stdlib only).

set -uo pipefail

MODE="${1:-}"
if [[ "$MODE" != "--create-check" ]]; then
  echo "usage: $0 --create-check [URL]" >&2
  exit 2
fi
URL="${2:-${MCPHOST_URL:-https://mcphost.dev/mcp}}"
case "$URL" in
  */mcp) ;;
  *) URL="${URL%/}/mcp" ;;
esac
ENV_FILE="${MCPHOST_JOE_TEST_ENV:-$HOME/.config/mcphost/joe-test.env}"
if [[ ! -r "$ENV_FILE" ]]; then
  echo "proof.sh: cannot read $ENV_FILE" >&2
  exit 2
fi

exec python3 -I - "$URL" "$ENV_FILE" <<'PY'
import json, sys, time, urllib.request

url, env_file = sys.argv[1], sys.argv[2]
key = ""
for line in open(env_file):
    line = line.strip()
    if line.startswith("export "):
        line = line[len("export "):]
    name, _, value = line.partition("=")
    if name in ("MCPHOST_KEY", "MCPHOST_API_KEY", "KEY") and value:
        key = value.strip().strip("'\"")
        break
if not key:
    print("proof.sh: no key in the env file", file=sys.stderr)
    sys.exit(2)

def call(tool, args):
    body = json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": args, "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "retrieval-proof", "version": "1.0.0"},
        }},
    }).encode()
    req = urllib.request.Request(url, data=body, headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2026-07-28",
        "Mcp-Method": "tools/call",
        "Mcp-Name": tool,
        "Authorization": "Bearer " + key,
    })
    raw = urllib.request.urlopen(req, timeout=30).read().decode()
    if raw.lstrip().startswith("data:"):
        datas = [l[5:].strip() for l in raw.splitlines() if l.startswith("data:")]
        raw = datas[-1]
    msg = json.loads(raw)
    if "error" in msg:
        raise RuntimeError(tool + ": " + json.dumps(msg["error"]))
    result = msg.get("result") or {}
    if result.get("structuredContent") is not None:
        return result["structuredContent"]
    content = result.get("content") or []
    return json.loads(content[0]["text"]) if content else None

NAME = "hidx-probe"
DOCS = {
    "hidx-probe/a.md": "Refunds are processed within 14 days of the request.",
    "hidx-probe/b.md": "Shipping is free on orders above fifty dollars.",
}
ok = False
last = {}
try:
    try:
        call("host.index.delete", {"name": NAME})
    except RuntimeError:
        pass
    for name, content in DOCS.items():
        call("host.docs.put", {"name": name, "content": content})
    created = call("host.index.create", {"name": NAME, "sources": [{"kind": "doc", "prefix": "hidx-probe/"}]})
    print("created", json.dumps(created))
    deadline = time.time() + 30
    while time.time() < deadline:
        last = call("host.index.status", {"name": NAME})
        if last["pending"] == 0 and last["chunks"] >= 2:
            ok = True
            break
        time.sleep(1)
finally:
    try:
        call("host.index.delete", {"name": NAME})
    except RuntimeError:
        pass
    for name in DOCS:
        try:
            call("host.docs.delete", {"name": name})
        except RuntimeError:
            pass

print("chunks>=2 pending=0" if ok else "FAIL chunks=%s pending=%s" % (last.get("chunks"), last.get("pending")))
sys.exit(0 if ok else 1)
PY
