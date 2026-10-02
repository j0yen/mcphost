#!/usr/bin/env bash
# docs-qa.sh -- runnable end-to-end proof of "Docs Q&A in a minute" (see
# www/llms.txt): one fresh tenant against <endpoint>, the 8-document
# corpus/ put and indexed, `ask_docs` published (python kind, searches over
# the zero-network mcphost.docs.search loopback), the 6 gold.json questions
# asked, and a receipt JSON written with per-step timings, hit results, and
# a quota summary. Exit 0 only when >= 5 of 6 questions hit.
#
# Requires: bash, python3 (stdlib only -- no curl, no jq, no third-party
# package: the whole recipe is one urllib-based JSON-RPC client).
#
# Usage:
#   docs-qa.sh <endpoint>
#   docs-qa.sh <endpoint> [--corpus-dir DIR] [--receipt-dir DIR] \
#              [--reuse-state FILE] [--embeddings PROVIDER_ENDPOINT MODEL SECRET_NAME]
#
#   <endpoint>        the mcphost MCP endpoint, e.g. https://mcphost.dev/mcp
#   --corpus-dir      corpus directory to put (default: ./corpus next to
#                     this script) -- override to prove a re-index picks up
#                     an edited document (PRD AC4).
#   --receipt-dir     where to write the receipt JSON (default:
#                     $MCPHOST_RECEIPT_DIR, or a fresh mktemp -d).
#   --reuse-state     a file to load {tenant, key} from at start (skipping
#                     signup and publish) and (over)write with the current
#                     tenant/key at the end -- lets a second invocation
#                     reuse the same tenant to prove a document update
#                     changes the answer without a fresh signup (AC4). The
#                     base recipe (no flag) always signs up fresh, so this
#                     is purely a test/re-run affordance, never required.
#   --embeddings      configures the embeddings provider FIRST (before any
#                     document is put), asks in embeddings mode, then
#                     switches back to lexical and re-asks -- the receipt's
#                     "modes" section reports both hit rates (P1
#                     requirement 6/AC7). PROVIDER_ENDPOINT is the
#                     openai-compatible embeddings API URL; the secret
#                     value stored under SECRET_NAME comes from
#                     $DOCS_QA_EMBED_SECRET_VALUE (default a placeholder --
#                     a real Live run exports the real key under that
#                     name).
set -uo pipefail
cd "$(dirname "$0")"

python3 - "$@" <<'PY'
import glob
import json
import os
import sys
import time
import urllib.error
import urllib.request

def usage():
    print(
        "usage: docs-qa.sh <endpoint> [--corpus-dir DIR] [--receipt-dir DIR] "
        "[--reuse-state FILE] [--embeddings PROVIDER_ENDPOINT MODEL SECRET_NAME]",
        file=sys.stderr,
    )
    sys.exit(2)

argv = sys.argv[1:]
if not argv:
    usage()
endpoint = argv.pop(0)
corpus_dir = "corpus"
receipt_dir = os.environ.get("MCPHOST_RECEIPT_DIR")
reuse_state = None
embeddings = None  # (provider_endpoint, model, secret_name)

i = 0
while i < len(argv):
    a = argv[i]
    if a == "--corpus-dir" and i + 1 < len(argv):
        corpus_dir = argv[i + 1]
        i += 2
    elif a == "--receipt-dir" and i + 1 < len(argv):
        receipt_dir = argv[i + 1]
        i += 2
    elif a == "--reuse-state" and i + 1 < len(argv):
        reuse_state = argv[i + 1]
        i += 2
    elif a == "--embeddings" and i + 3 < len(argv):
        embeddings = (argv[i + 1], argv[i + 2], argv[i + 3])
        i += 4
    else:
        usage()

if receipt_dir is None:
    import tempfile
    receipt_dir = tempfile.mkdtemp(prefix="docs-qa-receipt-")
os.makedirs(receipt_dir, exist_ok=True)

GOLD_PATH = os.path.join(corpus_dir, "gold.json")
ASK_DOCS_SOURCE_PATH = "ask_docs.py"
SYNTHETIC_SOURCE = "recipe-docs-qa"

call_log = []


def mcp_call(tool, arguments, key=None):
    call_log.append(tool)
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": arguments,
                # SEP-2575: this host runs every request stateless (no
                # `initialize` handshake to remember client context
                # from), so every `tools/call` must carry it itself.
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "docs-qa-recipe",
                        "version": "1.0.0",
                    },
                },
            },
        }
    ).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2026-07-28",
        "Mcp-Method": "tools/call",
        "Mcp-Name": tool,
    }
    if key:
        headers["Authorization"] = f"Bearer {key}"
    req = urllib.request.Request(endpoint, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read()
    except urllib.error.HTTPError as e:
        raw = e.read()
    text = raw.decode()
    # PRD-mcphost-one-next-tool requirement 3: a call that binds this
    # connection (signup) now emits notifications/tools/list_changed
    # before its own result, which upgrades that one response from a
    # plain JSON body to a text/event-stream one (one "data: <json>" line
    # per message, blocks separated by a blank line) -- the real JSON-RPC
    # message is always the last one on the wire.
    if text.lstrip().startswith("data:"):
        last = None
        for block in text.split("\n\n"):
            for line in block.splitlines():
                if line.startswith("data: "):
                    last = line[len("data: "):]
                elif line.startswith("data:"):
                    last = line[len("data:"):]
        text = last if last is not None else text
    return json.loads(text)


def is_error(resp):
    return "error" in resp


def structured_of(resp):
    if is_error(resp):
        return {"__error__": resp["error"]}
    result = resp.get("result") or {}
    sc = result.get("structuredContent")
    if sc is not None:
        return sc
    content = result.get("content") or []
    if content and isinstance(content[0], dict) and "text" in content[0]:
        text = content[0]["text"]
        try:
            return json.loads(text)
        except ValueError:
            return {"text": text}
    return {}


def fail(msg):
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def wait_for_index_ready(key, want_mode=None, timeout_s=90, poll_interval=1.0):
    start = time.monotonic()
    last_status = {}
    while True:
        resp = mcp_call("host.docs.status", {}, key)
        if is_error(resp):
            fail(f"host.docs.status error: {resp['error']}")
        status = structured_of(resp)
        last_status = status
        index = status.get("index", {})
        ready = index.get("lag_seconds") == 0 and index.get("pending_documents", 1) == 0
        if want_mode is not None:
            ready = ready and index.get("mode") == want_mode
        if ready:
            return time.monotonic() - start, status
        if time.monotonic() - start > timeout_s:
            fail(f"timed out after {timeout_s}s waiting for the index to catch up: {last_status}")
        time.sleep(poll_interval)


def put_corpus(key):
    paths = sorted(glob.glob(os.path.join(corpus_dir, "*.md")))
    if len(paths) != 8:
        fail(f"expected exactly 8 corpus documents in {corpus_dir}, found {len(paths)}")
    for path in paths:
        name = os.path.basename(path)
        with open(path, encoding="utf-8") as f:
            content = f.read()
        resp = mcp_call("host.docs.put", {"name": name, "content": content}, key)
        if is_error(resp):
            fail(f"host.docs.put {name} failed: {resp['error']}")
    return len(paths)


def publish_ask_docs(key):
    with open(ASK_DOCS_SOURCE_PATH, encoding="utf-8") as f:
        source = f.read()
    resp = mcp_call(
        "host.tool_publish",
        {"name": "ask_docs", "kind": "python", "spec": {"source": source}},
        key,
    )
    if is_error(resp):
        fail(f"host.tool_publish ask_docs failed: {resp['error']}")


def ask_gold_questions(namespace, key, gold):
    hits = 0
    answers = []
    for q in gold:
        resp = mcp_call(f"{namespace}.ask_docs", {"query": q["question"], "k": 5}, key)
        if is_error(resp):
            fail(f"ask_docs failed for question {q['id']}: {resp['error']}")
        structured = structured_of(resp)
        passages = structured.get("passages") or []
        hit = any(p.get("name") == q["expected_document"] for p in passages)
        citation = passages[0]["citation"] if passages else None
        passage_text = passages[0]["text"] if passages else None
        if hit:
            hits += 1
        answers.append(
            {
                "id": q["id"],
                "question": q["question"],
                "expected_document": q["expected_document"],
                "hit": hit,
                "citation": citation,
                "passage_text": passage_text,
                "index_mode": structured.get("index_mode"),
            }
        )
    return hits, answers


def main():
    started_at = time.time()
    wall_start = time.monotonic()

    with open(GOLD_PATH, encoding="utf-8") as f:
        gold = json.load(f)
    if len(gold) != 6:
        fail(f"expected exactly 6 gold questions in {GOLD_PATH}, found {len(gold)}")

    tenant = None
    key = None
    is_fresh = not (reuse_state and os.path.exists(reuse_state))
    if is_fresh:
        resp = mcp_call(
            "signup", {"name": "docs-qa-owner", "source": SYNTHETIC_SOURCE}
        )
        if is_error(resp):
            fail(f"signup failed: {resp['error']}")
        signup = structured_of(resp)
        tenant, key = signup["tenant"], signup["key"]
    else:
        with open(reuse_state, encoding="utf-8") as f:
            state = json.load(f)
        tenant, key = state["tenant"], state["key"]

    modes = {}
    index_ready_secs = None
    hits = 0
    answers = []

    if embeddings is not None:
        provider_endpoint, model, secret_name = embeddings
        secret_value = os.environ.get("DOCS_QA_EMBED_SECRET_VALUE", "test-embed-secret-value")
        resp = mcp_call("host.secret_set", {"name": secret_name, "value": secret_value}, key)
        if is_error(resp):
            fail(f"host.secret_set failed: {resp['error']}")
        # Requirement 6: the provider is configured FIRST, before any
        # document is put, so the corpus below is indexed in embeddings
        # mode from the start.
        resp = mcp_call(
            "host.docs.index_config",
            {
                "provider": "openai-compatible",
                "endpoint": provider_endpoint,
                "model": model,
                "secret": secret_name,
            },
            key,
        )
        if is_error(resp):
            fail(f"host.docs.index_config (embeddings) failed: {resp['error']}")

        put_corpus(key)
        embed_ready_secs, _ = wait_for_index_ready(key, want_mode="embeddings")
        if is_fresh:
            publish_ask_docs(key)
        embed_hits, embed_answers = ask_gold_questions(tenant, key, gold)
        modes["embeddings"] = {"hits": embed_hits, "total": len(gold), "index_ready_secs": embed_ready_secs}

        resp = mcp_call("host.docs.index_config", {"provider": "none"}, key)
        if is_error(resp):
            fail(f"host.docs.index_config (lexical) failed: {resp['error']}")
        lex_ready_secs, _ = wait_for_index_ready(key, want_mode="lexical")
        lex_hits, lex_answers = ask_gold_questions(tenant, key, gold)
        modes["lexical"] = {"hits": lex_hits, "total": len(gold), "index_ready_secs": lex_ready_secs}

        index_ready_secs = embed_ready_secs
        hits = embed_hits
        answers = embed_answers
        chunks_status_resp = mcp_call("host.docs.status", {}, key)
        chunks = structured_of(chunks_status_resp).get("index", {}).get("chunks", 0)
        tool_calls = len(gold) * 2
        overall_ok = embed_hits >= 5 and lex_hits >= 5
    else:
        put_corpus(key)
        index_ready_secs, status = wait_for_index_ready(key, want_mode="lexical")
        if is_fresh:
            publish_ask_docs(key)
        hits, answers = ask_gold_questions(tenant, key, gold)
        chunks = status.get("index", {}).get("chunks", 0)
        tool_calls = len(gold)
        overall_ok = hits >= 5

    if reuse_state:
        with open(reuse_state, "w", encoding="utf-8") as f:
            json.dump({"tenant": tenant, "key": key}, f)

    wall_time_ms = int((time.monotonic() - wall_start) * 1000)
    distinct_calls = list(dict.fromkeys(call_log))

    receipt = {
        "endpoint": endpoint,
        "tenant": tenant,
        "started_at": started_at,
        "index_ready_secs": index_ready_secs,
        "hits": hits,
        "total_questions": len(gold),
        "answers": answers,
        "quota": {"documents": 8, "chunks": chunks, "tool_calls": tool_calls},
        "wall_time_ms": wall_time_ms,
        "calls": distinct_calls,
    }
    if modes:
        receipt["modes"] = modes

    receipt_path = os.path.join(receipt_dir, f"docs-qa-{int(started_at * 1000)}.json")
    with open(receipt_path, "w", encoding="utf-8") as f:
        json.dump(receipt, f, indent=2)

    print(f"RECEIPT: {receipt_path}")
    print(f"WALL_TIME_MS={wall_time_ms}")
    if overall_ok:
        print(f"RESULT: PASS ({hits}/{len(gold)} hits)")
        sys.exit(0)
    else:
        print(f"RESULT: FAIL ({hits}/{len(gold)} hits)")
        sys.exit(1)


main()
PY
