#!/usr/bin/env bash
# ci-refresh-weights-auto.sh -- the CI-side wrapper of the weights refresh (.github/workflows/refresh-weights.yml).
#
# Runs, in order, ci-refresh-weights.sh <run-id>, gen-test-suites.sh and `ci-test-partition.sh check`;
# ANY non-zero exit stops the script with that failure (fail-closed, nothing is swallowed). Then prints
# exactly one line on stdout:
#     changed=0|1 skip=<none|self>
# `changed=1` means `git status --porcelain` shows a change under scripts/ci-suite-weights.json,
# scripts/ci-suite-measured.json, tests/ or Cargo.toml. The script touches no git remote; publishing
# (branch, PR, auto-merge) is the workflow's job.
#
# Loop guard (the diff is the primary guard, the subject check is secondary and does not depend on the actor):
#   - skip=self: HEAD's commit subject starts with "ci: refresh suite weights" (the merged refresh PR itself);
#     nothing is run, `changed=0 skip=self`.
#   - changed=0: no `_provisional` pending entry and the weights are already what the run measured.
#
# Usage:
#   ci-refresh-weights-auto.sh <run-id>
#   ci-refresh-weights-auto.sh --selftest-predicate "<commit subject>"   # prints self|none
# Env (tests only):
#   CI_REFRESH_AUTO_BIN_DIR  directory holding ci-refresh-weights.sh, gen-test-suites.sh and
#                            ci-test-partition.sh (default: this script's directory)
#   CI_REFRESH_AUTO_ROOT     the checkout to run in (default: this script's repo root)
set -uo pipefail

SELF_PREFIX="ci: refresh suite weights"

# is_self_subject <subject>: success when the subject is the refresh PR's own commit.
is_self_subject() {
  case "$1" in
    "$SELF_PREFIX"*) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "${1:-}" = "--selftest-predicate" ]; then
  [ "$#" -eq 2 ] || { echo "usage: $0 --selftest-predicate <subject>" >&2; exit 2; }
  if is_self_subject "$2"; then echo self; else echo none; fi
  exit 0
fi

[ "$#" -eq 1 ] && [[ "$1" =~ ^[0-9]+$ ]] \
  || { echo "usage: $0 <numeric CI run id>" >&2; exit 2; }
RUN_ID="$1"

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${CI_REFRESH_AUTO_BIN_DIR:-$SELF_DIR}"
ROOT="${CI_REFRESH_AUTO_ROOT:-$(cd "$SELF_DIR/.." && pwd)}"
cd "$ROOT" || exit 2

subject="$(git log -1 --format=%s HEAD)" || { echo "ci-refresh-weights-auto: cannot read HEAD's subject" >&2; exit 1; }
if is_self_subject "$subject"; then
  echo "changed=0 skip=self"
  exit 0
fi

# Inner output goes to stderr so stdout carries exactly the one result line.
"$BIN_DIR/ci-refresh-weights.sh" "$RUN_ID" >&2 || { echo "ci-refresh-weights-auto: ci-refresh-weights.sh failed" >&2; exit 1; }
"$BIN_DIR/gen-test-suites.sh" >&2 || { echo "ci-refresh-weights-auto: gen-test-suites.sh failed" >&2; exit 1; }
"$BIN_DIR/ci-test-partition.sh" check >&2 || { echo "ci-refresh-weights-auto: ci-test-partition.sh check failed" >&2; exit 1; }

status="$(git status --porcelain -- scripts/ci-suite-weights.json scripts/ci-suite-measured.json tests/ Cargo.toml)" \
  || { echo "ci-refresh-weights-auto: git status failed" >&2; exit 1; }
if [ -n "$status" ]; then
  echo "changed=1 skip=none"
else
  echo "changed=0 skip=none"
fi
