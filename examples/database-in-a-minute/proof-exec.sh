#!/usr/bin/env bash
# proof-exec.sh -- runnable proof that an agent changes one row in SQL
# (PRD-mcphost-table-sql-exec-and-grammar AC8, Live).
#
#   examples/database-in-a-minute/proof.sh --exec-check [URL]
#
# Against URL (default https://mcphost.dev/mcp, or $MCPHOST_URL) as the
# joe-test tenant: declare the table `exec_check`, seed one row, run ONE
# `host.table.exec` UPDATE of that row through the MCP endpoint, then read
# `host.table.query_log(limit=1)`. Passes iff the UPDATE answered
# `changed: 1` and the newest log row is that statement with
# `statement_kind: update` and `changed: 1`. The table is dropped at the end.
# The last stdout line is `changed=1 statement_kind=update` on a pass.
#
# The tenant key is read from ~/.config/mcphost/joe-test.env (a KEY=value
# file: MCPHOST_KEY, MCPHOST_API_KEY or KEY; override the path with
# MCPHOST_JOE_TEST_ENV) and is never printed.
# Requires: bash, python3 (stdlib only).
#
# MCPHOST_PROVE_DRY=1: print the plan and exit 0 before any network call.

set -uo pipefail

if [[ "${1:-}" != "--exec-check" ]]; then
  echo "usage: $0 --exec-check [URL]" >&2
  exit 2
fi
URL="${2:-${MCPHOST_URL:-https://mcphost.dev/mcp}}"
case "$URL" in
  */mcp) ;;
  *) URL="${URL%/}/mcp" ;;
esac
[[ -n "${MCPHOST_PROVE_DRY:-}" ]] && { echo "plan: database-in-a-minute exec-check against $URL"; exit 0; }
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
            "io.modelcontextprotocol/clientInfo": {"name": "database-in-a-minute-exec-check", "version": "1.0.0"},
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

TABLE = "exec_check"
STAMP = int(time.time())
UPDATE = "UPDATE %s SET a = %d WHERE id = 1" % (TABLE, STAMP)
ok = False
detail = "no result"
try:
    try:
        call("host.table.create", {"name": TABLE, "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"})
    except RuntimeError as e:
        if "table_already_exists" not in str(e):
            raise
    call("host.table.exec", {"sql": "INSERT OR REPLACE INTO %s (id, a) VALUES (1, 0)" % TABLE})
    updated = call("host.table.exec", {"sql": UPDATE})
    log = call("host.table.query_log", {"limit": 1})
    row = (log.get("rows") or [{}])[0]
    ok = (
        updated.get("changed") == 1
        and updated.get("table") == TABLE
        and row.get("sql") == UPDATE
        and row.get("statement_kind") == "update"
        and row.get("changed") == 1
    )
    detail = "exec=%s log=%s" % (json.dumps(updated), json.dumps({k: row.get(k) for k in ("sql", "statement_kind", "changed", "error_code")}))
finally:
    try:
        call("host.table.drop", {"name": TABLE})
    except RuntimeError:
        pass

print("changed=1 statement_kind=update" if ok else "FAIL " + detail)
sys.exit(0 if ok else 1)
PY
