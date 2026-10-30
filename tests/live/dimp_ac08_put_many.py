#!/usr/bin/env python3
"""dimp_ac08_put_many.py -- PRD-mcphost-docs-import-and-stated-boundary, AC 8.

Usage: python3 -I tests/live/dimp_ac08_put_many.py <MCP URL, e.g. https://mcphost.dev/mcp>

Synthetic tenant (header x-mcphost-synthetic + clientInfo synthorg-dimp-ac08):
  signup -> tools/list (host.docs.put starts with the R3 sentence) ->
  host.docs.put_many x3 -> host.docs.status until every document is indexed ->
  host.docs.search for a token planted in the third document only ->
  self_offboard in a finally.
The tenant key is held in memory only; it is never printed, logged or written.
Last line: dimp_ac08 stored=<n> hits=<n> <offboarded|not-offboarded> rc=<0|1|2>
Exit: 0 pass, 1 a check failed, 2 network/protocol error.
"""
import json
import sys
import time
import urllib.error
import urllib.request
import uuid

PROTOCOL = "2026-07-28"
SYNTHETIC = "synthorg-dimp-ac08"
BOUNDARY = ("Documents are text the host indexes itself; a vector index built elsewhere "
            "(LlamaIndex, Chroma, pgvector) is not importable: export its source documents and put them.")


class Protocol(Exception):
    """Network or protocol failure (rc 2)."""


class Check(Exception):
    """A checked condition failed (rc 1)."""


class Rpc:
    def __init__(self, url):
        self.url = url
        self.key = None
        self.n = 0

    def request(self, method, params, name=None, synthetic=False, timeout=60):
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


def indexed_count(status):
    """host.docs.status has no `indexed` counter: documents are indexed once the
    index block has nothing pending and has caught up to the store watermark."""
    index = status.get("index") or {}
    if index.get("pending_documents") == 0 and (index.get("indexed_watermark") or 0) >= (status.get("watermark") or 0):
        return status.get("documents") or 0
    return 0


def run(rpc, state):
    run_id = uuid.uuid4().hex[:12]
    token = "zq" + uuid.uuid4().hex[:16]  # lexically unique, only in the third document
    names = ["dimp-ac08-%s-%d.md" % (run_id, i) for i in (1, 2, 3)]
    out = rpc.call("signup", {"name": "synthorg-dimp-ac08-%s" % run_id}, synthetic=True)
    if not isinstance(out.get("key"), str):
        raise Check("signup returned no key")
    rpc.key = out["key"]
    del out

    msg = rpc.request("tools/list", {})
    if "error" in msg:
        raise Check("tools/list rpc error")
    tools = {t.get("name"): t for t in (msg["result"].get("tools") or []) if isinstance(t, dict)}
    put = tools.get("host.docs.put") or tools.get("host_docs_put")
    if put is None or BOUNDARY not in (put.get("description") or ""):
        raise Check("host.docs.put description lacks the R3 boundary sentence")
    if not (put["description"].startswith(BOUNDARY)):
        raise Check("host.docs.put description does not start with the R3 boundary sentence")

    docs = [{"name": names[0], "content": "# Alpha\nfirst filler document about onboarding\n"},
            {"name": names[1], "content": "# Beta\nsecond filler document about billing\n"},
            {"name": names[2], "content": "# Gamma\nthird document carrying the marker %s\n" % token}]
    res = rpc.call("host_docs_put_many", {"documents": docs})
    results = res.get("results")
    if not isinstance(results, list):
        raise Check("put_many returned no results list")
    state["stored"] = sum(1 for r in results if isinstance(r, dict) and r.get("status") in ("stored", "replaced"))
    if state["stored"] != 3:
        raise Check("put_many stored %d of 3" % state["stored"])

    deadline = time.time() + 30
    while True:
        if indexed_count(rpc.call("host_docs_status", {})) >= 3:
            break
        if time.time() >= deadline:
            raise Check("host.docs.status never reached indexed >= 3 within 30 s")
        time.sleep(1)

    found = rpc.call("host_docs_search", {"query": token}).get("results")
    if not isinstance(found, list):
        raise Check("search returned no results list")
    state["hits"] = len(found)
    if len(found) != 1:
        raise Check("search for the planted token returned %d hits" % len(found))
    if found[0].get("name") != names[2]:
        raise Check("the one hit is not the document the token was planted in")


def main(argv):
    if len(argv) != 1:
        print("usage: dimp_ac08_put_many.py <mcp-url>", file=sys.stderr)
        return 2
    rpc = Rpc(argv[0])
    state = {"stored": 0, "hits": 0}
    rc = 0
    reason = ""
    offboarded = False
    try:
        run(rpc, state)
    except Check as e:
        rc, reason = 1, str(e)
    except Protocol as e:
        rc, reason = 2, str(e)
    finally:
        if rpc.key:
            try:
                off = rpc.call("host_self_offboard", {})
                offboarded = off.get("disabled") is True
            except (Check, Protocol):
                offboarded = False
    if not offboarded and rc == 0:
        rc, reason = 1, "self_offboard did not confirm"
    if reason:
        print(reason, file=sys.stderr)
    print("dimp_ac08 stored=%d hits=%d %s rc=%d" % (
        state["stored"], state["hits"], "offboarded" if offboarded else "not-offboarded", rc))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
