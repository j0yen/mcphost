#!/usr/bin/env python3
"""nmove_ac08_nearest_move.py -- PRD-mcphost-refusals-name-the-nearest-move, AC 7 / AC 8.

Usage: python3 -I tests/live/nmove_ac08_nearest_move.py <MCP URL, e.g. https://mcphost.dev/mcp>

One streamable-HTTP client (Accept: application/json, text/event-stream), User-Agent
`nmove-ac08/<run id>`, never echoing `Mcp-Session-Id`:
  signup {name: synthorg-nmove-ac08, handoff: true} -> host.redeem -> the key rides as
  `tenant_key` on every later call.
  1. host.tool_publish tide_report (python, `def main(args)`); call `tide_reprot` ->
     tool_not_found with did_you_mean[0] == tide_report and next_call.tool == host.tool_call.
  2. call a unique `nmove_<id>` name -> tools_published == 1 and next_call.tool == host.tool_publish.
  3. host.tool_publish a `def run(args)` source -> invalid_spec, exception_class NoMain,
     defs_found == ["run"].
  4. host.whoami with a generated tenant_key matching no tenant -> tenant_key_invalid,
     next_call.tool == signup, the generated value absent from the error body.
  finally: host.self.offboard with the key.
A failed assertion prints the failing step, offboards, and exits 1. No key, namespace or
URL is printed.
Last line: nmove_ac08 tenant=<offboarded|not-offboarded> rc=<0|1|2>
Exit: 0 pass, 1 a check failed, 2 network/protocol error.
"""
import json
import secrets
import sys
import urllib.error
import urllib.request
import uuid

PROTOCOL = "2026-07-28"
SYNTHETIC = "synthorg-nmove-ac08"


class Protocol(Exception):
    """Network or protocol failure (rc 2)."""


class Check(Exception):
    """A checked condition failed (rc 1)."""


class Client:
    def __init__(self, url, user_agent):
        self.url = url
        self.user_agent = user_agent
        self.key = None
        self.n = 0

    def request(self, tool, args, timeout=60):
        """Returns the JSON-RPC message holding result or error."""
        self.n += 1
        meta = {"io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": SYNTHETIC, "version": "1"}}
        params = {"name": tool, "arguments": args, "_meta": meta}
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": "tools/call", "params": params}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": PROTOCOL, "Mcp-Method": "tools/call", "Mcp-Name": tool,
                   "User-Agent": self.user_agent, "x-mcphost-synthetic": SYNTHETIC}
        req = urllib.request.Request(self.url, body, headers)
        status = 200
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            # A refusal rides an HTTP 4xx whose body is the JSON-RPC error.
            status = e.code
            raw = e.read().decode("utf-8", "replace")
        except (urllib.error.URLError, OSError) as e:
            raise Protocol("%s %s" % (tool, type(e).__name__))
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
            raise Protocol("%s HTTP %d, no rpc response" % (tool, status))
        return msg

    def keyed(self, args):
        return dict(args, tenant_key=self.key) if self.key else dict(args)

    def call(self, tool, args):
        """Returns the structured result; an rpc or tool error is a failed check."""
        msg = self.request(tool, args)
        if "error" in msg:
            err = msg["error"] or {}
            data = err.get("data") if isinstance(err.get("data"), dict) else {}
            raise Check("%s rpc error %s %s" % (tool, err.get("code"), data.get("error_code") or "-"))
        res = msg["result"]
        structured = res.get("structuredContent")
        if not isinstance(structured, dict):
            raise Protocol("%s no structuredContent" % tool)
        if res.get("isError"):
            raise Check("%s tool error %s" % (tool, structured.get("error_code") or "-"))
        return structured

    def refused(self, tool, args):
        """Returns (error_code, data, raw body text) of a refused call; a success is a failed check."""
        msg = self.request(tool, args)
        if "error" not in msg:
            res = msg.get("result") or {}
            structured = res.get("structuredContent")
            if res.get("isError") and isinstance(structured, dict):
                return structured.get("error_code"), structured, json.dumps(msg)
            raise Check("%s was expected to be refused" % tool)
        err = msg["error"] or {}
        data = err.get("data") if isinstance(err.get("data"), dict) else {}
        return data.get("error_code"), data, json.dumps(msg)

    def offboard(self):
        if not self.key:
            return False
        try:
            return self.call("host.self.offboard", {"tenant_key": self.key}).get("disabled") is True
        except (Check, Protocol):
            return False


def expect(step, cond, detail=""):
    if not cond:
        raise Check("%s%s" % (step, (": " + detail) if detail else ""))


def run(c):
    signup = c.call("signup", {"name": SYNTHETIC, "handoff": True})
    token = signup.get("handoff_token")
    expect("signup returns a handoff_token", isinstance(token, str) and token)
    redeemed = c.call("host.redeem", {"handoff_token": token})
    key = redeemed.get("key")
    expect("host.redeem returns the key", isinstance(key, str) and key)
    c.key = key

    published = c.call("host.tool.publish", c.keyed({
        "name": "tide_report", "kind": "python",
        "spec": {"source": "def main(args):\n    return {\"port\": args.get(\"port\")}\n",
                 "args_schema": {"type": "object"}}}))
    expect("tide_report is published", published.get("name"))

    # 1. a mistyped name of a published tool
    code, data, _ = c.refused("tide_reprot", c.keyed({"port": "sfo"}))
    expect("tide_reprot is tool_not_found", code == "tool_not_found", str(code))
    expect("did_you_mean[0] is tide_report", (data.get("did_you_mean") or [None])[0] == "tide_report",
           json.dumps(data.get("did_you_mean")))
    nxt = data.get("next_call") or {}
    expect("next_call is host.tool_call", nxt.get("tool") == "host.tool_call", str(nxt.get("tool")))
    expect("next_call names tide_report", (nxt.get("args") or {}).get("name") == "tide_report")

    # 2. a name nothing is near
    unique = "nmove_" + uuid.uuid4().hex[:20]
    code, data, _ = c.refused(unique, c.keyed({}))
    expect("the unique name is tool_not_found", code == "tool_not_found", str(code))
    expect("tools_published is 1", data.get("tools_published") == 1, str(data.get("tools_published")))
    nxt = data.get("next_call") or {}
    expect("next_call is host.tool_publish", nxt.get("tool") == "host.tool_publish", str(nxt.get("tool")))

    # 3. a python source whose entry point is not main
    code, data, _ = c.refused("host.tool.publish", c.keyed({
        "name": "nmove_run_only", "kind": "python",
        "spec": {"source": "def run(args):\n    return args\n", "args_schema": {"type": "object"}}}))
    expect("def run(args) is invalid_spec", code == "invalid_spec", str(code))
    expect("exception_class is NoMain", data.get("exception_class") == "NoMain", str(data.get("exception_class")))
    expect("defs_found is [run]", data.get("defs_found") == ["run"], json.dumps(data.get("defs_found")))

    # 4. a key that matches no tenant
    ghost = "t_" + secrets.token_hex(32)
    code, data, body = c.refused("host.whoami", {"tenant_key": ghost})
    expect("the unknown key is tenant_key_invalid", code == "tenant_key_invalid", str(code))
    expect("next_call is signup", (data.get("next_call") or {}).get("tool") == "signup")
    expect("the generated value is not echoed", ghost not in body and ghost[:8] not in body)


def main(argv):
    if len(argv) != 1:
        print("usage: nmove_ac08_nearest_move.py <mcp-url>", file=sys.stderr)
        return 2
    c = Client(argv[0], "nmove-ac08/" + uuid.uuid4().hex[:8])
    rc = 0
    reason = ""
    try:
        run(c)
    except Check as e:
        rc, reason = 1, str(e)
    except Protocol as e:
        rc, reason = 2, str(e)
    finally:
        off = c.offboard()
    if rc == 0 and not off:
        rc, reason = 1, "self_offboard did not confirm"
    if reason:
        print(reason, file=sys.stderr)
    print("nmove_ac08 tenant=%s rc=%d" % ("offboarded" if off else "not-offboarded", rc))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
