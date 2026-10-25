#!/usr/bin/env python3
"""tblurlp_import_live.py -- PRD-mcphost-table-import-url-live-proof.

Proves import-from-URL end to end against one mcphost: the binary serves the
database-in-a-minute fixture at /examples/sample-1000.csv, and a synthetic
tenant creates a table from that URL in one `host_table_create` call.

  1. GET <base>/examples/sample-1000.csv  (fixture_http, fixture_bytes)
  2. signup {name} under x-mcphost-synthetic: synthorg-tblurlp   (skipped with --key-file)
  3. host_table_create {name: <table>, source_url} -> rows_imported
  4. host_table_count  {table}               -> count
  5. host_self_offboard {} in `finally`      -> offboarded = 1 when `disabled` is true

Usage: tblurlp_import_live.py --plan
       tblurlp_import_live.py [--base-url URL] [--table NAME] [--source-url URL]
                              [--key-file FILE] [--results FILE]
Evidence: one JSON row per attempt appended to --results (default
~/.local/state/mcphost-live/tblurlp-ac04-results.jsonl); the row's `summary`
equals the last stdout line
  fixture_http=<n> rows_imported=<n> count=<n> is_error=<n> offboarded=<n>
No key is printed or logged: a minted key lives only in a 0600 file under this
run's own mktemp -d root, shredded on exit. A --key-file is read-only and never
deleted; it skips signup and offboard (offboarded=0).
Exit: 0 pass, 1 fail, 2 usage, 3 fixture not served (fixture_http != 200:
deploy pending, never a fail).
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
DEFAULT_TABLE = "tblurlp_ac04"
SYNTHETIC = "synthorg-tblurlp"
PROTOCOL = "2026-07-28"
EXPECTED_ROWS = 1000

PLAN = [
    "GET the fixture URL (fixture_http, fixture_bytes)",
    "signup under x-mcphost-synthetic: " + SYNTHETIC + " (skipped with --key-file)",
    "host_table_create {name: <table>, source_url} -> rows_imported",
    "host_table_count {table} -> count",
    "host_self_offboard {} in finally -> offboarded (skipped with --key-file)",
]
EVIDENCE_TEMPLATE = "fixture_http=<n> rows_imported=<n> count=<n> is_error=<n> offboarded=<n>"


class Refused(Exception):
    pass


class ToolError(Exception):
    def __init__(self, tool, code):
        Exception.__init__(self, "%s tool error" % tool)
        self.code = code


class Rpc:
    def __init__(self, base, key_path=None):
        self.url = base.rstrip("/") + "/mcp"
        self.key_path = key_path

    def _key(self):
        if not self.key_path:
            return None
        with open(self.key_path) as f:
            return f.read().strip()

    def call(self, tool, args, synthetic=None, timeout=60):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": tool, "arguments": args, "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "tblurlp-import-live", "version": "1"}}}}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": PROTOCOL, "Mcp-Method": "tools/call", "Mcp-Name": tool}
        key = self._key()
        if key:
            headers["Authorization"] = "Bearer " + key
        if synthetic:
            headers["x-mcphost-synthetic"] = synthetic
        req = urllib.request.Request(self.url, body, headers)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", "replace")
            try:
                code = json.loads(body)["error"]["data"]["error_code"]
            except (ValueError, KeyError, TypeError):
                code = None
            if isinstance(code, str):
                raise ToolError(tool, code)
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
        out = res.get("structuredContent", res)
        if res.get("isError"):
            code = out.get("error_code") if isinstance(out, dict) else None
            raise ToolError(tool, code if isinstance(code, str) else "unknown")
        return out


def fetch_fixture(url, timeout=60):
    """(http status, byte count); a connection failure is (0, 0)."""
    req = urllib.request.Request(url, headers={"User-Agent": "tblurlp-import-live"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
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
    return ("fixture_http=%s rows_imported=%s count=%s is_error=%s offboarded=%s"
            % (row["fixture_http"], row["rows_imported"], row["count"], row["is_error"], row["offboarded"]))


def run_live(args):
    start = time.time()
    source_url = args.source_url or args.base_url.rstrip("/") + FIXTURE_PATH
    row = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "base_url": args.base_url,
           "table": args.table, "source_url": source_url, "fixture_http": 0, "fixture_bytes": 0,
           "rows_imported": 0, "count": 0, "is_error": 0, "error_code": None, "offboarded": 0,
           "result": "fail", "result_detail": "", "elapsed_s": 0}
    root = tempfile.mkdtemp(prefix="tblurlp-import-live-")
    os.chmod(root, 0o700)
    key_path = args.key_file
    rpc = Rpc(args.base_url, key_path)
    minted = False
    try:
        row["fixture_http"], row["fixture_bytes"] = fetch_fixture(source_url)
        if row["fixture_http"] != 200:
            row["result"] = "pending"
            row["result_detail"] = "fixture not served (deploy pending)"
            return row
        if not key_path:
            label = "%s-%d" % (SYNTHETIC, int(time.time()))
            signed = rpc.call("signup", {"name": label}, synthetic=SYNTHETIC)
            key_path = os.path.join(root, "key")
            fd = os.open(key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(signed["key"])
            rpc.key_path = key_path
            minted = True
        created = rpc.call("host_table_create", {"name": args.table, "source_url": source_url})
        row["rows_imported"] = created.get("rows_imported", 0)
        counted = rpc.call("host_table_count", {"table": args.table})
        row["count"] = counted.get("count", 0)
        ok = row["rows_imported"] == EXPECTED_ROWS and row["count"] == EXPECTED_ROWS
        row["result"] = "pass" if ok else "fail"
        if not ok:
            row["result_detail"] = "expected %d rows" % EXPECTED_ROWS
        return row
    except ToolError as e:
        row["is_error"], row["error_code"] = 1, e.code
        row["result_detail"] = str(e)
        return row
    except Refused as e:
        row["is_error"] = 1
        row["result_detail"] = str(e)
        return row
    except Exception as e:  # any bug: still offboard below
        row["is_error"] = 1
        row["result_detail"] = type(e).__name__
        return row
    finally:
        if minted:
            try:
                if rpc.call("host_self_offboard", {}).get("disabled") is True:
                    row["offboarded"] = 1
            except (Refused, ToolError, OSError):
                pass
            shred(key_path)
        shutil.rmtree(root, ignore_errors=True)
        row["elapsed_s"] = round(time.time() - start, 2)


def append_row(path, row):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    line = (json.dumps(row, sort_keys=True) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, line)  # one write under 4 KB
    finally:
        os.close(fd)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--plan", action="store_true")
    ap.add_argument("--base-url", default=os.environ.get("MCPHOST_BASE_URL", DEFAULT_BASE))
    ap.add_argument("--table", default=DEFAULT_TABLE)
    ap.add_argument("--source-url", default=None, help="default <base-url>%s" % FIXTURE_PATH)
    ap.add_argument("--key-file", default=None, help="read-only tenant key; skips signup and offboard")
    ap.add_argument("--results", default=RESULTS)
    args = ap.parse_args(argv)
    if args.plan:
        for i, step in enumerate(PLAN, 1):
            print("%d. %s" % (i, step))
        print("evidence: " + EVIDENCE_TEMPLATE)
        return 0
    if args.key_file and not os.access(args.key_file, os.R_OK):
        print("usage: --key-file is not readable", file=sys.stderr)
        return 2
    row = run_live(args)
    row["summary"] = summary(row)
    append_row(args.results, row)
    print("result=%s detail=%s" % (row["result"], row["result_detail"] or "-"))
    print(row["summary"])
    return {"pass": 0, "fail": 1, "pending": 3}[row["result"]]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
