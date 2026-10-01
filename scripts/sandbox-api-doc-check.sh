#!/usr/bin/env bash
# sandbox-api-doc-check.sh — PRD-mcphost-sandbox-bridge-discoverability
# requirement 7 (AC7). Diffs each doc's own `mcphost.<module>` mentions
# against the one source of truth for the sandbox's `import mcphost` API --
# the `sys.modules["mcphost.<module>"]` registration block in
# src/kinds/python.rs's PY_RUNNER_SCRIPT -- so a bridge module rename or
# removal can never again silently orphan the docs the way it did on
# 2026-09-30 (this PRD's own Grounding). A doc merely has to mention
# `mcphost.<module>` somewhere; this is a drift tripwire, not a prose
# linter (docs-check.sh already covers "is this file broken").
#
# Usage: scripts/sandbox-api-doc-check.sh [repo-root]
# repo-root defaults to this script's own parent directory (the real
# checkout); a test harness can pass a fixture tree instead (AC7's "given
# a fixture doc with one module removed").
# Exit 0, stdout "modules=<n> docs_in_sync=<k>" when every doc names every
# registered module. Exit 1, naming the first missing module and file on
# stderr, otherwise.
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

RUNNER_SRC="src/kinds/python.rs"
DOCS=(
  "docs/kinds/python.md"
  "www/llms.txt"
  "plugin/skills/mcphost/SKILL.md"
)

modules=$(grep -oE 'sys\.modules\["mcphost\.[a-z_]+"\]' "$RUNNER_SRC" \
  | sed -E 's/sys\.modules\["mcphost\.([a-z_]+)"\]/\1/' \
  | sort -u)

if [ -z "$modules" ]; then
  echo "sandbox-api-doc-check: found no mcphost.* module registrations in $RUNNER_SRC" >&2
  exit 1
fi
module_count=$(echo "$modules" | wc -l | tr -d ' ')

docs_in_sync=0
for doc in "${DOCS[@]}"; do
  if [ ! -f "$doc" ]; then
    echo "sandbox-api-doc-check: $doc does not exist" >&2
    exit 1
  fi
  for m in $modules; do
    if ! grep -q "mcphost\.$m" "$doc"; then
      echo "sandbox-api-doc-check: mcphost.$m missing from $doc" >&2
      exit 1
    fi
  done
  docs_in_sync=$((docs_in_sync + 1))
done

echo "modules=$module_count docs_in_sync=$docs_in_sync"
