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

if [ -f www/llms.txt ]; then
  python3 - www/llms.txt <<'PY' || status=1
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
fi

exit "$status"
