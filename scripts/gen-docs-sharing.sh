#!/usr/bin/env bash
# gen-docs-sharing.sh -- PRD-mcphost-shared-tool-call-path requirement 4.
#
# `www/llms.txt`'s "Share a tool, not a key" section is the one place an
# agent learns the `host.tool_call(name="<owner_namespace>.<tool>")` call
# form -- the exact form this PRD makes actually work. `docs/sharing.md` is
# the single source of that section's text; this script copies it, byte for
# byte, into the marked block in `www/llms.txt`, same "one canonical source,
# regenerate the marked block in every target" shape `gen-agent-docs.sh`
# already uses for `docs/agent-quickstart.md` -> `README.md`/`www/llms.txt`.
# A hand-edit to either file, or a docs/sharing.md edit nobody regenerated
# from, is caught by `--check` rather than silently drifting the way this
# PRD's own problem statement describes.
#
# Usage:
#   scripts/gen-docs-sharing.sh          regenerate the marked block in place
#   scripts/gen-docs-sharing.sh --check  exit 1, naming the first differing
#                                        line of www/llms.txt, if its marked
#                                        block has drifted from docs/sharing.md
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="docs/sharing.md"
TARGET="www/llms.txt"
START="<!-- sharing:start -->"
END="<!-- sharing:end -->"
MODE="${1:-write}"

if [ ! -f "$SOURCE" ]; then
  echo "gen-docs-sharing.sh: missing $SOURCE" >&2
  exit 2
fi

render_block() {
  printf '%s\n' "$START"
  cat "$SOURCE"
  printf '%s\n' "$END"
}

# Replaces the text between START and END (inclusive) in TARGET with the
# rendered block. Fails loudly if TARGET has zero or more than one
# start/end marker pair.
apply() {
  local n_start n_end
  n_start=$(grep -Fc "$START" "$TARGET")
  n_end=$(grep -Fc "$END" "$TARGET")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "gen-docs-sharing.sh: $TARGET must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    return 2
  fi

  local tmp
  tmp="$(mktemp)"
  python3 - "$TARGET" "$SOURCE" "$START" "$END" > "$tmp" <<'PY'
import sys
target_path, source_path, start_marker, end_marker = sys.argv[1:5]
with open(target_path, encoding="utf-8") as f:
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
    mv "$tmp" "$TARGET"
  else
    rm -f "$tmp"
    echo "gen-docs-sharing.sh: rendering $TARGET produced no output" >&2
    return 1
  fi
}

# --check: regenerates into a scratch copy, then reports the first
# differing line NUMBER (not just "stale") -- AC5's own requirement -- via
# `diff`'s unified-format `@@ -N` line-number markers, restoring TARGET
# unconditionally so check mode never leaves a write behind.
check() {
  local before after
  before="$(mktemp)"
  after="$(mktemp)"
  cp "$TARGET" "$before"
  if ! apply; then
    rm -f "$before" "$after"
    return 1
  fi
  cp "$TARGET" "$after"
  if diff -q "$before" "$after" > /dev/null; then
    rm -f "$before" "$after"
    return 0
  fi
  local first_line
  first_line=$(diff "$before" "$after" | head -1 | grep -oE '^[0-9]+')
  echo "gen-docs-sharing.sh --check: $TARGET line $first_line has drifted from $SOURCE" >&2
  diff -u "$before" "$after" >&2 || true
  cp "$before" "$TARGET"
  rm -f "$before" "$after"
  return 1
}

case "$MODE" in
  --check)
    check
    ;;
  write|"")
    apply
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac
