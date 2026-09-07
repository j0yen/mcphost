#!/usr/bin/env bash
# docs-check.sh — minimal honest validation for prose docs under docs/.
# Added project-locally for PRD-mcphost-healthz-minimal's vti-plan gap fix:
# docs/kinds/*.md and docs/receipts/*.md had no build tooling and no
# proof-lane, so 4 of the 5 unrouted paths in that gate run were here (the
# 5th, scripts/www-check.sh, is folded into the `www` lane instead since
# it's that lane's own proof script). There is no markdown linter in this
# repo (no node/npm, no markdownlint on this host) — matching the `www`
# lane's precedent, this uses only the Python stdlib (always available) to
# confirm each file decodes as non-empty UTF-8 text. Not a style/prose
# linter -- just "these files are not broken."
set -euo pipefail
cd "$(dirname "$0")/.."

status=0

check_utf8_nonempty() {
  local f="$1"
  python3 - "$f" <<'PY' || return 1
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
}

for f in docs/kinds/*.md docs/receipts/*.md; do
  [ -e "$f" ] || continue
  check_utf8_nonempty "$f" || status=1
done

exit "$status"
