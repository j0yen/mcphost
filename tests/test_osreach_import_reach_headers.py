"""Offline tests for scripts/live/osreach_import_reach.py's JSON-RPC client (hand PR drh).

No network beyond a 127.0.0.1 FIXTURE server. The FIXTURE copies the rmcp 3.3.0 checks the
production host runs on a stateless POST, error texts verbatim:

* rmcp-3.3.0/src/transport/streamable_http_server/tower.rs:510-560
  `validate_request_protocol_version_meta`: a body `_meta` protocolVersion with no
  MCP-Protocol-Version header -> "request _meta protocolVersion requires MCP-Protocol-Version
  header" (:539-541); header != meta -> "MCP-Protocol-Version header ({h}) does not match
  request _meta protocolVersion ({m})" (:546-551). Both go out through
  `header_mismatch_jsonrpc_response` (:660-668): HTTP 400, JSON-RPC code -32020.
* tower.rs:673-715 `validate_standard_headers` + rmcp-3.3.0/src/transport/common/mcp_headers.rs:
  273-284 `validate_request_headers`: with MCP-Protocol-Version >= 2026-07-28, Mcp-Method must be
  present and equal the body method ("missing required Mcp-Method header", "Mcp-Method header
  `{v}` does not match body method `{m}`"); a tools/call must carry Mcp-Name equal to
  params.name ("missing required Mcp-Name header for `{m}`", "Mcp-Name header `{d}` does not
  match body value `{e}`"). Same 400 / -32020 envelope (tower.rs:714-715).

Run: uv run pytest -q tests/test_osreach_import_reach_headers.py
OSREACH_SCRIPT=<path> points the suite at a mutated copy of the driver.
"""

import base64
import importlib.util
import json
import os
import re
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = Path(os.environ.get("OSREACH_SCRIPT", ROOT / "scripts/live/osreach_import_reach.py"))
META_KEY = "io.modelcontextprotocol/protocolVersion"
STANDARD_HEADERS = "2026-07-28"
FIXTURE_KEY = "mcph_FIXTUREKEY0123456789abcdefABCDEF"
FIXTURE_OTHER = "mcph_FIXTUREOTHER9876543210zyxwvuZYXWVU"

mod = None


def load():
    spec = importlib.util.spec_from_file_location("osreach_import_reach_under_test", SCRIPT)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


mod = load()


def rpc_error(rid, message, code=-32020):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}})


def decode_name(raw):
    # mcp_headers.rs `decode_header_value`: =?base64?<b64>?= wraps non-plain values
    if raw.startswith("=?base64?") and raw.endswith("?="):
        return base64.b64decode(raw[len("=?base64?"):-2]).decode()
    return raw


class FIXTURE_Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, status, text, ctype="application/json"):
        data = text.encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        srv = self.server
        srv.FIXTURE_seen.append({"headers": {k.lower(): v for k, v in self.headers.items()}, "body": body})
        if srv.FIXTURE_mode == "echo":
            return self.reply(400, srv.FIXTURE_body)
        rid = body.get("id")
        params = body.get("params") or {}
        meta_version = (params.get("_meta") or {}).get(META_KEY)
        hv = self.headers.get("MCP-Protocol-Version")
        # tower.rs:1926 runs validate_standard_headers first, then the _meta check (:1927)
        if hv is not None and hv >= STANDARD_HEADERS:
            method = body.get("method")
            hm = self.headers.get("Mcp-Method")
            if hm is None:
                return self.reply(400, rpc_error(rid, "missing required Mcp-Method header"))
            if hm != method:
                return self.reply(400, rpc_error(rid, "Mcp-Method header `%s` does not match body method `%s`" % (hm, method)))
            if method == "tools/call":
                raw = self.headers.get("Mcp-Name")
                if raw is None:
                    return self.reply(400, rpc_error(rid, "missing required Mcp-Name header for `%s`" % method))
                got = decode_name(raw)
                if got != params.get("name"):
                    return self.reply(400, rpc_error(rid, "Mcp-Name header `%s` does not match body value `%s`" % (got, params.get("name"))))
        if meta_version is not None:
            if hv is None:
                return self.reply(400, rpc_error(rid, "request _meta protocolVersion requires MCP-Protocol-Version header"))
            if hv != meta_version:
                return self.reply(400, rpc_error(rid, "MCP-Protocol-Version header (%s) does not match request _meta protocolVersion (%s)" % (hv, meta_version)))
        result = {"structuredContent": {"ok": True, "echo_name": params.get("name")}}
        return self.reply(200, json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}))


@pytest.fixture
def FIXTURE_server(monkeypatch):
    for var in ("HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("NO_PROXY", "127.0.0.1")
    srv = ThreadingHTTPServer(("127.0.0.1", 0), FIXTURE_Handler)
    srv.FIXTURE_seen = []
    srv.FIXTURE_mode = "rmcp"
    srv.FIXTURE_body = ""
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    srv.base = "http://127.0.0.1:%d" % srv.server_address[1]
    yield srv
    srv.shutdown()
    srv.server_close()
    t.join(timeout=5)


def mutate_headers(monkeypatch, drop=None, set_=None):
    """Mutation: a Request whose header `drop` is removed / `set_` overridden."""
    real = urllib.request.Request

    class Mutated(real):
        def __init__(self, *a, **kw):
            super().__init__(*a, **kw)
            for name in drop or ():
                for k in [k for k in self.headers if k.lower() == name.lower()]:
                    del self.headers[k]
            for name, value in (set_ or {}).items():
                self.headers[name.capitalize()] = value

    monkeypatch.setattr(urllib.request, "Request", Mutated)


def refusal(rpc, *a, **kw):
    with pytest.raises(mod.Refused) as ei:
        rpc.call(*a, **kw)
    return str(ei.value)


# (a) the fix: the version header travels with the _meta version -------------------------------

def test_fixed_call_gets_200_and_sends_the_three_headers(FIXTURE_server):
    rpc = mod.Rpc(FIXTURE_server.base)
    out = rpc.call("host.state.set", {"key": "k"}, key=FIXTURE_KEY, synthetic="synthorg:FIXTURE")
    assert out == {"ok": True, "echo_name": "host.state.set"}
    seen = FIXTURE_server.FIXTURE_seen[0]
    assert seen["headers"]["mcp-protocol-version"] == mod.PROTOCOL == seen["body"]["params"]["_meta"][META_KEY]
    assert seen["headers"]["mcp-method"] == seen["body"]["method"] == "tools/call"
    assert seen["headers"]["mcp-name"] == "host.state.set"
    assert seen["headers"]["authorization"] == "Bearer " + FIXTURE_KEY
    assert seen["headers"]["x-mcphost-synthetic"] == "synthorg:FIXTURE"


def test_protocol_value_matches_the_reference_client():
    src = (ROOT / "src/client.rs").read_text()
    m = re.search(r'\.header\("MCP-Protocol-Version",\s*"([^"]+)"\)', src)
    assert m and m.group(1) == mod.PROTOCOL
    assert '.header("Mcp-Method", method)' in src and '.header("Mcp-Name", name)' in src


def test_mutation_without_the_version_header_reproduces_the_prod_400(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, drop=["MCP-Protocol-Version"])
    msg = refusal(mod.Rpc(FIXTURE_server.base), "signup", {"name": "x"}, synthetic="synthorg:FIXTURE")
    assert msg == "signup HTTP 400 -32020 request _meta protocolVersion requires MCP-Protocol-Version header"


def test_mutation_with_a_different_version_header_is_refused(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, set_={"MCP-Protocol-Version": "2026-01-01"})
    msg = refusal(mod.Rpc(FIXTURE_server.base), "signup", {"name": "x"})
    assert msg == ("signup HTTP 400 -32020 MCP-Protocol-Version header (2026-01-01) does not match "
                   "request _meta protocolVersion (%s)" % mod.PROTOCOL)


# (b) rmcp's second check: Mcp-Method / Mcp-Name ----------------------------------------------

def test_mutation_without_mcp_method_is_refused(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, drop=["Mcp-Method"])
    assert refusal(mod.Rpc(FIXTURE_server.base), "signup", {}) == "signup HTTP 400 -32020 missing required Mcp-Method header"


def test_mutation_with_the_wrong_mcp_method_is_refused(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, set_={"Mcp-Method": "tools/list"})
    assert refusal(mod.Rpc(FIXTURE_server.base), "signup", {}) == (
        "signup HTTP 400 -32020 Mcp-Method header `tools/list` does not match body method `tools/call`")


def test_mutation_without_mcp_name_is_refused(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, drop=["Mcp-Name"])
    assert refusal(mod.Rpc(FIXTURE_server.base), "host.export", {}) == (
        "host.export HTTP 400 -32020 missing required Mcp-Name header for `tools/call`")


def test_mutation_with_the_wrong_mcp_name_is_refused(FIXTURE_server, monkeypatch):
    mutate_headers(monkeypatch, set_={"Mcp-Name": "host.state.get"})
    assert refusal(mod.Rpc(FIXTURE_server.base), "host.state.set", {}) == (
        "host.state.set HTTP 400 -32020 Mcp-Name header `host.state.get` does not match body value `host.state.set`")


# (c) the 400 cause survives, scrubbed BEFORE the cut -------------------------------------------

def echo(srv, text):
    srv.FIXTURE_mode, srv.FIXTURE_body = "echo", text


def test_key_mid_sentence_without_bearer_prefix_is_redacted(FIXTURE_server):
    echo(FIXTURE_server, rpc_error(1, "tenant %s is over quota, retry later" % FIXTURE_KEY, code=-32001))
    msg = refusal(mod.Rpc(FIXTURE_server.base), "host.export", {}, key=FIXTURE_KEY)
    assert FIXTURE_KEY not in msg and FIXTURE_KEY[:10] not in msg
    assert msg == "host.export HTTP 400 -32001 tenant [redacted] is over quota, retry later"


def test_key_straddling_the_200_char_cut_is_redacted_whole(FIXTURE_server):
    echo(FIXTURE_server, rpc_error(1, "a" * 185 + FIXTURE_KEY + " tail"))
    msg = refusal(mod.Rpc(FIXTURE_server.base), "host.export", {}, key=FIXTURE_KEY)
    assert FIXTURE_KEY[:8] not in msg
    assert msg.startswith("host.export HTTP 400 -32020 " + "a" * 185 + "[redacted]")
    assert len(msg.split(" -32020 ", 1)[1]) <= 200


def test_every_held_secret_is_redacted_and_bearer_claim_shapes_too(FIXTURE_server):
    other, synth = FIXTURE_OTHER, "synthorg:FIXTUREsecretlabel"
    text = ("auth Bearer %s other %s synth %s claim https://x.example/claim/AbCdEf123456 and "
            "authorization: bearer zzzzzzzzzz") % (FIXTURE_KEY, other, synth)
    echo(FIXTURE_server, rpc_error(1, text))
    rpc = mod.Rpc(FIXTURE_server.base, [other])
    msg = refusal(rpc, "signup", {}, key=FIXTURE_KEY, synthetic=synth)
    for leak in (FIXTURE_KEY, other, synth, "AbCdEf123456", "zzzzzzzzzz"):
        assert leak not in msg


def test_env_held_key_is_redacted(FIXTURE_server, monkeypatch):
    env_key = "FIXTUREenvKEYvalue-1234567890"
    monkeypatch.setenv("MCPHOST_FIXTURE_ADMIN_KEY", env_key)
    echo(FIXTURE_server, rpc_error(1, "saw %s here" % env_key))
    assert env_key not in refusal(mod.Rpc(FIXTURE_server.base), "signup", {})


def test_non_json_body_gives_the_first_120_chars_after_the_scrub(FIXTURE_server):
    echo(FIXTURE_server, "<html>" + "gateway says " + FIXTURE_KEY + " " + "p" * 400 + "</html>")
    msg = refusal(mod.Rpc(FIXTURE_server.base), "signup", {}, key=FIXTURE_KEY)
    cause = msg.split("HTTP 400 ", 1)[1]
    assert FIXTURE_KEY not in cause and "[redacted]" in cause and len(cause) <= 120


def test_json_without_error_member_falls_back_to_the_raw_scrub(FIXTURE_server):
    echo(FIXTURE_server, json.dumps({"detail": "nope " + FIXTURE_KEY}))
    msg = refusal(mod.Rpc(FIXTURE_server.base), "signup", {}, key=FIXTURE_KEY)
    assert msg == 'signup HTTP 400 {"detail": "nope [redacted]"}'


def test_body_read_is_capped_at_2kb(FIXTURE_server):
    echo(FIXTURE_server, rpc_error(1, "m" * 5000))
    msg = refusal(mod.Rpc(FIXTURE_server.base), "signup", {})
    assert msg.startswith("signup HTTP 400 ") and len(msg) < 200  # truncated JSON -> raw first 120


def test_scrub_hides_the_head_of_a_key_cut_by_the_read_cap():
    cut = "x" * 30 + FIXTURE_KEY[:12]
    out = mod.scrub(cut, [FIXTURE_KEY])
    assert FIXTURE_KEY[:8] not in out and out.endswith("[redacted]")


def test_scrub_runs_before_truncation_not_after():
    text = "q" * 115 + FIXTURE_KEY
    assert FIXTURE_KEY[:8] not in mod.http_cause(text, [FIXTURE_KEY])


def test_connection_errors_keep_their_old_shape():
    rpc = mod.Rpc("http://127.0.0.1:9")
    assert refusal(rpc, "signup", {}, timeout=2) == "signup URLError"


def test_plan_mode_is_unchanged(capsys):
    assert mod.main(["--plan"]) == 0
    out = capsys.readouterr().out
    assert out.splitlines()[0] == "1. source signup" and "evidence: tool_results=" in out
