#!/usr/bin/env bash
# docs-link-check.sh -- PRD-mcphost-docs-external-links-resolve R1/R2/R7.
#
# Extracts http(s) URLs from markdown/text files, dedupes them, and checks
# each one: HEAD, then GET (browser User-Agent) on any non-2xx/3xx answer,
# 10 s timeout, one retry on a timeout/transport error/5xx, 8 in parallel.
# Pass = final status 200-399. Only a DEFINITIVE answer fails the run: a
# 4xx (404/410/...) prints
#     FAIL <status> <url> <file>:<line>
# and the exit code is 1. A `timeout`, a transport `error` or a 5xx that
# survives the retry says nothing about the link (an external host's latency
# must not fail a required check on a PR that never touched docs), so it
# prints
#     WARN <status> <url> <file>:<line>
# and the exit code stays 0. Allowlisted URLs print SKIP with the reason.
# A URL whose host is in the known-good cache (scripts/docs-link-known-good.txt,
# one host per line, `# refreshed <date>` header) is not requested and prints
#     OK cached <url> <file>:<line>
# Non-http schemes (cursor://, vscode:) are neither checked nor reported.
#
# Usage:
#   scripts/docs-link-check.sh [--report] [--stamp-clients] [--refresh-known-good] [files...]
#   no files  -> README.md, docs/**/*.md, www/llms.txt, plugin/**
#   --report  -> also write target/docs-links.json  [{url, status, files}]
#   --stamp-clients -> on a fully green run, set `checked` in
#                docs/clients.toml to today for every client whose docs_url
#                passed (the generator's input; never edited by hand); then run
#                `mcphost gen-docs` and `mcphost llms-txt` to render it. The
#                known-good cache is bypassed so every stamp is a live check
#   --refresh-known-good -> bypass the cache, re-check every URL (default file
#                set, or the files given) whose host is listed, and rewrite the
#                known-good file: a host stays if every URL on it resolved
#                (200-399); it is dropped, with `DROP <host> <why>`, if one
#                answered 4xx, none resolved, or no file references it. Run by
#                a scheduled job, never by CI's docs check; always exits 0
# Env: DOCS_LINK_REPORT (report path), DOCS_LINK_TIMEOUT (seconds, default 10), DOCS_LINK_ALLOW (allowlist
# path, default scripts/docs-link-allow.txt), DOCS_LINK_KNOWN_GOOD (known-good
# host list, default scripts/docs-link-known-good.txt). Python stdlib only.
set -euo pipefail
export DOCS_LINK_ROOT
DOCS_LINK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 -I - "$@" <<'PY'
import concurrent.futures as cf, datetime, glob, json, os, re, sys
import urllib.error, urllib.parse, urllib.request

ROOT = os.environ["DOCS_LINK_ROOT"]
TIMEOUT = float(os.environ.get("DOCS_LINK_TIMEOUT", "10"))
ALLOW = os.environ.get("DOCS_LINK_ALLOW", os.path.join(ROOT, "scripts/docs-link-allow.txt"))
KNOWN_GOOD = os.environ.get("DOCS_LINK_KNOWN_GOOD", os.path.join(ROOT, "scripts/docs-link-known-good.txt"))
UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36"
OTHER_SCHEME = re.compile(r"(?<![\w+.-])(?!https?:)[A-Za-z][A-Za-z0-9+.-]*:(?://)?[^\s)>\]\"']*")
URL = re.compile(r"https?://[^\s<>)\]\"'`]+")

args = sys.argv[1:]
stamp = "--stamp-clients" in args
report = "--report" in args
refresh = "--refresh-known-good" in args
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

def read_hosts(path):
    hosts = []
    if os.path.exists(path):
        for raw in open(path, encoding="utf-8"):
            h = raw.partition("#")[0].strip().lower()
            if h:
                hosts.append(h)
    return hosts

listed = read_hosts(KNOWN_GOOD)
# --stamp-clients and --refresh-known-good need live answers, never the cache.
cached = set() if (stamp or refresh) else set(listed)

def host_of(url):
    try:
        return (urllib.parse.urlsplit(url).hostname or "").lower()
    except ValueError:
        return ""

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

KNOWN_GOOD_HEADER = """# refreshed {today}
# docs-link-known-good.txt -- hosts whose links resolved at the last refresh
# (scripts/docs-link-check.sh; one host per line). A URL on a listed host is
# not requested: it prints `OK cached`. Rewritten only by
# `scripts/docs-link-check.sh --refresh-known-good`, which re-checks every
# URL on each listed host and drops a host that stops resolving.
"""

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

todo, skipped, hits = [], [], []
for url, locs in found.items():
    why = allowed(url, locs[0][0]) or next((w for w in (allowed(url, p) for p, _ in locs) if w), None)
    if why:
        skipped.append((url, why))
    elif host_of(url) in cached:
        hits.append(url)
    elif refresh and host_of(url) not in listed:
        continue  # refresh re-checks only the listed hosts
    else:
        todo.append((url, why))
for url, why in skipped:
    f, n = found[url][0]
    print(f"SKIP {url} {f}:{n} ({why})")
for url in hits:
    f, n = found[url][0]
    print(f"OK cached {url} {f}:{n}")

results = {}
with cf.ThreadPoolExecutor(max_workers=8) as ex:
    for (url, _), st in zip(todo, ex.map(lambda t: check(t[0]), todo)):
        results[url] = st

failed = warned = 0
for url, _ in todo:
    st = results[url]
    f, n = found[url][0]
    if isinstance(st, int) and 200 <= st < 400:
        print(f"ok {st} {url}")
    elif st in ("timeout", "error") or (isinstance(st, int) and st >= 500):
        # No definitive answer from the host: warn, never fail.
        warned += 1
        print(f"WARN {st} {url} {f}:{n}")
    else:
        failed += 1
        print(f"FAIL {st} {url} {f}:{n}")
print(f"docs-link-check: {len(found)} urls, {len(skipped)} skipped, {len(hits)} cached, {warned} warned, {failed} failed")

if refresh:
    today = datetime.date.today().isoformat()
    verdict = {}  # host -> list of statuses of the URLs checked on it
    for url, _ in todo:
        verdict.setdefault(host_of(url), []).append(results[url])
    keep = []
    for h in sorted(set(listed)):
        sts = verdict.get(h)
        if not sts:
            print(f"DROP {h} unreferenced")
        elif any(isinstance(s, int) and 400 <= s < 500 for s in sts):
            print(f"DROP {h} dead-link {sorted({s for s in sts if isinstance(s, int) and 400 <= s < 500})[0]}")
        elif not any(isinstance(s, int) and 200 <= s < 400 for s in sts):
            print(f"DROP {h} unresolved {sts[0]}")
        else:
            keep.append(h)
            print(f"KEEP {h}")
    with open(KNOWN_GOOD, "w", encoding="utf-8") as fh:
        fh.write(KNOWN_GOOD_HEADER.format(today=today))
        fh.writelines(h + "\n" for h in keep)
    print(f"refreshed {KNOWN_GOOD}: {len(keep)} kept, {len(set(listed)) - len(keep)} dropped")
    sys.exit(0)

if report:
    out = os.environ.get("DOCS_LINK_REPORT", os.path.join(ROOT, "target", "docs-links.json"))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    rows = [{"url": u, "status": results.get(u, "cached" if u in hits else "skipped"), "files": sorted({p for p, _ in found[u]})} for u in found]
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
