#!/usr/bin/env bash
# ci-shard-report.sh -- a shard job's wall-time readout, budget and failure
# naming (PRD-mcphost-ci-sandbox-shards-balanced AC3 for the sandbox shards,
# PRD-mcphost-ci-core-suites-sharded for the core shards). ci.yml's shard steps
# time `cargo test --no-run` (build wall) and the test run (run wall)
# separately and hand the numbers here; only the RUN wall is budgeted, because
# compile time is runner/cache noise the tests cannot control.
#
#   ci-shard-report.sh [sandbox|core] <n> <of> <build_s> <run_s> <cargo-test-log> [<test-status>]
#
# The partition defaults to `sandbox` (the original call shape). Prints (and
# appends to $GITHUB_STEP_SUMMARY when set):
#   <partition> shard N/M build wall: <s>s
#   <partition> shard N/M run wall: <s>s
# warns at run wall >= WARN_AT, fails above FAIL_ABOVE (sandbox 240/300 s, core
# 420/540 s), and prints one line per suite:
#   suite=<name> measured=<s>s recorded=<s>s
# on warn or fail for the sandbox partition and ALWAYS for core (the core
# readout is the evidence the weights are refreshed from).
# measured = the suite's `finished in` seconds from the log, recorded = its
# weight in scripts/ci-suite-weights.json (CI_SUITE_WEIGHTS_FILE overrides).
#
# A non-zero test status also names every failed test, so `gh run view
# --log-failed` shows the test and its shard instead of a log cut mid-suite:
#   <partition> shard N/M FAILED <test>
# (names come from libtest's `failures:` list and `---- <test> stdout ----`
# headers; none parsed -> one `<partition> shard N/M ERROR ...` line).
# Exit: the tests' own status when non-zero, else 1 iff run wall > FAIL_ABOVE.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEIGHTS_FILE="${CI_SUITE_WEIGHTS_FILE:-$REPO_ROOT/scripts/ci-suite-weights.json}"

partition=sandbox
case "${1:-}" in
  sandbox|core) partition="$1"; shift ;;
esac

case "$partition" in
  core)    WARN_AT=420; FAIL_ABOVE=540 ;;
  *)       WARN_AT=240; FAIL_ABOVE=300 ;;
esac

n="${1:?usage: ci-shard-report.sh [sandbox|core] <n> <of> <build_s> <run_s> <log> [<status>]}"
of="${2:?}" build_s="${3:?}" run_s="${4:?}" log="${5:?}" status="${6:-0}"
label="$partition shard $n/$of"
if [ "$partition" = core ]; then widen="CORE_SHARDS"; else widen="SANDBOX_SHARDS"; fi

say() {
  echo "$*"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "$*" >> "$GITHUB_STEP_SUMMARY"; fi
}

# The log is cargo's colored output (CARGO_TERM_COLOR=always in CI): strip the
# escapes before any line match.
plain_log() { sed -E $'s/\x1b\\[[0-9;]*m//g' "$log"; }

say "$label build wall: ${build_s}s"
say "$label run wall: ${run_s}s"

level=ok
if [ "$run_s" -gt "$FAIL_ABOVE" ]; then
  level=fail
elif [ "$run_s" -ge "$WARN_AT" ]; then
  level=warn
fi

if [ "$level" = fail ]; then
  echo "::error::$label run wall ${run_s}s exceeds the ${FAIL_ABOVE}s budget -- refresh scripts/ci-suite-weights.json from the lines below and widen $widen in scripts/ci-test-partition.sh if one suite alone is too heavy"
elif [ "$level" = warn ]; then
  echo "::warning::$label run wall ${run_s}s is at or above ${WARN_AT}s (fails above ${FAIL_ABOVE}s) -- compare measured and recorded below"
fi

if [ "$level" != ok ] || [ "$partition" = core ]; then
  # Suite name for each `Running` line, then the `finished in` that follows it.
  # `Running unittests src/lib.rs` is the `lib` weight; any other unit-test
  # binary (core's `--bins`) is named by its source path.
  plain_log | awk '
    /^[[:space:]]*Running unittests / {
      name = $3
      if (name ~ /(^|\/)src\/lib\.rs$/) name = "lib"; else name = "bin:" name
      next
    }
    /^[[:space:]]*Running tests\// {
      name = $2; sub(/^tests\//, "", name); sub(/\.rs$/, "", name); next
    }
    /^test result:/ && name != "" {
      if (match($0, /finished in [0-9.]+s/)) {
        t = substr($0, RSTART + 12, RLENGTH - 13)
        printf "%s %s\n", name, t
      }
      name = ""
    }' | while read -r name measured; do
    recorded="$(jq -r --arg k "$name" '.[$k] // "unrecorded"' "$WEIGHTS_FILE" 2>/dev/null || echo unrecorded)"
    say "suite=$name measured=${measured}s recorded=${recorded}s"
  done
fi

if [ "$status" -ne 0 ]; then
  failed="$(plain_log | awk '
    /^---- .* stdout ----$/ {
      t = $0; sub(/^---- /, "", t); sub(/ stdout ----$/, "", t)
      if (!(t in seen)) { seen[t] = 1; print t }
      next
    }
    /^failures:$/ { inlist = 1; next }
    inlist && /^    [^[:space:]]+$/ {
      t = $1
      if (!(t in seen)) { seen[t] = 1; print t }
      next
    }
    inlist { inlist = 0 }')"
  if [ -n "$failed" ]; then
    while read -r t; do
      [ -n "$t" ] || continue
      say "$label FAILED $t"
    done <<<"$failed"
  else
    say "$label ERROR no failed test parsed from the log (test status $status; a compile error or a killed run leaves none)"
  fi
  exit "$status"
fi
[ "$level" = fail ] && exit 1
exit 0
