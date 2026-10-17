#!/usr/bin/env bash
# skill-gen-check.sh -- PRD-mcphost-plugin-skill-generated-from-tools-list
# requirement 6: the marked sections of plugin/skills/mcphost/SKILL.md and
# www/skill.md are `mcphost skill-gen` output, byte for byte. Runs beside
# sandbox-api-doc-check.sh in CI.
#
# Usage:
#   scripts/skill-gen-check.sh          check: exit 0, or exit 1 with
#                                        `skill-gen: <file>:<line> differs ...`.
#   scripts/skill-gen-check.sh --write  regenerate both files.
#
# Binary: $MCPHOST_BIN, else $CARGO_BIN_EXE_mcphost (set under `cargo test`),
# else a prebuilt target/{debug,release}/mcphost newer than src/, else
# `cargo run`.
set -uo pipefail
cd "$(dirname "$0")/.."

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

# stdout is the rendered plugin file; only the verdict matters here.
mcphost_bin skill-gen >/dev/null
rc=$?
if [ "$rc" -eq 0 ]; then
  echo "skill-gen-check.sh: plugin/skills/mcphost/SKILL.md and www/skill.md are in sync"
fi
exit "$rc"
