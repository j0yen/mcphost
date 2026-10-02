#!/usr/bin/env bash
# sandbox-api-doc-check.sh -- PRD-mcphost-sandbox-bridge-discoverability
# requirement 7 (AC7): docs/kinds/python.md, www/llms.txt, and
# plugin/skills/mcphost/SKILL.md must each carry the same module table --
# one bullet per `mcphost.*` submodule python-kind tool code can import,
# generated from src/kinds/python.rs's own BRIDGE_MODULES constant (the
# registration constant requirement 1's host.quickstart sandbox_api field
# also reads from -- see kinds::python::build_sandbox_api) rather than a
# second, hand-copied list that can drift from it.
#
# Usage:
#   scripts/sandbox-api-doc-check.sh          check: exit 0 and report
#                                              modules=<n> docs_in_sync=<n>,
#                                              or exit 1 naming the first
#                                              missing module and file.
#   scripts/sandbox-api-doc-check.sh --write  regenerate the marked block
#                                              in every target file.
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="src/kinds/python.rs"
TARGETS=("docs/kinds/python.md" "www/llms.txt" "plugin/skills/mcphost/SKILL.md")
START="<!-- mcphost-sandbox-api:start -->"
END="<!-- mcphost-sandbox-api:end -->"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

if [ ! -f "$SOURCE" ]; then
  echo "sandbox-api-doc-check.sh: missing $SOURCE" >&2
  exit 2
fi

# Renders the marked block's own content (markers included) from
# BRIDGE_MODULES's own `name: "..."` / `purpose: "..."` pairs, parsed back
# out of the Rust source text -- never a second, hand-maintained list.
render_block() {
  python3 - "$SOURCE" "$START" "$END" <<'PY'
import re
import sys

source_path, start, end = sys.argv[1:4]
with open(source_path, encoding="utf-8") as f:
    text = f.read()

m = re.search(r"pub const BRIDGE_MODULES: &\[BridgeModule\] = &\[(.*?)\n\];", text, re.S)
if not m:
    print("sandbox-api-doc-check.sh: could not find BRIDGE_MODULES in " + source_path, file=sys.stderr)
    sys.exit(2)
block = m.group(1)

names = re.findall(r'name: "([a-z_]+)"', block)
purposes = re.findall(r'purpose: "([^"]*)"', block)
if not names or len(names) != len(purposes):
    print(
        f"sandbox-api-doc-check.sh: BRIDGE_MODULES parse mismatch: {len(names)} names, "
        f"{len(purposes)} purposes",
        file=sys.stderr,
    )
    sys.exit(2)

print(start)
print(
    "A python tool's own code does `import mcphost` to reach this tenant's "
    "data without a second tool call:"
)
print()
for name, purpose in zip(names, purposes):
    print(f"- `mcphost.{name}` -- {purpose}")
print(end)
PY
}

# Replaces the text between START and END (inclusive) in $1 with $2 --
# same mechanism gen-agent-docs.sh/gen-docs-sharing.sh already use. Fails
# loudly if the file has zero or more than one start/end marker pair.
apply_to_file() {
  local file="$1" rendered="$2"
  local n_start n_end
  n_start=$(grep -Fc "$START" "$file")
  n_end=$(grep -Fc "$END" "$file")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "sandbox-api-doc-check.sh: $file must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    return 2
  fi
  local tmp
  tmp="$(mktemp)"
  python3 - "$file" "$START" "$END" "$rendered" > "$tmp" <<'PY'
import sys

file_path, start_marker, end_marker, rendered = sys.argv[1:5]
with open(file_path, encoding="utf-8") as f:
    text = f.read()
start_idx = text.index(start_marker)
end_idx = text.index(end_marker) + len(end_marker)
sys.stdout.write(text[:start_idx] + rendered + text[end_idx:])
PY
  mv "$tmp" "$file"
}

RENDERED="$(render_block)" || exit $?
MODULE_COUNT=$(printf '%s\n' "$RENDERED" | grep -c '^- `mcphost\.')

if [ "$MODE" = "write" ]; then
  for f in "${TARGETS[@]}"; do
    apply_to_file "$f" "$RENDERED" || exit $?
  done
  echo "sandbox-api-doc-check.sh: wrote modules=${MODULE_COUNT} to ${#TARGETS[@]} files"
  exit 0
fi

# Check mode: each target's own marked block must equal $RENDERED exactly;
# a drifted or missing module is named with the file it's missing from.
FAILURES=0
for f in "${TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "sandbox-api-doc-check.sh: $f does not exist" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  n_start=$(grep -Fc "$START" "$f")
  n_end=$(grep -Fc "$END" "$f")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "sandbox-api-doc-check.sh: $f must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  ACTUAL=$(python3 - "$f" "$START" "$END" <<'PY'
import sys

file_path, start_marker, end_marker = sys.argv[1:4]
with open(file_path, encoding="utf-8") as f:
    text = f.read()
start_idx = text.index(start_marker)
end_idx = text.index(end_marker) + len(end_marker)
sys.stdout.write(text[start_idx:end_idx])
PY
  )
  while IFS= read -r name; do
    if ! printf '%s\n' "$ACTUAL" | grep -qF "\`mcphost.${name}\`"; then
      echo "sandbox-api-doc-check.sh: $f is missing mcphost.${name}" >&2
      FAILURES=$((FAILURES + 1))
    fi
  done < <(printf '%s\n' "$RENDERED" | grep -o '`mcphost\.[a-z_]*`' | sed -e 's/`mcphost\.//' -e 's/`//')
done

if [ "$FAILURES" -gt 0 ]; then
  echo "sandbox-api-doc-check.sh: FAILED (${FAILURES} issue(s))" >&2
  exit 1
fi

echo "modules=${MODULE_COUNT} docs_in_sync=${#TARGETS[@]}"
exit 0
