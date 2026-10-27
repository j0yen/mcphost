#!/usr/bin/env python3
"""tblurlp_import_live.py -- PRD-mcphost-table-import-url-live-proof.

Proves import-from-URL end to end against a running mcphost: a synthetic
tenant creates a table from the public fixture URL, counts it and offboards.

  1. fixture GET     (records fixture_http and the byte count)
  2. signup          (synthetic tenant, header x-mcphost-synthetic: synthorg-tblurlp;
                      skipped with --key-file)
  3. host_table_create {name, source_url}   -> rows_imported
  4. host_table_count  {table}              -> count
  5. host_self_offboard {}  on every exit path (skipped with --key-file)

Usage: tblurlp_import_live.py --plan
       tblurlp_import_live.py [--base-url URL] [--table NAME] [--source-url URL]
                              [--key-file FILE] [--results FILE]
The last stdout line is the evidence line:
  fixture_http=<n> rows_imported=<n> count=<n> is_error=<n> offboarded=<n>
One JSON row per attempt is appended to --results, carrying that line as `summary`.
The minted key lives only in a 0600 file under this run's own mktemp -d root and is
shredded on exit; it is never printed or logged. A --key-file is read, never deleted.
Exit: 0 pass, 1 fail, 2 usage, 3 fixture not served (fixture_http != 200: deploy
pending, never a fail).
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
RESULTS = os.path.expanduser("~/.local/state/mcphost-live/tblurlp-ac04-results.jsonl")
FIXTURE_PATH = "/examples/sample-1000.csv"
SYNTHETIC = "synthorg-tblurlp"
EXPECTED_ROWS = 1000
PROTOCOL = "2026-07-28"

PLAN = [
    "fixture GET",
    "signup (synthetic tenant)",
    "host_table_create from source_url",
    "host_table_count",
    "host_self_offboard",
]
EVIDENCE_TEMPLATE = "fixture_http=<n> rows_imported=<n> count=<n> is_error=<n> offboarded=<n>"


class Refused(Exception):
    pass


class Rpc:
    def __init__(self, base):
        self.url = base.rstrip("/") + "/mcp"

    def call(self, tool, args, key=None, synthetic=None, timeout=60):
        """Returns (is_error, structured result, error_code)."""
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": tool, "arguments": args, "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "tblurlp-import-live", "version": "1"}}}}).encode()
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
            code = (msg["error"] or {}).get("code")
            return True, {}, "rpc_%s" % code
        res = msg["result"]
        structured = res.get("structuredContent")
        if not isinstance(structured, dict):
            structured = {}
        if res.get("isError"):
            return True, structured, error_code(res, structured)
        return False, structured, None


def error_code(res, structured):
    code = structured.get("error_code") or structured.get("code")
    if isinstance(code, str):
        return code
    for block in res.get("content") or []:
        if isinstance(block, dict) and isinstance(block.get("text"), str):
            try:
                inner = json.loads(block["text"])
            except ValueError:
                continue
            if isinstance(inner, dict) and isinstance(inner.get("error_code"), str):
                return inner["error_code"]
    return "tool_error"


def fetch_fixture(url):
    """(http status, byte count); status 0 when no response came back."""
    try:
        with urllib.request.urlopen(url, timeout=60) as resp:
            return resp.status, len(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, 0
    except (urllib.error.URLError, OSError):
        return 0, 0


def shred(path):
    try:
        subprocess.run(["shred", "-u", path], check=False, capture_output=True)
    except OSError:
        pass
    finally:
        if os.path.exists(path):
            os.remove(path)


def summary(row):
    return "fixture_http=%d rows_imported=%d count=%d is_error=%d offboarded=%d" % (
        row["fixture_http"], row["rows_imported"], row["count"], row["is_error"], row["offboarded"])


def run_live(args):
    started = time.time()
    source_url = args.source_url or args.base_url.rstrip("/") + FIXTURE_PATH
    row = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "base_url": args.base_url,
           "table": args.table, "fixture_http": 0, "fixture_bytes": 0, "rows_imported": 0,
           "count": 0, "is_error": 0, "error_code": None, "offboarded": 0,
           "result": "fail", "reason": "", "elapsed_s": 0.0}
    root = tempfile.mkdtemp(prefix="tblurlp-import-live-")
    os.chmod(root, 0o700)
    rpc = Rpc(args.base_url)
    minted = None  # path of the 0600 file holding a key this run minted

    def read_key():
        with open(minted or args.key_file) as f:
            return f.read().strip()

    try:
        row["fixture_http"], row["fixture_bytes"] = fetch_fixture(source_url)
        if row["fixture_http"] != 200:
            row["result"], row["reason"] = "pending", "fixture not served (HTTP %d)" % row["fixture_http"]
            return row
        if not args.key_file:
            stamp = "synthorg-tblurlp-%d" % int(time.time())
            is_err, out, code = rpc.call("signup", {"name": stamp}, synthetic=SYNTHETIC)
            if is_err or not isinstance(out.get("key"), str):
                row["is_error"], row["error_code"] = 1, code or "signup_no_key"
                row["reason"] = "signup refused"
                return row
            minted = os.path.join(root, "key")
            fd = os.open(minted, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(out["key"])
            del out
        is_err, out, code = rpc.call("host_table_create", {"name": args.table, "source_url": source_url},
                                     key=read_key())
        if is_err:
            row["is_error"], row["error_code"], row["reason"] = 1, code, "host_table_create refused"
            return row
        row["rows_imported"] = out.get("rows_imported") if isinstance(out.get("rows_imported"), int) else 0
        is_err, out, code = rpc.call("host_table_count", {"table": args.table}, key=read_key())
        if is_err:
            row["is_error"], row["error_code"], row["reason"] = 1, code, "host_table_count refused"
            return row
        row["count"] = out.get("count") if isinstance(out.get("count"), int) else 0
        return row
    except Refused as e:
        row["is_error"], row["error_code"], row["reason"] = 1, "transport", str(e)
        return row
    finally:
        if minted:
            try:
                is_err, out, _ = rpc.call("host_self_offboard", {}, key=read_key())
                if not is_err and out.get("disabled") is True:
                    row["offboarded"] = 1
            except (Refused, OSError):
                pass
            shred(minted)
        shutil.rmtree(root, ignore_errors=True)
        row["elapsed_s"] = round(time.time() - started, 3)
        if row["result"] == "fail" and not row["is_error"] and not row["reason"]:
            ok = (row["rows_imported"] == EXPECTED_ROWS and row["count"] == EXPECTED_ROWS
                  and (args.key_file or row["offboarded"] == 1))
            row["result"] = "pass" if ok else "fail"
            if not ok:
                row["reason"] = "rows_imported/count/offboard mismatch"
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
    ap.add_argument("--table", default="tblurlp_ac04")
    ap.add_argument("--source-url", default=None)
    ap.add_argument("--key-file", default=None)
    ap.add_argument("--results", default=RESULTS)
    args = ap.parse_args(argv)
    if args.plan:
        for i, step in enumerate(PLAN, 1):
            print("%d. %s" % (i, step))
        print("evidence: " + EVIDENCE_TEMPLATE)
        return 0
    if args.key_file and not os.path.isfile(args.key_file):
        print("usage: --key-file %s is not a file" % args.key_file, file=sys.stderr)
        return 2
    row = run_live(args)
    row.setdefault("summary", summary(row))
    append_row(args.results, row)
    print("result=%s reason=%s error_code=%s" % (row["result"], row["reason"] or "-", row["error_code"] or "-"))
    print(row["summary"])
    return {"pass": 0, "pending": 3}.get(row["result"], 1)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
