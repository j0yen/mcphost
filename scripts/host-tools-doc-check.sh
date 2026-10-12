#!/usr/bin/env bash
# host-tools-doc-check.sh -- PRD-mcphost-dry-run-on-write-verbs R5 (AC6):
# docs/host-tools.md carries one table row per write verb -- its kind and
# whether it honours `dry_run`, with the `no_dry_run_reason` beside every
# opt-out -- generated from the WRITE_VERBS table in src/write_verbs.rs
# (parsed back out of the Rust source text, never a second hand-kept list).
# sandbox-api-doc-check.sh calls this in both of its modes.
#
# Usage:
#   scripts/host-tools-doc-check.sh          check: exit 0 or exit 1 naming
#                                             the drift.
#   scripts/host-tools-doc-check.sh --write  regenerate the marked block.
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="src/write_verbs.rs"
TARGET="docs/host-tools.md"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

if [ ! -f "$SOURCE" ]; then
  echo "host-tools-doc-check.sh: missing $SOURCE" >&2
  exit 2
fi

render_block() {
  python3 - "$SOURCE" <<'PY'
import re
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    text = f.read()
m = re.search(r"pub const WRITE_VERBS: &\[WriteVerb\] = &\[(.*?)\n\];", text, re.S)
if not m:
    print("host-tools-doc-check.sh: could not find WRITE_VERBS in " + sys.argv[1], file=sys.stderr)
    sys.exit(2)
rows = re.findall(r'^\s*(sv|nd)\("([^"]+)", Kind::(\w+)(?:, "([^"]+)")?\),', m.group(1), re.M)
if not rows:
    print("host-tools-doc-check.sh: WRITE_VERBS has no rows", file=sys.stderr)
    sys.exit(2)
print("<!-- mcphost-write-verbs:start -->")
print("| verb | kind | dry_run | no_dry_run_reason |")
print("|---|---|---|---|")
for tag, name, kind, reason in rows:
    if tag == "sv":
        print(f"| `{name}` | {kind.lower()} | supported | |")
    else:
        print(f"| `{name}` | {kind.lower()} | refused | `{reason}` |")
# PRD-mcphost-destructive-verb-confirmation-token R4: the confirm rule, once per
# gated verb (a `sv` row of kind delete/drop/revoke/remove -- the same predicate
# as confirm::gated), the sentence parsed out of this same source file.
sentence = re.search(r'pub const CONFIRM_SENTENCE: &str = "([^"]+)";', text)
if not sentence:
    print("host-tools-doc-check.sh: could not find CONFIRM_SENTENCE in " + sys.argv[1], file=sys.stderr)
    sys.exit(2)
print()
print("### Confirm rule")
print()
for tag, name, kind, reason in rows:
    if tag == "sv" and kind in ("Delete", "Drop", "Revoke", "Remove"):
        print(f"- `{name}`: {sentence.group(1)}")
print("<!-- mcphost-write-verbs:end -->")
PY
}

RENDERED="$(render_block)" || exit $?

if [ "$MODE" = "write" ]; then
  {
    echo "# Write verbs and \`dry_run\`"
    echo
    echo "Every host verb that mutates tenant data, from the \`WRITE_VERBS\` table in"
    echo "\`src/write_verbs.rs\`. A \`supported\` verb called with \`dry_run: true\` runs inside a"
    echo "rolled-back savepoint and answers \`{would: {...}, _meta: {dry_run: true}}\`; a \`refused\`"
    echo "verb fails with \`args_invalid\` and \`data.no_dry_run_reason\` set to the reason beside it."
    echo "Regenerate with \`scripts/sandbox-api-doc-check.sh --write\`."
    echo
    printf '%s\n' "$RENDERED"
  } > "$TARGET"
  echo "host-tools-doc-check.sh: wrote $(printf '%s\n' "$RENDERED" | grep -c '^| `') verbs to $TARGET"
  exit 0
fi

if [ ! -f "$TARGET" ]; then
  echo "host-tools-doc-check.sh: $TARGET does not exist" >&2
  exit 1
fi
ACTUAL="$(sed -n '/<!-- mcphost-write-verbs:start -->/,/<!-- mcphost-write-verbs:end -->/p' "$TARGET")"
if [ "$ACTUAL" != "$RENDERED" ]; then
  echo "host-tools-doc-check.sh: $TARGET is out of sync with $SOURCE (run scripts/sandbox-api-doc-check.sh --write)" >&2
  diff <(printf '%s\n' "$RENDERED") <(printf '%s\n' "$ACTUAL") >&2 || true
  exit 1
fi
echo "write_verbs=$(printf '%s\n' "$RENDERED" | grep -c '^| `') host_tools_doc_in_sync=1"
