#!/usr/bin/env bash
# skill-gen-check.sh -- PRD-mcphost-plugin-skill-generated-from-registry R6:
# the marked sections of plugin/skills/mcphost/SKILL.md (verbs, limits, errors,
# local loop) and www/skill.md must equal what `mcphost skill-gen` renders
# from the host's own registries, the same shape as signup-args-doc-check.sh.
#
# Usage:
#   scripts/skill-gen-check.sh          check: exit 0, or exit 1 naming the first
#                                        stale line of each file.
#   scripts/skill-gen-check.sh --write  regenerate both files.
# generated-docs-check.sh runs this in both modes, so the CI regen-check job
# covers it; tests/skgn_ac03_*.rs is the same comparison inside `cargo test`.
set -uo pipefail
cd "$(dirname "$0")/.."

# Same lookup as signup-args-doc-check.sh's mcphost_bin.
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

if [ "${1:-}" = "--write" ]; then
  mcphost_bin skill-gen --write
  exit $?
fi
if mcphost_bin skill-gen >/dev/null; then
  echo "skill-gen-check.sh: plugin skill and www/skill.md in sync"
  exit 0
fi
echo "skill-gen-check.sh: the skill is stale (run scripts/skill-gen-check.sh --write)" >&2
exit 1
