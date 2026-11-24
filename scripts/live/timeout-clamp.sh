#!/usr/bin/env bash
# timeout-clamp.sh -- PRD-mcphost-sandbox-timeout-clamp-live-probe AC6.
#
# One fresh synthetic free tenant publishes the quickstart's python starter with
# spec.timeout_s 45 and expects result.warnings[0] to say the plan clamped it:
# code timeout_clamped, field timeout_s, requested 45, effective 30, plan free.
# The tenant is offboarded on EXIT (host.self.offboard, the canonical verb) and
# the dead key must then be refused by host.whoami with tenant_disabled. The
# tenant key is read into one shell variable and reaches curl only as the
# `header = "Authorization: Bearer ..."` line on stdin (`curl -K -`): never in
# argv, never in a request body, never in stdout/stderr, never exported.
# Needs bash, curl, python3.
#
# Usage: timeout-clamp.sh <base_url>
#        timeout-clamp.sh --selftest   offline: a curl shim serves canned replies
# stdout (success): `status: ok` / `warning: timeout_s 45 -> 30 free` / `offboarded: tenant_disabled`
# stdout (failure): `status: error reason: <step> <code>` (then the offboard receipt, if a tenant existed)
# exit: 0 ok, 1 failure, 2 usage, 3 missing tool.
set -euo pipefail

SELF="${BASH_SOURCE[0]}"
BASE="${1:-}"
BASE="${BASE%/}"
if [ -z "$BASE" ] || [ $# -gt 1 ]; then
  echo "usage: timeout-clamp.sh <base_url> | --selftest" >&2
  exit 2
fi
if [ "$BASE" != "--selftest" ]; then
  for tool in curl python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "timeout-clamp.sh: missing required tool: $tool" >&2
      exit 3
    fi
  done
fi

KEY=""

# rpc <step> <keyed|open> <tool> <args-json>: prints the tool's structured
# result on stdout; on any failure prints `<step> <code>` on stdout, returns 1.
rpc() {
  local step="$1" mode="$2" tool="$3" args="$4" body resp
  body=$(python3 -c '
import json, sys
tool, args = sys.argv[1], json.loads(sys.argv[2])
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "timeout-clamp-live", "version": "1"}}}}))
' "$tool" "$args") || { echo "$step bad_request"; return 1; }
  local -a curl_args=(-sS --max-time 15 -X POST "$BASE/mcp"
    -H 'Content-Type: application/json'
    -H 'Accept: application/json, text/event-stream'
    -H 'MCP-Protocol-Version: 2026-07-28'
    -H 'Mcp-Method: tools/call'
    -H "Mcp-Name: $tool"
    -H 'x-mcphost-synthetic: script:timeout-clamp'
    --data-binary "$body")
  if [ "$mode" = "keyed" ]; then
    # printf is a builtin: the key is not in any argv.
    resp=$(printf 'header = "Authorization: Bearer %s"\n' "$KEY" | curl -K - "${curl_args[@]}" 2>/dev/null) \
      || { echo "$step unreachable"; return 1; }
  else
    resp=$(curl "${curl_args[@]}" </dev/null 2>/dev/null) || { echo "$step unreachable"; return 1; }
  fi
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

# check_warning <publish-result-json>: prints `timeout_s 45 -> 30 free` when
# result.warnings[0] is the expected clamp, else `<code>` naming the miss.
check_warning() {
  printf '%s' "$1" | python3 -c '
import json, sys
d = json.load(sys.stdin)
warnings = d.get("warnings")
if not isinstance(warnings, list) or not warnings:
    print("no_warning")
    sys.exit(1)
w = warnings[0]
want = {"code": "timeout_clamped", "field": "timeout_s", "requested": 45, "effective": 30, "plan": "free"}
if not isinstance(w, dict) or any(w.get(k) != v for k, v in want.items()):
    print("warning_mismatch")
    sys.exit(1)
print("timeout_s %d -> %d %s" % (w["requested"], w["effective"], w["plan"]))
'
}

OK_LINES=""

# finish: the EXIT trap. Offboards the tenant (idempotent on a second call),
# then asserts the dead key is refused with tenant_disabled. The success lines
# print only here, after that receipt is known good.
finish() {
  local rc=$? out code
  trap - EXIT
  set +e
  if [ -n "$KEY" ]; then
    out=$(rpc offboard keyed host.self.offboard '{}') \
      || echo "timeout-clamp.sh: offboard failed: $out" >&2
    out=$(rpc whoami keyed host.whoami '{}')
    if [ $? -eq 0 ]; then
      code="key_still_live"
    else
      code="${out#whoami }"
    fi
    KEY=""
    if [ "$code" = "tenant_disabled" ]; then
      if [ "$rc" -eq 0 ]; then
        printf '%s' "$OK_LINES"
      fi
      echo "offboarded: tenant_disabled"
    else
      echo "status: error reason: whoami $code"
      rc=1
    fi
  fi
  exit "$rc"
}

fail() {
  echo "status: error reason: $1"
  exit 1
}

selftest() {
  local dir="$1" key="fixture-key-$$-7f3a9c" name
  mkdir -p "$dir/bin"
  cat >"$dir/bin/curl" <<'SHIM'
#!/usr/bin/env bash
# curl shim: records each call's argv and stdin, serves the canned reply for
# the Mcp-Name header value.
n=$(( $(cat "$SHIM_DIR/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$SHIM_DIR/count"
printf '%s\n' "$@" >"$SHIM_DIR/call.$n.argv"
cat >"$SHIM_DIR/call.$n.stdin"
name=""
for a in "$@"; do
  case "$a" in [Mm]cp-[Nn]ame:*) name="${a#*: }" ;; esac
done
echo "$name" >>"$SHIM_DIR/calls"
cat "$SHIM_DIR/reply.$name"
SHIM
  chmod +x "$dir/bin/curl"

  python3 - "$dir" "$key" <<'PY'
import json, os, sys
d, key = sys.argv[1], sys.argv[2]
def rpc(sc):
    return json.dumps({"jsonrpc": "2.0", "id": 1, "result": {"structuredContent": sc, "isError": False}})
warn = {"code": "timeout_clamped", "field": "timeout_s", "requested": 45, "ceiling": 30,
        "ceiling_field": "sandbox_cpu_s_max", "effective": 30, "plan": "free", "message": "m"}
publish = {"tested_version": 1, "tool_lint": [], "warnings": [warn]}
dead = {"jsonrpc": "2.0", "id": 1, "error": {"code": -32600, "message": "tenant is disabled",
        "data": {"error_code": "tenant_disabled", "disabled_reason": "self_offboard"}}}
starter = {"starter_tool": {"publish_call": {"arguments": {"name": "starter", "spec": {"source": "def run(): pass"}}}}}
def put(scn, name, text):
    os.makedirs(os.path.join(d, scn), exist_ok=True)
    open(os.path.join(d, scn, "reply." + name), "w").write(text)
sse = lambda body: "event: message\ndata: " + body + "\n\n"
for scn in ("json", "sse", "nowarn", "stillive"):
    frame = sse if scn == "sse" else (lambda b: b)
    body = {k: v for k, v in publish.items() if not (scn == "nowarn" and k == "warnings")}
    put(scn, "signup", frame(rpc({"key": key, "tenant_id": "t_fixture"})))
    put(scn, "host.quickstart", frame(rpc(starter)))
    put(scn, "host.tool_publish", frame(rpc(body)))
    put(scn, "host.self.offboard", frame(rpc({"offboarded": True})))
    put(scn, "host.whoami", frame(rpc({"tenant_id": "t_fixture"}) if scn == "stillive" else json.dumps(dead)))
PY

  # run_scn <scenario>: runs the script's own flow through the shim.
  run_scn() {
    local scn="$1" sd="$dir/run.$1"
    mkdir -p "$sd"
    cp "$dir/$scn"/reply.* "$sd/"
    local rc=0
    SHIM_DIR="$sd" PATH="$dir/bin:$PATH" bash "$SELF" http://selftest.invalid >"$sd/out" 2>"$sd/err" || rc=$?
    echo "$rc" >"$sd/rc"
  }
  local scn sd
  for scn in json sse nowarn stillive; do run_scn "$scn"; done

  for scn in json sse; do
    sd="$dir/run.$scn"
    [ "$(cat "$sd/rc")" = "0" ] || { echo "selftest: $scn rc $(cat "$sd/rc")"; return 1; }
    [ "$(cat "$sd/out")" = $'status: ok\nwarning: timeout_s 45 -> 30 free\noffboarded: tenant_disabled' ] \
      || { echo "selftest: $scn stdout: $(tr '\n' '|' <"$sd/out")"; return 1; }
    [ "$(tr '\n' ' ' <"$sd/calls")" = "signup host.quickstart host.tool_publish host.self.offboard host.whoami " ] \
      || { echo "selftest: $scn calls: $(tr '\n' ' ' <"$sd/calls")"; return 1; }
    # Key hygiene: not in any argv or capture; in stdin of the keyed calls only.
    if grep -qF "$key" "$sd"/call.*.argv "$sd/out" "$sd/err"; then
      echo "selftest: $scn key leaked outside stdin"; return 1
    fi
    [ ! -s "$sd/call.1.stdin" ] || { echo "selftest: $scn signup sent stdin"; return 1; }
    local n
    for n in 2 3 4 5; do
      grep -qF "header = \"Authorization: Bearer $key\"" "$sd/call.$n.stdin" \
        || { echo "selftest: $scn call $n lacks the key on stdin"; return 1; }
      grep -qF 'tenant_key' "$sd/call.$n.argv" && { echo "selftest: $scn call $n body carries tenant_key"; return 1; }
    done
    grep -qxF 'Mcp-Name: host.tool_publish' "$sd/call.3.argv" || { echo "selftest: $scn lacks Mcp-Name"; return 1; }
    grep -qF '"timeout_s": 45' "$sd/call.3.argv" || { echo "selftest: $scn publish lacks timeout_s 45"; return 1; }
  done

  sd="$dir/run.nowarn"
  [ "$(cat "$sd/rc")" = "1" ] || { echo "selftest: nowarn rc $(cat "$sd/rc")"; return 1; }
  [ "$(cat "$sd/out")" = $'status: error reason: publish no_warning\noffboarded: tenant_disabled' ] \
    || { echo "selftest: nowarn stdout: $(tr '\n' '|' <"$sd/out")"; return 1; }
  [ "$(tail -n 2 "$sd/calls" | tr '\n' ' ')" = "host.self.offboard host.whoami " ] \
    || { echo "selftest: nowarn trap did not run"; return 1; }

  sd="$dir/run.stillive"
  [ "$(cat "$sd/rc")" = "1" ] || { echo "selftest: stillive rc $(cat "$sd/rc")"; return 1; }
  [ "$(cat "$sd/out")" = 'status: error reason: whoami key_still_live' ] \
    || { echo "selftest: stillive stdout: $(tr '\n' '|' <"$sd/out")"; return 1; }
  echo "selftest: ok"
}

if [ "$BASE" = "--selftest" ]; then
  command -v python3 >/dev/null 2>&1 || { echo "timeout-clamp.sh: missing required tool: python3" >&2; exit 3; }
  SELFTEST_DIR=$(mktemp -d)
  trap 'rm -rf "$SELFTEST_DIR"' EXIT
  SELF=$(cd "$(dirname "$SELF")" && pwd)/$(basename "$SELF")
  selftest "$SELFTEST_DIR"
  exit $?
fi

trap finish EXIT

step_out=""
run() { step_out=$("$@") || fail "$step_out"; }

run rpc signup open signup '{"name":"timeout-clamp-live","handoff":false}'
KEY=$(field "$step_out" 'd["key"]')
[ -n "$KEY" ] || fail "signup bad_response"

run rpc quickstart keyed host.quickstart '{"kind":"python"}'
PUBLISH_ARGS=$(field "$step_out" 'json.dumps(dict(d["starter_tool"]["publish_call"]["arguments"], spec=dict(d["starter_tool"]["publish_call"]["arguments"]["spec"], timeout_s=45)))')
[ -n "$PUBLISH_ARGS" ] || fail "quickstart bad_response"

run rpc publish keyed host.tool_publish "$PUBLISH_ARGS"
WARNING=$(check_warning "$step_out") || fail "publish $WARNING"

OK_LINES=$'status: ok\n'"warning: $WARNING"$'\n'
exit 0
