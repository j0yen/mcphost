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
# 720/900 s), and ALWAYS (every partition, every level) prints one line per suite:
#   suite=<name> measured=<s>s recorded=<s>s
# plus one `::notice title=... readout` annotation carrying them all. The readout is
# the evidence scripts/ci-refresh-weights.sh refreshes the weights from, so a suite
# that ran under its warn line must still report: a sandbox shard under 240 s used to
# print nothing, and suite_sandbox_02 stayed recorded at 152 s while it ran ~300 s.
# measured = the suite's `finished in` seconds from the log, recorded = its
# weight in scripts/ci-suite-weights.json (CI_SUITE_WEIGHTS_FILE overrides).
#
# A non-zero test status also names every failed test, so `gh run view
# --log-failed` shows the test and its shard instead of a log cut mid-suite:
#   <partition> shard N/M FAILED <test>
# (emitted as a `::error::` annotation too, which survives a cut job log)
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

# WARN_AT / FAIL_ABOVE are NOT restated here: ci-test-partition.sh `limits` is the one
# definition (its `check` applies the same two lines to the predicted heaviest core
# shard). 10-17 slow hosted runners measured suite_core_08 559 s and suite_core_07
# 438 s against the then-recorded 375/341 s; the core weights are the max measured
# seconds since 10-18.
read -r WARN_AT FAIL_ABOVE < <("$REPO_ROOT/scripts/ci-test-partition.sh" limits "$partition") \
  || { echo "ci-shard-report: cannot read the $partition limits from ci-test-partition.sh" >&2; exit 2; }

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

# Suite name for each `Running` line, then the `finished in` that follows it.
# `Running unittests src/lib.rs` is the `lib` weight; any other unit-test
# binary (core's `--bins`) is named by its source path.
suite_lines="$(plain_log | awk '
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
  unit=s; [ "$recorded" = unrecorded ] && unit=""
  echo "suite=$name measured=${measured}s recorded=${recorded}${unit}"
done)"
if [ -n "$suite_lines" ]; then
  while IFS= read -r line; do say "$line"; done <<<"$suite_lines"
fi
# One annotation carries the whole readout: the step summary and the job
# log are not always retrievable (a hosted log can be cut mid-suite), an
# annotation is, so the weights can still be refreshed from a red run.
echo "::notice title=$label readout::build ${build_s}s; run ${run_s}s; $(printf '%s' "$suite_lines" | tr '\n' ';')"

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
    # Printed as `::error::` annotations as well as step-summary lines: the
    # hosted job log is sometimes cut mid-suite (the original defect), and the
    # annotation survives that, readable from the check run's annotations.
    while read -r t; do
      [ -n "$t" ] || continue
      echo "::error::$label FAILED $t"
      if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "$label FAILED $t" >> "$GITHUB_STEP_SUMMARY"; fi
    done <<<"$failed"
  else
    echo "::error::$label ERROR no failed test parsed from the log (test status $status; a compile error or a killed run leaves none)"
  fi
  exit "$status"
fi
[ "$level" = fail ] && exit 1
exit 0
