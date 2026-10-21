#!/usr/bin/env bash
# signup-args-doc-check.sh -- plugin/skills/mcphost/SKILL.md and www/skill.md
# must each carry the same `signup` argument block, rendered by
# `mcphost signup-args-block` (control::render_signup_args_block) from the
# control::SIGNUP_ARGS const -- the same const the `signup` descriptor and the
# handler's argument reads derive from -- never a hand-copied argument list.
# (skill-walk 19/21, chain run 211800Z: both files promised `signup(source)`
# while the descriptor never declared `source`.)
#
# Usage:
#   scripts/signup-args-doc-check.sh          check: exit 0, or exit 1 naming
#                                              the file whose block differs.
#   scripts/signup-args-doc-check.sh --write  regenerate the marked block in
#                                              every target file.
# sandbox-api-doc-check.sh runs this in both modes, so the CI regen-check job
# (which runs sandbox-api-doc-check.sh) covers it.
set -uo pipefail
cd "$(dirname "$0")/.."

TARGETS=("plugin/skills/mcphost/SKILL.md" "www/skill.md")
START="<!-- mcphost-signup-args:start -->"
END="<!-- mcphost-signup-args:end -->"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

# Same lookup as sandbox-api-doc-check.sh's mcphost_bin: $MCPHOST_BIN, else
# $CARGO_BIN_EXE_mcphost, else a prebuilt binary newer than src/, else a
# cargo run of the mcphost bin.
mcphost_bin() {
  local bin="${MCPHOST_BIN:-${CARGO_BIN_EXE_mcphost:-}}"
  if [ -n "$bin" ]; then
    "$bin" "$@"
    return $?
  fi
  local cand dir
  for dir in target ${CARGO_TARGET_DIR:+"$CARGO_TARGET_DIR"}; do
    for cand in "$dir/release/mcphost" "$dir/debug/mcphost"; do
      if [ -x "$cand" ] && [ -z "$(find src Cargo.toml Cargo.lock -newer "$cand" -print -quit 2>/dev/null)" ]; then
        "$cand" "$@"
        return $?
      fi
    done
  done
  cargo run -q --bin mcphost -- "$@"
}

# Rendered into a variable first and validated before any file is touched.
RENDERED="$(mcphost_bin signup-args-block)" || {
  echo "signup-args-doc-check.sh: could not render the block (mcphost signup-args-block failed)" >&2
  exit 2
}
case "$RENDERED" in
  "$START"*"$END") ;;
  *)
    echo "signup-args-doc-check.sh: signup-args-block output is empty or lacks its $START / $END markers; writing nothing" >&2
    exit 2
    ;;
esac
if ! grep -qE '^- `[a-z_]+` \(' <<<"$RENDERED"; then
  echo "signup-args-doc-check.sh: signup-args-block output has no argument bullets; writing nothing" >&2
  exit 2
fi

FAILURES=0
for f in "${TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "signup-args-doc-check.sh: $f does not exist" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  n_start=$(grep -Fc "$START" "$f")
  n_end=$(grep -Fc "$END" "$f")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "signup-args-doc-check.sh: $f must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  if [ "$MODE" = "write" ]; then
    tmp="$(mktemp "$f.XXXXXX")" || exit 2  # same dir: mv is an atomic rename
    if ! python3 - "$f" "$START" "$END" "$RENDERED" > "$tmp" <<'PY'
import sys

path, start, end, block = sys.argv[1:5]
with open(path, encoding="utf-8") as f:
    text = f.read()
s, e = text.index(start), text.index(end) + len(end)
if e <= s:
    sys.exit("end marker precedes start marker")
sys.stdout.write(text[:s] + block + text[e:])
PY
    then
      rm -f "$tmp"
      echo "signup-args-doc-check.sh: refusing to write $f: the marker splice failed; file left untouched" >&2
      exit 2
    fi
    if [ ! -s "$tmp" ]; then
      rm -f "$tmp"
      echo "signup-args-doc-check.sh: refusing to write $f: the splice produced an empty file; file left untouched" >&2
      exit 2
    fi
    chmod 644 "$tmp"
    mv "$tmp" "$f"
    continue
  fi
  ACTUAL="$(python3 - "$f" "$START" "$END" <<'PY'
import sys

path, start, end = sys.argv[1:4]
with open(path, encoding="utf-8") as f:
    text = f.read()
sys.stdout.write(text[text.index(start) : text.index(end) + len(end)])
PY
)"
  if [ "$ACTUAL" != "$RENDERED" ]; then
    echo "signup-args-doc-check.sh: $f signup argument block differs from control::SIGNUP_ARGS (run signup-args-doc-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
  fi
done

if [ "$MODE" = "write" ]; then
  [ "$FAILURES" -eq 0 ] || exit 2
  echo "signup-args-doc-check.sh: wrote the signup argument block to ${#TARGETS[@]} files"
  exit 0
fi
if [ "$FAILURES" -ne 0 ]; then
  exit 1
fi
echo "signup-args-doc-check.sh: signup args in sync in ${#TARGETS[@]} files"
