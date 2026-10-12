#!/usr/bin/env bash
# ci-shard-report.sh -- the sandbox shard job's wall-time readout and budget
# (PRD-mcphost-ci-sandbox-shards-balanced AC3). ci.yml's sandbox step times
# `cargo test --no-run` (build wall) and the test run (run wall) separately
# and hands the numbers here; only the RUN wall is budgeted, because compile
# time is runner/cache noise the tests cannot control.
#
#   ci-shard-report.sh <n> <of> <build_s> <run_s> <cargo-test-log> [<test-status>]
#
# Prints (and appends to $GITHUB_STEP_SUMMARY when set):
#   sandbox shard N/M build wall: <s>s
#   sandbox shard N/M run wall: <s>s
# warns at run wall >= WARN_AT (240 s), fails above FAIL_ABOVE (300 s), and on
# warn or fail prints one line per suite:
#   suite=<name> measured=<s>s recorded=<s>s
# measured = the suite's `finished in` seconds from the log, recorded = its
# weight in scripts/ci-suite-weights.json (CI_SUITE_WEIGHTS_FILE overrides).
# Exit: the tests' own status when non-zero, else 1 iff run wall > FAIL_ABOVE.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEIGHTS_FILE="${CI_SUITE_WEIGHTS_FILE:-$REPO_ROOT/scripts/ci-suite-weights.json}"
WARN_AT=240
FAIL_ABOVE=300

n="${1:?usage: ci-shard-report.sh <n> <of> <build_s> <run_s> <log> [<status>]}"
of="${2:?}" build_s="${3:?}" run_s="${4:?}" log="${5:?}" status="${6:-0}"

say() {
  echo "$*"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "$*" >> "$GITHUB_STEP_SUMMARY"; fi
}

say "sandbox shard $n/$of build wall: ${build_s}s"
say "sandbox shard $n/$of run wall: ${run_s}s"

level=ok
if [ "$run_s" -gt "$FAIL_ABOVE" ]; then
  level=fail
elif [ "$run_s" -ge "$WARN_AT" ]; then
  level=warn
fi

if [ "$level" != ok ]; then
  if [ "$level" = fail ]; then
    echo "::error::sandbox shard $n/$of run wall ${run_s}s exceeds the ${FAIL_ABOVE}s budget -- refresh scripts/ci-suite-weights.json from the lines below and widen SANDBOX_SHARDS in scripts/ci-test-partition.sh if one suite alone is too heavy"
  else
    echo "::warning::sandbox shard $n/$of run wall ${run_s}s is at or above ${WARN_AT}s (fails above ${FAIL_ABOVE}s) -- compare measured and recorded below"
  fi
  # Suite name for each `Running` line, then the `finished in` that follows it.
  awk '
    /^[[:space:]]*Running unittests / { name = "lib"; next }
    /^[[:space:]]*Running tests\// {
      name = $2; sub(/^tests\//, "", name); sub(/\.rs$/, "", name); next
    }
    /^test result:/ && name != "" {
      if (match($0, /finished in [0-9.]+s/)) {
        t = substr($0, RSTART + 12, RLENGTH - 13)
        printf "%s %s\n", name, t
      }
      name = ""
    }' "$log" | while read -r name measured; do
    recorded="$(jq -r --arg k "$name" '.[$k] // "unrecorded"' "$WEIGHTS_FILE" 2>/dev/null || echo unrecorded)"
    say "suite=$name measured=${measured}s recorded=${recorded}s"
  done
fi

if [ "$status" -ne 0 ]; then exit "$status"; fi
[ "$level" = fail ] && exit 1
exit 0
