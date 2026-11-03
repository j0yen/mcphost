#!/usr/bin/env python3
"""spmsg_ac08.py -- PRD-mcphost-invalid-spec-message-carries-fix, AC 8.

Proves an invalid-spec refusal carries its own fix in the one field every MCP
SDK client shows, the JSON-RPC error `message`: a synthetic tenant publishes an
http spec without `method`, reads only that message, parses the minimal spec out
of the text, republishes it and passes.

  1. signup           (synthetic tenant, header x-mcphost-synthetic: synthorg-spmsg,
                       client name synthorg-spmsg-live)
  2. host_tool_publish {name, kind: http, spec: {"url": "https://example.com"}}
                       -> refused; records the error `message` only
  3. the message names `method`
  4. parse the spec between "; minimal spec: " and "; docs: "
  5. host_tool_publish again with that spec -> not an error
  6. host_tool_remove {name}
  7. host_self_offboard {}  on every exit path

Usage: spmsg_ac08.py --plan
       spmsg_ac08.py [--base-url URL] [--name TOOL] [--results FILE]
The last stdout line is the evidence line:
  refused=<n> field_named=<n> spec_in_message=<n> republished=<n> removed=<n> offboarded=<n>
One JSON row per attempt is appended to --results, carrying that line as `summary`.
The minted key lives only in a 0600 file under this run's own mktemp -d root and is
shredded on exit; it is never printed or logged.
Exit: 0 pass, 1 fail, 2 usage.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

DEFAULT_BASE = "https://mcphost.dev"
RESULTS = os.path.expanduser("~/.local/state/mcphost-live/spmsg-ac08-results.jsonl")
SYNTHETIC = "synthorg-spmsg"
CLIENT_NAME = "synthorg-spmsg-live"
TOOL_NAME = "spmsg_ac08"
BAD_SPEC = {"url": "https://example.com"}
MINIMAL_MARK = "; minimal spec: "
DOCS_MARK = "; docs: "
PROTOCOL = "2026-07-28"

PLAN = [
    "signup (synthetic tenant)",
    "host_tool_publish an http spec without method (refused; record the message)",
    "check the message names method",
    "parse the minimal spec out of the message",
    "host_tool_publish the parsed spec",
    "host_tool_remove",
    "host_self_offboard",
]
EVIDENCE_TEMPLATE = ("refused=<n> field_named=<n> spec_in_message=<n> republished=<n> "
                     "removed=<n> offboarded=<n>")
COUNTERS = ("refused", "field_named", "spec_in_message", "republished", "removed", "offboarded")


class Refused(Exception):
    pass


class Rpc:
    def __init__(self, base):
        self.url = base.rstrip("/") + "/mcp"

    def call(self, tool, args, key=None, synthetic=None, timeout=60):
        """Returns (is_error, structured result, error message, error code).

        The message is the JSON-RPC error's `message` (or, for an isError result, the
        text block) -- the only part of a refusal an SDK client shows.
        """
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": tool, "arguments": args, "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": CLIENT_NAME, "version": "1"}}}}).encode()
        # SEP-2243: once the protocol version is declared the server requires the
        # method and tool name mirrored in headers.
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": PROTOCOL, "Mcp-Method": "tools/call", "Mcp-Name": tool}
        if key:
            headers["Authorization"] = "Bearer " + key
        if synthetic:
            headers["x-mcphost-synthetic"] = synthetic
        req = urllib.request.Request(self.url, body, headers)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            # A refusal rides a 4xx with the JSON-RPC error as its body; that body is
            # what an SDK client reads, so parse it before giving up on the status.
            try:
                raw = e.read().decode()
            except (OSError, ValueError):
                raw = ""
            if "{" not in raw:
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
        if msg is None:
            raise Refused("%s no rpc response" % tool)
        if "error" in msg:
            err = msg["error"] or {}
            data = err.get("data") if isinstance(err.get("data"), dict) else {}
            code = data.get("error_code") if isinstance(data.get("error_code"), str) else "rpc_%s" % err.get("code")
            text = err.get("message")
            return True, {}, text if isinstance(text, str) else "", code
        res = msg["result"]
        structured = res.get("structuredContent")
        if not isinstance(structured, dict):
            structured = {}
        if res.get("isError"):
            texts = [b.get("text") for b in res.get("content") or []
                     if isinstance(b, dict) and isinstance(b.get("text"), str)]
            code = structured.get("error_code") if isinstance(structured.get("error_code"), str) else "tool_error"
            return True, structured, texts[0] if texts else "", code
        return False, structured, "", None


def minimal_spec_in(message):
    """The spec between "; minimal spec: " and "; docs: " (else to the end), or None."""
    head, mark, tail = message.partition(MINIMAL_MARK)
    if not mark:
        return None
    text = tail.rsplit(DOCS_MARK, 1)[0] if DOCS_MARK in tail else tail
    try:
        spec = json.loads(text)
    except ValueError:
        return None
    return spec if isinstance(spec, dict) else None


def shred(path):
    try:
        subprocess.run(["shred", "-u", path], check=False, capture_output=True)
    except OSError:
        pass
    finally:
        if os.path.exists(path):
            os.remove(path)


def summary(row):
    return " ".join("%s=%d" % (name, row[name]) for name in COUNTERS)


def run_live(args):
    started = time.time()
    row = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "base_url": args.base_url,
           "tool": args.name, "message": None, "error_code": None, "result": "fail",
           "reason": "", "elapsed_s": 0.0}
    row.update({name: 0 for name in COUNTERS})
    root = tempfile.mkdtemp(prefix="spmsg-ac08-")
    os.chmod(root, 0o700)
    rpc = Rpc(args.base_url)
    key_path = None  # the 0600 file holding the key this run minted

    def read_key():
        with open(key_path) as f:
            return f.read().strip()

    try:
        stamp = "synthorg-spmsg-%d" % int(time.time())
        is_err, out, _, code = rpc.call("signup", {"name": stamp}, synthetic=SYNTHETIC)
        if is_err or not isinstance(out.get("key"), str):
            row["error_code"], row["reason"] = code or "signup_no_key", "signup refused"
            return row
        key_path = os.path.join(root, "key")
        fd = os.open(key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(out["key"])
        del out

        is_err, _, message, code = rpc.call(
            "host_tool_publish", {"name": args.name, "kind": "http", "spec": BAD_SPEC}, key=read_key())
        if not is_err:
            row["reason"] = "the invalid spec was published"
            return row
        row["refused"], row["message"], row["error_code"] = 1, message, code
        row["field_named"] = int("`method`" in message)
        spec = minimal_spec_in(message)
        row["spec_in_message"] = int(spec is not None)
        if spec is None:
            row["reason"] = "no parseable minimal spec in the message"
            return row
        is_err, _, _, code = rpc.call(
            "host_tool_publish", {"name": args.name, "kind": "http", "spec": spec}, key=read_key())
        if is_err:
            row["error_code"], row["reason"] = code, "republish refused"
            return row
        row["republished"] = 1
        is_err, _, _, code = rpc.call("host_tool_remove", {"name": args.name}, key=read_key())
        if is_err:
            row["error_code"], row["reason"] = code, "host_tool_remove refused"
            return row
        row["removed"] = 1
        return row
    except Refused as e:
        row["error_code"], row["reason"] = "transport", str(e)
        return row
    finally:
        if key_path:
            try:
                is_err, out, _, _ = rpc.call("host_self_offboard", {}, key=read_key())
                if not is_err and out.get("disabled") is True:
                    row["offboarded"] = 1
            except (Refused, OSError):
                pass
            shred(key_path)
        shutil.rmtree(root, ignore_errors=True)
        row["elapsed_s"] = round(time.time() - started, 3)
        ok = all(row[name] == 1 for name in COUNTERS)
        row["result"] = "pass" if ok else "fail"
        if not ok and not row["reason"]:
            row["reason"] = "a step did not hold: " + summary(row)
        row["summary"] = summary(row)


def append_row(path, row):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    line = (json.dumps(row, sort_keys=True) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, line)
    finally:
        os.close(fd)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--plan", action="store_true")
    ap.add_argument("--base-url", default=os.environ.get("MCPHOST_BASE_URL", DEFAULT_BASE))
    ap.add_argument("--name", default=TOOL_NAME)
    ap.add_argument("--results", default=RESULTS)
    args = ap.parse_args(argv)
    if args.plan:
        for i, step in enumerate(PLAN, 1):
            print("%d. %s" % (i, step))
        print("evidence: " + EVIDENCE_TEMPLATE)
        return 0
    row = run_live(args)
    append_row(args.results, row)
    print("result=%s reason=%s error_code=%s" % (row["result"], row["reason"] or "-", row["error_code"] or "-"))
    print(row["summary"])
    return 0 if row["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
