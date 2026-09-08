#!/usr/bin/env bash
# gen-agent-docs.sh — README.md and www/llms.txt both carry the agent
# quickstart + measured proof points (PRD-mcphost-agent-findability
# requirement 1: "llms.txt is generated from the same source [as the
# README] so the two cannot drift"). docs/agent-quickstart.md is that one
# source; this script writes its content, byte-for-byte, into the marked
# block in both files.
#
# Usage:
#   scripts/gen-agent-docs.sh          regenerate both marked blocks in place
#   scripts/gen-agent-docs.sh --check  exit 1 if either file's marked block
#                                      is not exactly docs/agent-quickstart.md
#                                      (drift check; wired into CI-equivalent
#                                      local checks the way docs-check.sh is)
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="docs/agent-quickstart.md"
START="<!-- agent-quickstart:start -->"
END="<!-- agent-quickstart:end -->"
MODE="${1:-write}"

if [ ! -f "$SOURCE" ]; then
  echo "gen-agent-docs.sh: missing $SOURCE" >&2
  exit 2
fi

# Renders the marked block (including its own markers) for one target file,
# given the marker comment style to use around the shared content. Both
# README.md and www/llms.txt use the same HTML-comment markers so the
# provenance is visible in either file.
render_block() {
  printf '%s\n' "$START"
  cat "$SOURCE"
  printf '%s\n' "$END"
}

# Replaces the text between START and END (inclusive) in $1 with the
# rendered block. Fails loudly if the file has zero or more than one
# start/end marker pair -- this must be mechanical, not best-effort.
apply_to_file() {
  local file="$1"
  local n_start n_end
  n_start=$(grep -Fc "$START" "$file")
  n_end=$(grep -Fc "$END" "$file")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "gen-agent-docs.sh: $file must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    return 2
  fi

  local tmp
  tmp="$(mktemp)"
  python3 - "$file" "$SOURCE" "$START" "$END" > "$tmp" <<'PY'
import sys
file_path, source_path, start_marker, end_marker = sys.argv[1:5]
with open(file_path, encoding="utf-8") as f:
    text = f.read()
with open(source_path, encoding="utf-8") as f:
    source = f.read()

start_idx = text.index(start_marker)
end_idx = text.index(end_marker) + len(end_marker)

block = start_marker + "\n" + source.rstrip("\n") + "\n" + end_marker
new_text = text[:start_idx] + block + text[end_idx:]
sys.stdout.write(new_text)
PY
  if [ -s "$tmp" ]; then
    mv "$tmp" "$file"
  else
    rm -f "$tmp"
    echo "gen-agent-docs.sh: rendering $file produced no output" >&2
    return 1
  fi
}

check_file() {
  local file="$1"
  local before after status
  before="$(mktemp)"
  after="$(mktemp)"
  cp "$file" "$before"
  if ! apply_to_file "$file"; then
    rm -f "$before" "$after"
    return 1
  fi
  cp "$file" "$after"
  if diff -q "$before" "$after" > /dev/null; then
    status=0
  else
    echo "gen-agent-docs.sh --check: $file is stale against $SOURCE" >&2
    # restore the file: check mode must not leave a write behind
    cp "$before" "$file"
    status=1
  fi
  rm -f "$before" "$after"
  return "$status"
}

TARGETS="README.md www/llms.txt"
overall=0

case "$MODE" in
  --check)
    for f in $TARGETS; do
      check_file "$f" || overall=1
    done
    ;;
  write|"")
    for f in $TARGETS; do
      apply_to_file "$f" || overall=1
    done
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac

exit "$overall"
