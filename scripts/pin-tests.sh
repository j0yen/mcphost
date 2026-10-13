#!/usr/bin/env bash
# pin-tests.sh -- run the hand-kept PIN assertions in seconds.
#
# 2026-10-13: runs 764/765 (and earlier outsch_ac01) went red on GitHub CI after
# a GREEN wm-build gate, each on a hand-maintained pin assertion that only the
# full CI suite ran. Each cost a ~40 min CI round trip plus a hand redrive.
# This runs just those tests as a wm-build host `[[check]]` (wm-build-ci.toml),
# where the cargo target is already warm from the inner's own test builds.
#
# The pin list is ONE list below: `<test binary>:<filter>` pairs. A filter is a
# cargo test name substring; it must select only the pin tests (<= 3 tests, none
# whose name mentions sandbox/bwrap), so this stays fast and needs no userns.
#
# Respects an existing CARGO_TARGET_DIR; never sets one.
#
# Usage:
#   pin-tests.sh          # run every pin, exit 1 if any failed
#   pin-tests.sh --list   # print the pairs and exit 0
set -euo pipefail

PINS=(
  "suite_core_08:at_most_ten_suites_and_none_drifted"
  "suite_sandbox_02:lanecov_ac04"
  "suite_core_07:outsch_ac01"
  "suite_core_08:toolname_ac08_tools_list_counts_updated"
  "suite_core_07:compat_ac04_ac05_admin_and_tenant_cache_fields"
  "suite_core_07:lanecov_ac01"
)

cd "$(dirname "$0")/.."

if [ "${1:-}" = "--list" ]; then
  printf '%s\n' "${PINS[@]}"
  exit 0
fi

start=$SECONDS
failed=0
for pin in "${PINS[@]}"; do
  bin=${pin%%:*}
  filter=${pin#*:}
  t0=$SECONDS
  if out=$(cargo test -q --test "$bin" "$filter" -- --test-threads=1 2>&1); then
    status=pass
  else
    status=FAIL
    failed=1
  fi
  echo "pin-tests: $status $bin:$filter ($((SECONDS - t0))s)"
  if [ "$status" = FAIL ]; then
    printf '%s\n' "$out" | tail -40
  fi
done

total=$((SECONDS - start))
if [ "$failed" -ne 0 ]; then
  echo "pin-tests: FAILED (${#PINS[@]} pins, ${total}s)"
  exit 1
fi
echo "pin-tests: ok (${#PINS[@]} pins, ${total}s)"
