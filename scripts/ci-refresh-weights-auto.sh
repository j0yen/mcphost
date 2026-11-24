#!/usr/bin/env bash
# ci-refresh-weights-auto.sh -- the CI-side wrapper of the weights refresh (.github/workflows/refresh-weights.yml).
#
# Runs, in order, ci-refresh-weights.sh <window run ids>, gen-test-suites.sh and `ci-test-partition.sh check`;
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
#   - changed=0: no `_provisional` pending entry and the weights are already what the window measured.
#
# Window: the source run plus every green push run of the same workflow on the same branch created from the
# PREVIOUS refresh's source run (the newest `ci: refresh suite weights from run <id>` subject on HEAD's first-parent
# history) through the source run; ci-refresh-weights.sh records each suite at its MAX over them. One run is one
# sample of a suite that swings: suite_core_12 (tlint_ac06's nested `cargo test` retries its strict run up to three
# times) measured 600.56 s on 44757115005 and 326.72 s on 45034667949; the refresh of the second alone recorded 327,
# packed it with suite_core_11, and main run 45038989292 ran that core shard 909 s against 900. The window also
# reads the refresh commit's own run, which skip=self never measures. No refresh subject in history: the source alone.
# The run list (gh api) failing fails the script (fail-closed, no result line). Needs fetch-depth 0.
#
# Usage:
#   ci-refresh-weights-auto.sh <run-id>
#   ci-refresh-weights-auto.sh --selftest-predicate "<commit subject>"   # prints self|none
# Env: CI_REFRESH_REPO (default j0yen/mcphost), the repository whose runs the window lists.
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

# The window (see the header): the source run, plus the green runs since the previous refresh's source.
REPO="${CI_REFRESH_REPO:-j0yen/mcphost}"
runs="$RUN_ID"
prev_subject="$(git log --first-parent -n 1 -E --grep="^${SELF_PREFIX} from run [0-9]+" --format=%s HEAD)" \
  || { echo "ci-refresh-weights-auto: cannot read HEAD's history" >&2; exit 1; }
prev=""
if [[ "$prev_subject" =~ ^"$SELF_PREFIX from run "([0-9]+) ]]; then prev="${BASH_REMATCH[1]}"; fi
if [ -n "$prev" ] && [ "$prev" != "$RUN_ID" ]; then
  src_json="$(gh api "repos/$REPO/actions/runs/$RUN_ID")" && prev_json="$(gh api "repos/$REPO/actions/runs/$prev")" \
    || { echo "ci-refresh-weights-auto: cannot read runs $RUN_ID / $prev" >&2; exit 1; }
  read -r wf branch src_at < <(jq -r '"\(.workflow_id) \(.head_branch) \(.created_at)"' <<<"$src_json") \
    && prev_at="$(jq -r '.created_at' <<<"$prev_json")" \
    && [[ "$wf" =~ ^[0-9]+$ ]] && [ -n "$branch" ] && [ "$branch" != null ] && [ "$src_at" != null ] && [ "$prev_at" != null ] \
    || { echo "ci-refresh-weights-auto: run $RUN_ID / $prev lacks workflow_id, head_branch or created_at" >&2; exit 1; }
  listed="$(gh api --paginate "repos/$REPO/actions/workflows/$wf/runs?branch=$branch&event=push&status=success&created=$prev_at..$src_at&per_page=100")" \
    || { echo "ci-refresh-weights-auto: cannot list the green $branch runs of workflow $wf from $prev_at to $src_at" >&2; exit 1; }
  # the query's filters are re-applied here: only what they name is ever measured
  ids="$(jq -r --arg b "$branch" --arg lo "$prev_at" --arg hi "$src_at" \
           '.workflow_runs[] | select(.conclusion == "success" and .event == "push" and .head_branch == $b
                                     and .created_at >= $lo and .created_at <= $hi) | .id' <<<"$listed")" \
    || { echo "ci-refresh-weights-auto: cannot read the run list" >&2; exit 1; }
  runs="$(printf '%s\n%s\n' "$RUN_ID" "$ids" | grep -E '^[0-9]+$' | sort -n -u | tr '\n' ' ')"
  echo "ci-refresh-weights-auto: window = runs ${runs}(green $branch push runs from $prev, the previous refresh's source, through $RUN_ID)" >&2
else
  echo "ci-refresh-weights-auto: no \`${SELF_PREFIX} from run <id>\` subject on HEAD's history; window = run $RUN_ID" >&2
fi

# Inner output goes to stderr so stdout carries exactly the one result line.
# shellcheck disable=SC2086 # $runs is a space-separated list of numeric run ids
"$BIN_DIR/ci-refresh-weights.sh" $runs >&2 || { echo "ci-refresh-weights-auto: ci-refresh-weights.sh failed" >&2; exit 1; }
"$BIN_DIR/gen-test-suites.sh" >&2 || { echo "ci-refresh-weights-auto: gen-test-suites.sh failed" >&2; exit 1; }
"$BIN_DIR/ci-test-partition.sh" check >&2 || { echo "ci-refresh-weights-auto: ci-test-partition.sh check failed" >&2; exit 1; }

status="$(git status --porcelain -- scripts/ci-suite-weights.json scripts/ci-suite-measured.json tests/ Cargo.toml)" \
  || { echo "ci-refresh-weights-auto: git status failed" >&2; exit 1; }
if [ -n "$status" ]; then
  echo "changed=1 skip=none"
else
  echo "changed=0 skip=none"
fi
