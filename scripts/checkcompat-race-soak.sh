#!/usr/bin/env bash
# checkcompat-race-soak.sh -- PRD-mcphost-checkcompat-port-race AC5.
#
# Runs `cargo test checkcompat -- --test-threads=32` (the two
# tests/checkcompat_ac02_ac03.rs tests plus every tests/checkcompat_race_
# ac*.rs test, including the 30 dummy servers in ac05) in a loop, forcing
# 32 test threads regardless of the host's real core count (the PRD's own
# technical considerations: the race scaled with thread count, not core
# count, so RedBaron must be pushed to the same concurrency a 32-core box
# would produce on its own). Exits non-zero on the first failing iteration,
# printing which one.
#
# Usage: scripts/checkcompat-race-soak.sh [iterations]   (default 200)
set -euo pipefail

ITERATIONS="${1:-200}"

for i in $(seq 1 "$ITERATIONS"); do
  if ! cargo test checkcompat -- --test-threads=32 >/tmp/checkcompat-race-soak-"$i".log 2>&1; then
    echo "checkcompat-race-soak: FAILED on iteration $i/$ITERATIONS -- see /tmp/checkcompat-race-soak-$i.log" >&2
    tail -n 40 /tmp/checkcompat-race-soak-"$i".log >&2
    exit 1
  fi
  rm -f /tmp/checkcompat-race-soak-"$i".log
done

echo "checkcompat-race-soak: ok ($ITERATIONS/$ITERATIONS iterations, 0 failures)"
