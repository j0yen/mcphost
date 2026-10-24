#!/usr/bin/env python3
"""osreach_import_reach.py -- PRD-mcphost-import-dry-run-live-real-export.

A live client previews a REAL export through `host_import` dry run. The `host.import`
verb looks its `run_id` up, so a placeholder ("x") is schema-valid but not
state-valid; this driver creates the id it passes:

  1. source signup (synthetic tenant, client_name `synthorg-...`)
  2. state set (one small key)
  3. export, 4. wait (up to 300 s) for `done`
  5. destination signup (fresh, empty tenant)
  6. Claude Code `host_import` dry run with the export's run_id, then `host_doctor`
  7. key grep on the stream (any match -> the stream is shredded)
  8. offboard both tenants (`host.self_offboard`) on every exit path

Usage: osreach_import_reach.py --plan
       osreach_import_reach.py --parse STREAM.jsonl      (offline parser, prints JSON)
       osreach_import_reach.py [--base-url URL] [--host-note NOTE] [--deployed-sha SHA]
                               [--attempt N] [--results FILE]
Evidence: one JSON line per attempt appended to
~/.local/state/mcphost-live/osreach-ac06b-results.jsonl.
No key, claim URL or key-bearing header is printed or logged; keys live only in a
0600 file under this run's own mktemp -d root.
Exit: 0 pass (or --plan/--parse ok), 1 fail, 2 usage, 3 stop (missing tool, old
client, refusal).
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

DEFAULT_BASE = "https://mcphost.dev"
RESULTS = os.path.expanduser("~/.local/state/mcphost-live/osreach-ac06b-results.jsonl")
MIN_CLAUDE = (2, 1, 293)
PROTOCOL = "2026-07-28"
WAIT_SECS = 300
MISMATCH = "Structured content does not match"
JUDGED = ("host_import", "host_doctor")

PLAN = [
    "source signup",
    "state set",
    "export",
    "wait",
    "destination signup",
    "Claude Code host_import dry run",
    "host_doctor",
    "key grep on the stream",
    "offboard both",
]
EVIDENCE_TEMPLATE = (
    "tool_results=<n> mismatch=<n> is_error=<n> export_run_id=<id> "
    "unreached=<n> cap=<m> offboarded=<n>"
)


def canonical(name):
    """`mcp__server__host_import` / `host.import` -> `host_import`."""
    return name.rsplit("__", 1)[-1].replace(".", "_")


def _text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict))
    return json.dumps(content)


def _number(text, field):
    m = re.search(r'\b%s\b"?\s*[:=]\s*(\d+)' % re.escape(field), text)
    return int(m.group(1)) if m else None


def parse_stream(lines):
    """Count only `tool_result` blocks paired by `tool_use_id` with a judged tool."""
    uses = {}
    results = []
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        content = (msg.get("message") or {}).get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                uses[block.get("id")] = canonical(block.get("name", ""))
            elif block.get("type") == "tool_result":
                results.append(block)
    out = {"tool_results": 0, "mismatch": 0, "is_error": 0, "unreached": None, "cap": None}
    seen = set()
    for block in results:
        tool = uses.get(block.get("tool_use_id"))
        if tool not in JUDGED:
            continue
        out["tool_results"] += 1
        seen.add(tool)
        text = _text(block.get("content"))
        if MISMATCH in text:
            out["mismatch"] += 1
        if block.get("is_error"):
            out["is_error"] += 1
        if tool == "host_doctor":
            out["unreached"] = _number(text, "unreached")
            out["cap"] = _number(text, "unreached_cap")
    ok = out["tool_results"] == 2 and out["mismatch"] == 0 and out["is_error"] == 0 and len(seen) == 2
    out["result"] = "pass" if ok else "fail"
    if not ok:
        out["reason"] = "tool_results=%d mismatch=%d is_error=%d" % (
            out["tool_results"], out["mismatch"], out["is_error"])
    return out


def leaks(text, secrets):
    """True when the stream holds a key, a claim URL or a key-bearing header."""
    if re.search(r"authorization:\s*bearer", text, re.I) or re.search(r"/claim[/?]", text):
        return True
    return any(s and s in text for s in secrets)


class Refused(Exception):
    pass


class Rpc:
    def __init__(self, base):
        self.url = base.rstrip("/") + "/mcp"

    def call(self, tool, args, key=None, synthetic=None, timeout=30):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": tool, "arguments": args, "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "osreach-import-reach", "version": "1"}}}}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
        if key:
            headers["Authorization"] = "Bearer " + key
        if synthetic:
            headers["x-mcphost-synthetic"] = synthetic
        req = urllib.request.Request(self.url, body, headers)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            raise Refused("%s HTTP %d" % (tool, e.code))
        except (urllib.error.URLError, OSError) as e:
            raise Refused("%s %s" % (tool, type(e).__name__))
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
        if msg is None or "error" in msg:
            raise Refused("%s rpc error" % tool)
        res = msg["result"]
        if res.get("isError"):
            raise Refused("%s tool error" % tool)
        return res.get("structuredContent", res)


def claude_version():
    try:
        out = subprocess.run(["claude", "--version"], capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    m = re.search(r"(\d+)\.(\d+)\.(\d+)", out)
    return tuple(int(x) for x in m.groups()) if m else None


def shred(path):
    try:
        subprocess.run(["shred", "-u", path], check=False, capture_output=True)
    finally:
        if os.path.exists(path):
            os.remove(path)


def run_live(args):
    row = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "attempt": args.attempt,
           "host_note": args.host_note, "deployed_sha": args.deployed_sha, "claude_version": None,
           "served_tool_names": [], "tool_results": 0, "mismatch": 0, "is_error": 0,
           "export_run_id": None, "unreached": None, "cap": None, "offboarded": 0,
           "result": "stop", "reason": ""}
    root = tempfile.mkdtemp(prefix="osreach-import-reach-")
    os.chmod(root, 0o700)
    rpc = Rpc(args.base_url)
    tenants = []  # (key) in creation order
    secrets = []
    try:
        ver = claude_version()
        if ver is None:
            row["reason"] = "claude not runnable"
            return row
        row["claude_version"] = ".".join(map(str, ver))
        if ver < MIN_CLAUDE:
            row["reason"] = "claude %s below 2.1.293" % row["claude_version"]
            return row
        stamp = "synthorg-osrx-%d" % int(time.time())
        label = "synthorg:osreach-import-reach"
        try:
            src = rpc.call("signup", {"name": stamp + "-src"}, synthetic=label)
        except Refused as e:
            row["reason"] = str(e)
            return row
        tenants.append(src["key"])
        secrets.append(src["key"])
        rpc.call("host.state.set", {"key": "osrx-k1", "value": {"n": 1}}, key=src["key"])
        enq = rpc.call("host.export", {}, key=src["key"])
        run_id = enq["run_id"]
        row["export_run_id"] = run_id
        deadline = time.time() + WAIT_SECS
        status = "running"
        while time.time() < deadline:
            status = rpc.call("host.runs.get", {"run_id": run_id}, key=src["key"]).get("status")
            if status in ("done", "error"):
                break
            time.sleep(3)
        if status != "done":
            row["result"], row["reason"] = "fail", "export status %s after wait" % status
            return row
        dst = rpc.call("signup", {"name": stamp + "-dst"}, synthetic=label)
        tenants.append(dst["key"])
        secrets.append(dst["key"])
        for k in ("claim_url", "claim"):
            if isinstance(src.get(k), str):
                secrets.append(src[k])
            if isinstance(dst.get(k), str):
                secrets.append(dst[k])
        cfg = os.path.join(root, "mcp.json")
        fd = os.open(cfg, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({"mcpServers": {"mcphost": {"type": "http", "url": rpc.url,
                       "headers": {"Authorization": "Bearer " + dst["key"]}}}}, f)
        prompt = ("Call the mcphost tool host_import with dry_run true and run_id \"%s\". "
                  "Then call host_doctor. Call no other tool and report nothing else." % run_id)
        stream = os.path.join(root, "stream.jsonl")
        fd = os.open(stream, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            with os.fdopen(fd, "w") as out:
                subprocess.run(["claude", "-p", prompt, "--output-format", "stream-json", "--verbose",
                                "--max-turns", "6", "--mcp-config", cfg, "--strict-mcp-config",
                                "--allowedTools", "mcp__mcphost__host_import,mcp__mcphost__host_doctor"],
                               stdout=out, stderr=subprocess.DEVNULL, timeout=600)
        except subprocess.TimeoutExpired:
            row["result"], row["reason"] = "fail", "claude timed out"
            return row
        text = open(stream).read()
        if leaks(text, secrets):
            shred(stream)
            row["result"], row["reason"] = "fail", "stream matched the key grep and was shredded"
            return row
        parsed = parse_stream(text.splitlines())
        for k in ("tool_results", "mismatch", "is_error", "unreached", "cap", "result"):
            row[k] = parsed[k]
        row["reason"] = parsed.get("reason", "")
        names = set()
        for line in text.splitlines():
            if '"tools"' in line and line.startswith("{"):
                try:
                    names.update(t for t in json.loads(line).get("tools", []) if isinstance(t, str))
                except ValueError:
                    pass
        row["served_tool_names"] = sorted(n for n in names if canonical(n) in JUDGED)
        return row
    except Refused as e:
        row["result"], row["reason"] = "fail", str(e)
        return row
    except Exception as e:  # parse failure or any bug: still offboard below
        row["result"], row["reason"] = "fail", type(e).__name__
        return row
    finally:
        for key in tenants:
            try:
                if rpc.call("host.self_offboard", {}, key=key).get("disabled") is True:
                    row["offboarded"] += 1
            except Refused:
                pass
        shutil.rmtree(root, ignore_errors=True)


def append_row(path, row):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    line = (json.dumps(row, sort_keys=True) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, line)  # one write under 4 KB
    finally:
        os.close(fd)


def evidence(row):
    return ("tool_results=%s mismatch=%s is_error=%s export_run_id=%s unreached=%s cap=%s offboarded=%s"
            % (row["tool_results"], row["mismatch"], row["is_error"], row["export_run_id"],
               row["unreached"], row["cap"], row["offboarded"]))


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--plan", action="store_true")
    ap.add_argument("--parse", metavar="STREAM")
    ap.add_argument("--base-url", default=os.environ.get("MCPHOST_BASE_URL", DEFAULT_BASE))
    ap.add_argument("--host-note", default="")
    ap.add_argument("--deployed-sha", default="")
    ap.add_argument("--attempt", type=int, default=1)
    ap.add_argument("--results", default=RESULTS)
    args = ap.parse_args(argv)
    if args.plan:
        for i, step in enumerate(PLAN, 1):
            print("%d. %s" % (i, step))
        print("evidence: " + EVIDENCE_TEMPLATE)
        return 0
    if args.parse:
        with open(args.parse) as f:
            print(json.dumps(parse_stream(f.read().splitlines()), sort_keys=True))
        return 0
    row = run_live(args)
    append_row(args.results, row)
    print("result=%s reason=%s" % (row["result"], row["reason"] or "-"))
    print(evidence(row))
    return {"pass": 0, "fail": 1}.get(row["result"], 3)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
