#!/usr/bin/env bash
# kind-contract-doc-check.sh -- PRD-mcphost-kind-contract-conformance-generated-b
# R5 (AC1): the `## Contract` block of every docs/kinds/<kind>.md is rendered
# from the kind's KindContract (`mcphost gen-docs --kinds-only`); this diffs
# rendered against committed and exits 1, naming the kind and the row, on
# drift.
#
# Usage:
#   scripts/kind-contract-doc-check.sh [repo-root]   check (default: this repo)
#   scripts/kind-contract-doc-check.sh --write [repo-root]   regenerate the blocks
# MCPHOST_BIN overrides the binary (default: cargo run).
set -uo pipefail
if [ "${1:-}" = "--write" ]; then
  MODE_ARGS=(gen-docs --kinds-only)
  cd "${2:-$(dirname "$0")/..}"
else
  MODE_ARGS=(gen-docs --kinds-only --check)
  cd "${1:-$(dirname "$0")/..}"
fi
if [ -n "${MCPHOST_BIN:-}" ]; then
  "$MCPHOST_BIN" "${MODE_ARGS[@]}"
else
  "${CARGO:-$HOME/.cargo/bin/cargo}" run -q --bin mcphost -- "${MODE_ARGS[@]}"
fi
