#!/usr/bin/env bash
# contract-lint.sh -- PRD-mcphost-contract-suite-generated.
#
# A test that asserts a set the repository derives from itself (the CHANGELOG
# heading list, the migration set, the recorded suite weights, ...) is a
# CONTRACT test: it must declare `//! contract: <what it asserts>` in its first
# 10 lines so scripts/gen-test-suites.sh routes it into a
# tests/suite_contracts_<core|sandbox>_NN.rs binary that scripts/pin-tests.sh
# (the inner's `contract-suite` check and CI's `contract suites` step) runs
# before the full suite. This lint fails an UNMARKED file whose source reads
# one of those tree-derived sources, naming the file and the line to add.
#
# Usage:
#   scripts/contract-lint.sh [DIR-or-FILE]   defaults to `tests`
# Exit 0 clean, 1 on any unmarked contract-shaped file.
set -uo pipefail

TARGET="${1:-tests}"
if [ -d "$TARGET" ]; then
  mapfile -t FILES < <(find "$TARGET" -type f -name '*.rs' | LC_ALL=C sort)
else
  FILES=("$TARGET")
fi

# <regex>|<what the file reads>
RULES=(
  'include_str!\([^)]*CHANGELOG\.md|read_to_string\([^)]*CHANGELOG\.md|the CHANGELOG.md heading list'
  'read_dir\([^)]*migrations|the migrations/ file set'
  'ci-suite-weights\.json|the recorded scripts/ci-suite-weights.json shard weights'
)

bad=0
for f in "${FILES[@]:-}"; do
  [ -f "$f" ] || continue
  case "$f" in
    */tests/common/*|tests/common/*) continue ;;
  esac
  case "$(basename "$f")" in
    suite_core_*|suite_sandbox_*|suite_env_*|suite_contracts_*) continue ;;
  esac
  head -n 10 "$f" | grep -qE '^//! contract:' && continue
  for rule in "${RULES[@]}"; do
    re=${rule%|*}
    what=${rule##*|}
    if hit=$(grep -nE "$re" "$f" | head -1) && [ -n "$hit" ]; then
      echo "contract-lint: $f:${hit%%:*} reads $what -- add the line \`//! contract: <what it asserts>\` to its first 10 lines"
      bad=1
      break
    fi
  done
done

if [ "$bad" -ne 0 ]; then
  echo "contract-lint: FAILED"
  exit 1
fi
echo "contract-lint: ok"
exit 0
