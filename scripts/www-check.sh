#!/usr/bin/env bash
# www-check.sh — minimal honest validation for the static site under www/.
# Added project-locally for PRD-mcphost-healthz-minimal's vti-plan gap fix:
# www/**.html and www/llms.txt had no build tooling and no proof-lane, so
# 5 changed paths from the sibling www-redesign commits were unrouted in
# proof-lanes.toml. There is no HTML linter/build step in this repo (no
# node/npm, no tidy/xmllint on this host) — rather than invent a fake pass,
# this uses Python's stdlib html.parser (always available, matches the
# infer-data lane's "python stdlib" precedent) to catch real parse failures
# and encoding corruption, and confirms llms.txt decodes as UTF-8 text.
# Not a style/accessibility linter — just "these files are not broken."
set -euo pipefail
cd "$(dirname "$0")/.."

status=0

for f in www/*.html; do
  python3 - "$f" <<'PY' || status=1
import sys
from html.parser import HTMLParser

path = sys.argv[1]
with open(path, "rb") as fh:
    data = fh.read()

try:
    text = data.decode("utf-8")
except UnicodeDecodeError as e:
    print(f"{path}: not valid UTF-8: {e}", file=sys.stderr)
    sys.exit(1)

if not text.strip():
    print(f"{path}: empty file", file=sys.stderr)
    sys.exit(1)

class Checker(HTMLParser):
    def error(self, message):
        raise AssertionError(message)

try:
    Checker(convert_charrefs=True).feed(text)
except Exception as e:
    print(f"{path}: HTML parse failure: {e}", file=sys.stderr)
    sys.exit(1)

if "<html" not in text.lower():
    print(f"{path}: missing <html> element", file=sys.stderr)
    sys.exit(1)
PY
done

for f in www/llms.txt www/llms-full.txt; do
  [ -f "$f" ] || continue
  python3 - "$f" <<'PY' || status=1
import sys
path = sys.argv[1]
with open(path, "rb") as fh:
    data = fh.read()
try:
    text = data.decode("utf-8")
except UnicodeDecodeError as e:
    print(f"{path}: not valid UTF-8: {e}", file=sys.stderr)
    sys.exit(1)
if not text.strip():
    print(f"{path}: empty file", file=sys.stderr)
    sys.exit(1)
PY
done

# www/llms-full.txt (requirement 7, PRD-mcphost-agent-findability) is
# generated from README.md + docs/kinds + docs/benchmarks + docs/receipts;
# fail if it has drifted from its source, same shape as gen-agent-docs.sh
# --check for README.md/www/llms.txt.
if [ -f scripts/gen-llms-full.sh ]; then
  scripts/gen-llms-full.sh --check || status=1
fi

# PRD-mcphost-examples-gallery-page R6: when WWW_CHECK_HOST names a running
# host (e.g. http://127.0.0.1:8080), fetch /examples, /examples/<first> and
# /examples.json from it and fail on a non-200 or an empty card list. Unset,
# there is no host to fetch from and this step is skipped.
if [ -n "${WWW_CHECK_HOST:-}" ]; then
  python3 - "$WWW_CHECK_HOST" <<'PY' || status=1
import json
import sys
import urllib.error
import urllib.request

base = sys.argv[1].rstrip("/")


def fetch(path):
    try:
        with urllib.request.urlopen(base + path, timeout=15) as r:
            return r.status, r.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        return e.code, ""
    except OSError as e:
        print(f"{base}{path}: {e}", file=sys.stderr)
        return 0, ""


failed = False
code, html = fetch("/examples")
cards = html.count('class="card"')
if code != 200:
    print(f"/examples: HTTP {code}", file=sys.stderr)
    failed = True
elif cards == 0:
    print("/examples: zero cards", file=sys.stderr)
    failed = True

code, body = fetch("/examples.json")
first = None
if code != 200:
    print(f"/examples.json: HTTP {code}", file=sys.stderr)
    failed = True
else:
    try:
        rows = json.loads(body)["examples"]
    except (ValueError, KeyError, TypeError):
        rows = []
    if not rows:
        print("/examples.json: no examples", file=sys.stderr)
        failed = True
    else:
        first = rows[0]["name"]

if first is not None:
    code, _ = fetch("/examples/" + first)
    if code != 200:
        print(f"/examples/{first}: HTTP {code}", file=sys.stderr)
        failed = True

sys.exit(1 if failed else 0)
PY
fi

exit "$status"
