#!/usr/bin/env python3
"""tdisc_live_receipts_rank.py -- PRD-mcphost-trusted-discovery-catalog-receipts, AC 7 / AC 8.

Usage: python3 -I tests/live/tdisc_live_receipts_rank.py <MCP URL, e.g. https://mcphost.dev/mcp>

Two synthetic tenants (header x-mcphost-synthetic + clientInfo synthorg-tdisc-live),
each signed up with handoff: true and redeemed with host.redeem:
  A publishes two python tools under one unique prefix, shares both public, republishes
    the second (so only the first has a verdict for its current version) and runs
    host.tool_test on the first;
  B calls A's first tool once, then host.catalog.search {q: prefix, include_synthetic: true}
    must list the tested, called tool first (test_verdict pass, calls_30d n >= 1 ok >= 1) and
    the other second (test_verdict null, calls_30d n 0);
  GET <base>/.well-known/mcp/catalog.json must list neither tool and say
    view.include_synthetic false;
  both tenants are offboarded (host.self_offboard) in a finally.
Tenant keys are held in memory only; they are never printed, logged or written.
Last line: tdisc_live first=tested receipts=ok hidden=2 view=real offboarded rc=0
On a failed check or a network/protocol error the same line carries what was reached
(first=..., receipts=..., hidden=<n>, view=..., offboarded|not-offboarded) and rc=<1|2>.
Exit: 0 pass, 1 a check failed, 2 network/protocol error (the exit code is the rc).
"""
import json
import sys
import time
import urllib.error
import urllib.request
import uuid

PROTOCOL = "2026-07-28"
SYNTHETIC = "synthorg-tdisc-live"
SOURCE = 'def main(args):\n    return {"ok": True}\n'


class Protocol(Exception):
    """Network or protocol failure (rc 2)."""


class Check(Exception):
    """A checked condition failed (rc 1)."""


class Rpc:
    def __init__(self, url):
        self.url = url
        self.key = None
        self.n = 0

    def request(self, method, params, name=None, synthetic=False, timeout=90):
        """Returns the JSON-RPC message holding result or error."""
        self.n += 1
        meta = {"io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": SYNTHETIC, "version": "1"}}
        params = dict(params, _meta=meta)
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": PROTOCOL, "Mcp-Method": method}
        if name:
            headers["Mcp-Name"] = name
        if synthetic:
            headers["x-mcphost-synthetic"] = SYNTHETIC
        if self.key:
            headers["Authorization"] = "Bearer " + self.key
        req = urllib.request.Request(self.url, body, headers)
        label = name or method
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

    def call(self, tool, args, synthetic=False):
        """Returns the structured result; a tool or rpc error is a failed check."""
        msg = self.request("tools/call", {"name": tool, "arguments": args}, name=tool, synthetic=synthetic)
        if "error" in msg:
            raise Check("%s rpc error %s" % (tool, (msg["error"] or {}).get("code")))
        res = msg["result"]
        structured = res.get("structuredContent")
        if not isinstance(structured, dict):
            raise Protocol("%s no structuredContent" % tool)
        if res.get("isError"):
            raise Check("%s tool error %s" % (tool, structured.get("error_code") or structured.get("code") or "-"))
        return structured

    def call_ready(self, tool, args, wait=120):
        """Like call(), but a python tool still building its environment is retried."""
        deadline = time.time() + wait
        while True:
            out = self.call(tool, args)
            if out.get("status") != "building":
                return out
            if time.time() >= deadline:
                raise Check("%s still building after %d s" % (tool, wait))
            time.sleep(max(0.25, min((out.get("retry_after_ms") or 1000) / 1000.0, 5)))


def base_of(url):
    """<base> of <base>/mcp."""
    if not url.endswith("/mcp"):
        raise Check("argument must end in /mcp")
    return url[:-len("/mcp")]


def get_json(url, timeout=60):
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers={"Accept": "application/json"}), timeout=timeout) as resp:
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        raise Protocol("GET %s HTTP %d" % (url.rsplit("/", 1)[-1], e.code))
    except (urllib.error.URLError, OSError) as e:
        raise Protocol("GET %s %s" % (url.rsplit("/", 1)[-1], type(e).__name__))
    except ValueError:
        raise Protocol("GET %s not json" % url.rsplit("/", 1)[-1])


def sign_up(url, label):
    """handoff signup + host.redeem; returns (rpc holding the key, namespace)."""
    anon = Rpc(url)
    out = anon.call("signup", {"name": label, "handoff": True}, synthetic=True)
    token, namespace = out.get("handoff_token"), out.get("tenant")
    if not isinstance(token, str) or not isinstance(namespace, str):
        raise Check("signup returned no handoff_token")
    redeemed = anon.call("host.redeem", {"handoff_token": token})
    del out, token
    rpc = Rpc(url)
    if not isinstance(redeemed.get("key"), str):
        raise Check("host.redeem returned no key")
    rpc.key = redeemed["key"]
    del redeemed
    return rpc, namespace


def publish_python(rpc, name):
    rpc.call("host.tool_publish", {"name": name, "kind": "python",
                                   "spec": {"source": SOURCE, "args_schema": {"type": "object"}}})


def by_name(tools, name):
    return next((t for t in tools if isinstance(t, dict) and t.get("name") == name), None)


def run(urls, state):
    url, base = urls
    run_id = uuid.uuid4().hex[:12]
    prefix = "tdl" + run_id[:10]
    first, second = prefix + "_first", prefix + "_second"

    a, ns_a = sign_up(url, "synthorg-tdisc-live-a-%s" % run_id)
    state["tenants"].append(a)
    b, ns_b = sign_up(url, "synthorg-tdisc-live-b-%s" % run_id)
    state["tenants"].append(b)

    for name in (first, second):
        publish_python(a, name)
    for name in (first, second):
        a.call_ready("host.tool_share", {"name": name, "visibility": "public",
                                         "description": "tdisc live %s" % name})
    # A new version stays public but has no verdict for its current version: it ranks untested.
    publish_python(a, second)
    tested = a.call_ready("host.tool_test", {"name": first, "args": {}})
    if tested.get("verdict") != "pass":
        raise Check("host.tool_test of the first tool is %s, not pass" % tested.get("verdict"))

    full_first, full_second = "%s.%s" % (ns_a, first), "%s.%s" % (ns_a, second)
    called = b.call_ready(full_first, {})
    if called.get("ok") is not True:
        raise Check("B's call of the first tool did not return ok")

    found = b.call("host.catalog.search", {"q": prefix, "include_synthetic": True, "limit": 20})
    tools = found.get("tools")
    if not isinstance(tools, list):
        raise Check("catalog.search returned no tools list")
    names = [t.get("name") for t in tools if isinstance(t, dict)]
    if names != [full_first, full_second]:
        raise Check("catalog.search order is %s, not tested-first" % len(names))
    state["first"] = "tested"
    top, other = by_name(tools, full_first), by_name(tools, full_second)
    calls = top.get("calls_30d") or {}
    if top.get("test_verdict") != "pass" or not (calls.get("n", 0) >= 1 and calls.get("ok", 0) >= 1):
        raise Check("the first tool's receipts are not pass with n >= 1, ok >= 1")
    other_calls = other.get("calls_30d") or {}
    if other.get("test_verdict") is not None or other_calls.get("n") != 0:
        raise Check("the second tool's receipts are not null with n 0")
    state["receipts"] = "ok"
    view = found.get("view") or {}
    if view.get("include_synthetic") is not True:
        raise Check("catalog.search view does not say include_synthetic true")

    doc = get_json(base + "/.well-known/mcp/catalog.json")
    listed = [t.get("name") for t in (doc.get("tools") or []) if isinstance(t, dict)]
    state["hidden"] = sum(1 for n in (full_first, full_second) if n not in listed)
    if state["hidden"] != 2:
        raise Check("the well-known catalog lists %d of A's synthetic tools" % (2 - state["hidden"]))
    if (doc.get("view") or {}).get("include_synthetic") is not False:
        raise Check("the well-known catalog does not say view.include_synthetic false")
    state["view"] = "real"


def offboard(rpc):
    try:
        return rpc.call("host.self_offboard", {}).get("disabled") is True
    except (Check, Protocol):
        return False


def main(argv):
    if len(argv) != 1:
        print("usage: tdisc_live_receipts_rank.py <mcp-url>", file=sys.stderr)
        return 2
    url = argv[0]
    state = {"first": "untested", "receipts": "missing", "hidden": 0, "view": "unchecked", "tenants": []}
    rc, reason = 0, ""
    try:
        run((url, base_of(url)), state)
    except Check as e:
        rc, reason = 1, str(e)
    except Protocol as e:
        rc, reason = 2, str(e)
    finally:
        offboarded = bool(state["tenants"])
        for rpc in state["tenants"]:
            if not offboard(rpc):
                offboarded = False
    if rc == 0 and not offboarded:
        rc, reason = 1, "host.self_offboard did not confirm for both tenants"
    if reason:
        print(reason, file=sys.stderr)
    print("tdisc_live first=%s receipts=%s hidden=%d view=%s %s rc=%d" % (
        state["first"], state["receipts"], state["hidden"], state["view"],
        "offboarded" if offboarded else "not-offboarded", rc))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
