#!/usr/bin/env bash
# docs-link-check.sh -- PRD-mcphost-docs-external-links-resolve R1/R2/R7.
#
# Extracts http(s) URLs from markdown/text files, dedupes them, and checks
# each one: HEAD, then GET (browser User-Agent) on any non-2xx/3xx answer,
# 10 s timeout, one retry on a timeout/transport error/5xx, 8 in parallel.
# Pass = final status 200-399. Failures print
#     FAIL <status> <url> <file>:<line>
# and the exit code is 1. Allowlisted URLs print SKIP with the reason.
# Non-http schemes (cursor://, vscode:) are neither checked nor reported.
#
# Usage:
#   scripts/docs-link-check.sh [--report] [--stamp-clients] [files...]
#   no files  -> README.md, docs/**/*.md, www/llms.txt, plugin/**
#   --report  -> also write target/docs-links.json  [{url, status, files}]
#   --stamp-clients -> on a fully green run, set `checked` in
#                docs/clients.toml to today for every client whose docs_url
#                passed (the generator's input; never edited by hand); then run
#                `mcphost gen-docs` and `mcphost llms-txt` to render it
# Env: DOCS_LINK_REPORT (report path), DOCS_LINK_TIMEOUT (seconds, default 10), DOCS_LINK_ALLOW (allowlist
# path, default scripts/docs-link-allow.txt). Python stdlib only.
set -euo pipefail
export DOCS_LINK_ROOT
DOCS_LINK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 -I - "$@" <<'PY'
import concurrent.futures as cf, datetime, glob, json, os, re, sys
import urllib.error, urllib.request

ROOT = os.environ["DOCS_LINK_ROOT"]
TIMEOUT = float(os.environ.get("DOCS_LINK_TIMEOUT", "10"))
ALLOW = os.environ.get("DOCS_LINK_ALLOW", os.path.join(ROOT, "scripts/docs-link-allow.txt"))
UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36"
OTHER_SCHEME = re.compile(r"(?<![\w+.-])(?!https?:)[A-Za-z][A-Za-z0-9+.-]*:(?://)?[^\s)>\]\"']*")
URL = re.compile(r"https?://[^\s<>)\]\"'`]+")

args = sys.argv[1:]
stamp = "--stamp-clients" in args
report = "--report" in args
files = [a for a in args if not a.startswith("--")]
if not files:
    os.chdir(ROOT)
    files = ["README.md", "www/llms.txt"]
    files += sorted(glob.glob("docs/**/*.md", recursive=True))
    files += sorted(p for p in glob.glob("plugin/**", recursive=True) if os.path.isfile(p))

allow = []
if os.path.exists(ALLOW):
    for raw in open(ALLOW, encoding="utf-8"):
        raw = raw.rstrip("\n")
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        pat, _, why = raw.partition("  #")
        allow.append((pat.strip(), why.strip() or "allowlisted"))

def allowed(url, path):
    for pat, why in allow:
        if pat and (pat in url or pat in path):
            return why
    return None

found = {}  # url -> [(file, line)]
for f in files:
    try:
        text = open(f, encoding="utf-8").read()
    except (OSError, UnicodeDecodeError):
        continue
    for n, line in enumerate(text.splitlines(), 1):
        line = OTHER_SCHEME.sub(" ", line)
        for m in URL.finditer(line):
            if line[m.end():m.end() + 1] in ("<", "{", "$"):
                continue  # template: https://host.<region>.example.com
            url = m.group(0).rstrip(".,;:!?")
            found.setdefault(url, []).append((f, n))

def attempt(url):
    last = "error"
    for method in ("HEAD", "GET"):
        req = urllib.request.Request(url, method=method, headers={"User-Agent": UA})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
                return r.status
        except urllib.error.HTTPError as e:
            last = e.code
            continue
        except TimeoutError:
            return "timeout"
        except urllib.error.URLError as e:
            return "timeout" if isinstance(e.reason, TimeoutError) else "error"
        except OSError:
            return "error"
    return last

def check(url):
    st = attempt(url)
    if st in ("timeout", "error") or (isinstance(st, int) and st >= 500):
        st = attempt(url)
    return st

todo, skipped = [], []
for url, locs in found.items():
    why = allowed(url, locs[0][0]) or next((w for w in (allowed(url, p) for p, _ in locs) if w), None)
    (skipped if why else todo).append((url, why))
for url, why in skipped:
    f, n = found[url][0]
    print(f"SKIP {url} {f}:{n} ({why})")

results = {}
with cf.ThreadPoolExecutor(max_workers=8) as ex:
    for (url, _), st in zip(todo, ex.map(lambda t: check(t[0]), todo)):
        results[url] = st

failed = 0
for url, _ in todo:
    st = results[url]
    if isinstance(st, int) and 200 <= st < 400:
        print(f"ok {st} {url}")
    else:
        failed += 1
        f, n = found[url][0]
        print(f"FAIL {st} {url} {f}:{n}")
print(f"docs-link-check: {len(found)} urls, {len(skipped)} skipped, {failed} failed")

if report:
    out = os.environ.get("DOCS_LINK_REPORT", os.path.join(ROOT, "target", "docs-links.json"))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    rows = [{"url": u, "status": results.get(u, "skipped"), "files": sorted({p for p, _ in found[u]})} for u in found]
    json.dump(rows, open(out, "w"), indent=2)
    print(f"report: {out}")

if stamp and not failed:
    path = os.path.join(ROOT, "docs/clients.toml")
    today = datetime.date.today().isoformat()
    out, cur = [], None
    for ln in open(path, encoding="utf-8").read().split("\n"):
        m = re.match(r'docs_url\s*=\s*"([^"]+)"', ln)
        if m:
            cur = m.group(1)
        elif ln.startswith("[["):
            cur = None
        elif ln.startswith("checked") and cur is not None and isinstance(results.get(cur), int):
            ln = f'checked = "{today}"'
        out.append(ln)
    open(path, "w", encoding="utf-8").write("\n".join(out))
    print(f"stamped docs/clients.toml checked = {today}")
sys.exit(1 if failed else 0)
PY
