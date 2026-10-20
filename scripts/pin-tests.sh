#!/usr/bin/env bash
# pin-tests.sh -- run the contract test suites in minutes, before CI.
#
# 2026-10-13: runs 764/765 (and earlier outsch_ac01) went red on GitHub CI after
# a GREEN wm-build gate, each on a hand-maintained pin assertion that only the
# full CI suite ran. 2026-10-15: runs 824/247/250 again, on a seventh contract
# test nobody had pinned. PRD-mcphost-contract-suite-generated replaces the
# hand array with a derived list: a test file that asserts a set the tree
# derives from itself declares `//! contract: <what it asserts>`, and
# scripts/gen-test-suites.sh routes it into tests/suite_contracts_<core|sandbox>_NN.rs.
# The run list below is read from Cargo.toml's generated [[test]] entries, so the
# inner (wm-build-ci.toml `[[check]] contract-suite`), the gate and CI's
# `contract suites` step all run the same set from one source.
#
# Each contract binary runs whole. The sandbox binaries run --test-threads=1
# (the server-spawning tests stay hermetic); the core binaries spawn no server,
# only drift-check scripts over the tree, so they run in parallel (serial they
# alone took ~150 s of the 300 s budget). Respects an existing CARGO_TARGET_DIR; never sets one.
#
# Usage:
#   pin-tests.sh          # run every contract suite, exit 1 if any failed
#   pin-tests.sh --list   # print the contract suite binary names and exit 0
#   pin-tests.sh --cargo-free  # only the cargo-free drift checks (gate form)
set -euo pipefail

cd "$(dirname "$0")/.."

# The one source: the generated [[test]] names in Cargo.toml.
contract_suites() {
  grep -oE '^name = "suite_contracts_[a-z]+_[0-9]+"' Cargo.toml | cut -d'"' -f2 | LC_ALL=C sort
}

if [ "${1:-}" = "--list" ]; then
  contract_suites
  exit 0
fi

if [ "${1:-}" = "--cargo-free" ]; then
  start=$SECONDS
  failed=0
  for chk in "gen-test-suites --check:scripts/gen-test-suites.sh --check" \
             "ci-test-partition check:scripts/ci-test-partition.sh check" \
             "audit-marker-check:scripts/audit-marker-check.sh"; do
    name=${chk%%:*}
    cmd=${chk#*:}
    t0=$SECONDS
    # shellcheck disable=SC2086
    if out=$(bash $cmd 2>&1); then
      status=pass
    else
      status=FAIL
      failed=1
    fi
    echo "pin-tests: $status $name ($((SECONDS - t0))s)"
    if [ "$status" = FAIL ]; then
      printf '%s\n' "$out" | tail -40
    fi
  done
  if [ "$failed" -ne 0 ]; then
    echo "pin-tests: FAILED (3 checks, $((SECONDS - start))s)"
    exit 1
  fi
  echo "pin-tests: ok (3 checks, $((SECONDS - start))s)"
  exit 0
fi

mapfile -t SUITES < <(contract_suites)
if [ "${#SUITES[@]}" -eq 0 ]; then
  echo "pin-tests: FAILED no suite_contracts_* [[test]] entries in Cargo.toml (run scripts/gen-test-suites.sh)"
  exit 1
fi

start=$SECONDS
failed=0
for bin in "${SUITES[@]}"; do
  t0=$SECONDS
  threads=1
  case "$bin" in suite_contracts_core_*) threads=8 ;; esac
  if out=$(${CARGO:-cargo} test -q --test "$bin" -- --test-threads="$threads" 2>&1); then
    status=pass
  else
    status=FAIL
    failed=1
  fi
  echo "pin-tests: $status $bin ($((SECONDS - t0))s)"
  if [ "$status" = FAIL ]; then
    printf '%s\n' "$out" | tail -40
  fi
done

total=$((SECONDS - start))
if [ "$failed" -ne 0 ]; then
  echo "pin-tests: FAILED (${#SUITES[@]} suites, ${total}s)"
  exit 1
fi
echo "pin-tests: ok (${#SUITES[@]} suites, ${total}s)"
