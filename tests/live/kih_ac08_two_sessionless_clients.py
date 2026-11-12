#!/usr/bin/env python3
"""kih_ac08_two_sessionless_clients.py -- PRD-mcphost-implicit-signup-key-in-hand, AC 7 / AC 8.

Usage: python3 -I tests/live/kih_ac08_two_sessionless_clients.py <MCP URL, e.g. https://mcphost.dev/mcp>

Two streamable-HTTP clients, one User-Agent `kih-ac08/<run id>`, neither ever
echoing `Mcp-Session-Id` (the claude-agent-sdk shape):
  client A: host.table.create with no tenant_key -> asserts `implicit: true` and
            a non-empty onboarding.tenant_key; then host.table.list with that value
            as tenant_key -> asserts its table is listed.
  client B, within 60 s of A's mint: host.table.create with no tenant_key ->
            asserts `implicit: true` and an onboarding.tenant different from A's.
  finally:  host.self.offboard on both tenants with their keys.
Tenant keys are held in memory only; no key, namespace or URL is printed.
Last line: kih_ac08 A=<offboarded|not-offboarded> B=<offboarded|not-offboarded> rc=<0|1|2>
Exit: 0 pass, 1 a check failed, 2 network/protocol error.
"""
import json
import sys
import time
import urllib.error
import urllib.request
import uuid

PROTOCOL = "2026-07-28"
SYNTHETIC = "synthorg-kih-ac08"
TABLE = "kih_ac08_runs"


class Protocol(Exception):
    """Network or protocol failure (rc 2)."""


class Check(Exception):
    """A checked condition failed (rc 1)."""


class Client:
    """One sessionless client: it never sends or stores an Mcp-Session-Id."""

    def __init__(self, url, user_agent, label):
        self.url = url
        self.user_agent = user_agent
        self.label = label
        self.key = None
        self.n = 0

    def request(self, method, params, name=None, timeout=60):
        """Returns the JSON-RPC message holding result or error."""
        self.n += 1
        meta = {"io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": SYNTHETIC, "version": "1"}}
        params = dict(params, _meta=meta)
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": PROTOCOL, "Mcp-Method": method, "User-Agent": self.user_agent,
                   "x-mcphost-synthetic": SYNTHETIC}
        if name:
            headers["Mcp-Name"] = name
        req = urllib.request.Request(self.url, body, headers)
        label = "%s %s" % (self.label, name or method)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            raise Protocol("%s HTTP %d" % (label, e.code))
        except (urllib.error.URLError, OSError) as e:
            raise Protocol("%s %s" % (label, type(e).__name__))
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
            raise Protocol("%s no rpc response" % label)
        return msg

    def call(self, tool, args):
        """Returns the structured result; a tool or rpc error is a failed check."""
        msg = self.request("tools/call", {"name": tool, "arguments": args}, name=tool)
        if "error" in msg:
            err = msg["error"] or {}
            data = err.get("data") if isinstance(err.get("data"), dict) else {}
            raise Check("%s %s rpc error %s %s" % (self.label, tool, err.get("code"), data.get("error_code") or "-"))
        res = msg["result"]
        structured = res.get("structuredContent")
        if not isinstance(structured, dict):
            raise Protocol("%s %s no structuredContent" % (self.label, tool))
        if res.get("isError"):
            raise Check("%s %s tool error %s" % (self.label, tool, structured.get("error_code") or structured.get("code") or "-"))
        return structured

    def mint(self):
        """A key-less first call: returns (namespace, tenant_key) from the envelope."""
        out = self.call("host.table.create", {"name": TABLE, "columns": {"id": "integer"}})
        if out.get("implicit") is not True:
            raise Check("%s first call is not marked implicit: true" % self.label)
        onboarding = out.get("onboarding")
        if not isinstance(onboarding, dict):
            raise Check("%s first call carries no onboarding" % self.label)
        namespace = onboarding.get("tenant")
        key = onboarding.get("tenant_key")
        if not isinstance(namespace, str) or not namespace:
            raise Check("%s onboarding.tenant is empty" % self.label)
        if not isinstance(key, str) or not key:
            raise Check("%s onboarding.tenant_key is empty" % self.label)
        self.key = key
        return namespace

    def offboard(self):
        if not self.key:
            return False
        try:
            off = self.call("host.self.offboard", {"tenant_key": self.key})
        except (Check, Protocol):
            return False
        return off.get("disabled") is True


def run(a, b):
    a_namespace = a.mint()
    minted_at = time.monotonic()
    listed = a.call("host.table.list", {"tenant_key": a.key})
    tables = listed.get("tables")
    if not isinstance(tables, list) or not any(isinstance(t, dict) and t.get("name") == TABLE for t in tables):
        raise Check("A's table is not listed under A's key")

    if time.monotonic() - minted_at > 60:
        raise Check("B did not start within 60 s of A's mint")
    b_namespace = b.mint()
    if b_namespace == a_namespace:
        raise Check("B was given A's tenant")


def main(argv):
    if len(argv) != 1:
        print("usage: kih_ac08_two_sessionless_clients.py <mcp-url>", file=sys.stderr)
        return 2
    agent = "kih-ac08/" + uuid.uuid4().hex[:8]
    a = Client(argv[0], agent, "A")
    b = Client(argv[0], agent, "B")
    rc = 0
    reason = ""
    try:
        run(a, b)
    except Check as e:
        rc, reason = 1, str(e)
    except Protocol as e:
        rc, reason = 2, str(e)
    finally:
        a_off = a.offboard()
        b_off = b.offboard()
    if rc == 0 and not (a_off and b_off):
        rc, reason = 1, "self_offboard did not confirm"
    if reason:
        print(reason, file=sys.stderr)
    print("kih_ac08 A=%s B=%s rc=%d" % ("offboarded" if a_off else "not-offboarded",
                                         "offboarded" if b_off else "not-offboarded", rc))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
