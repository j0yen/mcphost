#!/usr/bin/env bash
# launch-docs-check.sh — PRD-mcphost-agent-findability wave-1 listings prep
# (chore/listings-wave1). docs/launch/** has no build tooling of its own
# (same gap class the `docs`/`www` lanes already named: no node/npm, no
# markdown linter on this host) -- same honest floor as scripts/docs-check.sh,
# duplicated rather than folded into that lane's own required_commands
# because tests/lanecov_ac04_existing_lanes_unchanged.rs pins every
# pre-existing lane's required_commands as an immutable baseline (see the
# `sharing-docs` lane's own comment for the same reasoning).
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

for f in docs/launch/*.md; do
  [ -e "$f" ] || continue
  check_utf8_nonempty "$f" || status=1
done

exit "$status"
