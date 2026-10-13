#!/usr/bin/env bash
# heal-doc-check.sh -- PRD-mcphost-golden-signals-self-probe-b R6 (AC1):
# docs/self-healing.md carries one table of the golden signals, generated from
# the `[signals.*]` tables of heal.toml (never a second hand-maintained list).
# A key added to or removed from the toml fails the check until the table is
# regenerated.
#
# Usage:
#   scripts/heal-doc-check.sh          check: exit 0 and report
#                                       signals=<n> docs_in_sync=1, or exit 1.
#   scripts/heal-doc-check.sh --write  regenerate the marked block.
#
# Env overrides (tests run the script against scratch copies):
#   HEAL_TOML (default heal.toml)   HEAL_DOC (default docs/self-healing.md)
set -uo pipefail
cd "$(dirname "$0")/.."

TOML="${HEAL_TOML:-heal.toml}"
DOC="${HEAL_DOC:-docs/self-healing.md}"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

for f in "$TOML" "$DOC"; do
  if [ ! -f "$f" ]; then
    echo "heal-doc-check.sh: missing $f" >&2
    exit 2
  fi
done

python3 - "$TOML" "$DOC" "$MODE" <<'PY'
import sys
import tomllib

toml_path, doc_path, mode = sys.argv[1:4]
START = "<!-- mcphost-heal-signals:start -->"
END = "<!-- mcphost-heal-signals:end -->"
with open(toml_path, "rb") as fh:
    signals = tomllib.load(fh).get("signals", {})

def num(v):
    return str(int(v)) if float(v).is_integer() else str(v)

lines = [START, "| signal | amber | red | for_s | unit |", "|---|---|---|---|---|"]
for name, t in signals.items():
    lines.append(f"| `{name}` | {num(t['amber'])} | {num(t['red'])} | {num(t['for_s'])} | {t['unit']} |")
lines.append(END)
rendered = "\n".join(lines)

text = open(doc_path, encoding="utf-8").read()
if text.count(START) != 1 or text.count(END) != 1:
    print(f"heal-doc-check.sh: {doc_path} must have exactly one {START} / {END} pair", file=sys.stderr)
    sys.exit(2)
a = text.index(START)
b = text.index(END) + len(END)
if mode == "write":
    open(doc_path, "w", encoding="utf-8").write(text[:a] + rendered + text[b:])
    print(f"heal-doc-check.sh: wrote signals={len(signals)}")
    sys.exit(0)
if text[a:b] == rendered:
    print(f"heal-doc-check.sh: signals={len(signals)} docs_in_sync=1")
    sys.exit(0)
print(f"heal-doc-check.sh: {doc_path} drifted from {toml_path}; run --write", file=sys.stderr)
sys.exit(1)
PY
