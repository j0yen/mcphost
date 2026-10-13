#!/usr/bin/env bash
# heal-doc-check.sh -- docs/self-healing.md carries two generated tables, each
# derived from its single source rather than a hand-copied list:
#   * golden signals (PRD-mcphost-golden-signals-self-probe-b R6, AC1): one row
#     per `[signals.*]` table of heal.toml.
#   * heal actions (PRD-mcphost-heal-action-registry-and-ledger R6, AC1): one
#     row per `HealAction`, from the `heal_actions!` invocation in src/heal.rs.
# A key or variant added to or removed from its source fails the check until
# the table is regenerated.
#
# Usage:
#   scripts/heal-doc-check.sh                  check both: exit 0 and report
#                                               signals=<n> docs_in_sync=1 and
#                                               actions=<n> docs_in_sync=<n>,
#                                               or exit 1 naming the drift.
#   scripts/heal-doc-check.sh --write          regenerate both marked blocks.
#   scripts/heal-doc-check.sh --source <file>  check actions against another
#                                               copy of heal.rs (drift test).
#
# Env overrides (tests run the script against scratch copies):
#   HEAL_TOML (default heal.toml)   HEAL_DOC (default docs/self-healing.md)
set -uo pipefail
cd "$(dirname "$0")/.."

TOML="${HEAL_TOML:-heal.toml}"
DOC="${HEAL_DOC:-docs/self-healing.md}"
SOURCE="src/heal.rs"
TARGET="$DOC"
START="<!-- mcphost-heal-actions:start -->"
END="<!-- mcphost-heal-actions:end -->"
MODE="check"
while [ $# -gt 0 ]; do
  case "$1" in
    --write) MODE="write" ;;
    --source) shift; SOURCE="${1:-}" ;;
    *) echo "heal-doc-check.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

for f in "$TOML" "$DOC" "$SOURCE"; do
  if [ ! -f "$f" ]; then
    echo "heal-doc-check.sh: missing $f" >&2
    exit 2
  fi
done

check_signals() {
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
}

render_block() {
  python3 - "$SOURCE" "$START" "$END" <<'PY'
import re
import sys

source_path, start, end = sys.argv[1:4]
text = open(source_path, encoding="utf-8").read()
m = re.search(r"heal_actions! \{(.*?)\n\}", text, re.S)
if not m:
    print("heal-doc-check.sh: could not find heal_actions! in " + source_path, file=sys.stderr)
    sys.exit(2)
rows = re.findall(r'^\s*\w+ => \("(\w+)", (\d+), "(.*)"\),$', m.group(1), re.M)
if not rows:
    print("heal-doc-check.sh: no actions parsed from " + source_path, file=sys.stderr)
    sys.exit(2)
print(start)
print("| action | max per hour | destructive | summary |")
print("|---|---|---|---|")
for name, mx, summary in rows:
    print(f"| `{name}` | {mx} | no | {summary} |")
print(end)
PY
}

BLOCK="$(render_block)" || exit 2

write_actions() {
  python3 - "$TARGET" "$START" "$END" "$BLOCK" <<'PY'
import sys

path, start, end, block = sys.argv[1:5]
text = open(path, encoding="utf-8").read()
a, b = text.find(start), text.find(end)
if a < 0 or b < 0:
    print("heal-doc-check.sh: markers missing in " + path, file=sys.stderr)
    sys.exit(2)
open(path, "w", encoding="utf-8").write(text[:a] + block + text[b + len(end):])
PY
}

check_actions() {
  n=0
  while IFS= read -r name; do
    n=$((n + 1))
    if ! grep -q "^| \`$name\` |" "$TARGET"; then
      echo "heal-doc-check.sh: action $name missing from $TARGET" >&2
      return 1
    fi
  done < <(printf '%s\n' "$BLOCK" | sed -n 's/^| `\([a-z_]*\)` |.*/\1/p')
  echo "actions=$n docs_in_sync=$n"
}

check_signals || exit $?
if [ "$MODE" = "write" ]; then
  write_actions
  exit $?
fi
check_actions
