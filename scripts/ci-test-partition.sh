#!/usr/bin/env bash
# ci-test-partition.sh -- split this crate's cargo test targets into the two
# jobs `.github/workflows/ci.yml` runs in parallel.
#
# PRD-mcphost-ci-sandbox-coverage AC6 (P1): once the sandbox suites actually
# execute in CI instead of skipping, `cargo test --workspace` measured 313-336s
# on the hosted runner -- over the PRD's own 300s budget. AC6 offers two ways
# out; this is the second one ("the suites run as a parallel job").
#
# The partition is DERIVED, never hand-listed. A test target is `sandbox` iff
# its source mentions the sandbox-execution surface (the capability guard or
# `PythonKind`), because those are the targets that spawn a real `bwrap`/
# `unshare` child and therefore need the `sandbox` job's userns grant. Any
# other target is `core`.
#
# Deriving it matters in one direction only, and the asymmetry is deliberate:
#   * over-inclusion (a target that merely NAMES `PythonKind` in a comment
#     lands in the sandbox job) is harmless -- it just runs somewhere that
#     happens to have more capability than it needs;
#   * under-inclusion (a genuinely userns-dependent target left in `core`)
#     would skip silently under `$CI` and re-create the exact false-green this
#     PRD exists to remove -- so BOTH jobs assert zero capability-skips in
#     their logs. A misfiled target turns the core job red, it never passes
#     vacuously.
#
# `check` proves the partition is total and disjoint over `tests/*.rs`, and
# that `Cargo.toml` declares no explicit `[[test]]` target (which would break
# the "every tests/*.rs is exactly one integration target" assumption cargo's
# auto-discovery gives us).
#
# Doctests are NOT in either list: cargo rejects `--doc` combined with any
# other target selector, so the workflow runs `cargo test --doc` as its own
# step in the core job.
#
# Usage:
#   ci-test-partition.sh sandbox   # --lib --test a --test b ...
#   ci-test-partition.sh core      # --bins --test x --test y ...
#   ci-test-partition.sh sandbox-shard <n> <of>   # this shard's slice of the
#                                                  # sandbox partition, same
#                                                  # flag shape as `sandbox`
#   ci-test-partition.sh list <sandbox|core>   # bare target names, one per line
#   ci-test-partition.sh check     # exit 0 iff total, disjoint, and unshadowed
#
# AC6 follow-up: even run as its own job, the sandbox partition alone measured
# 313s (v0.13.3) against the 300s budget -- one job wasn't enough, so the
# sandbox job is further split into SANDBOX_SHARDS matrix jobs, each running
# `sandbox-shard <n> <SANDBOX_SHARDS>`. Targets are assigned to shards by
# `index-in-the-sorted-list mod SANDBOX_SHARDS`, which interleaves the (mostly
# alphabetically-adjacent, individually slow) `python_ac*` targets across
# shards rather than clustering them in one. `--lib` (the sandbox-surface unit
# tests) only ships in shard 1, so it runs once per CI run, not once per shard.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Single source of truth for how many matrix jobs `.github/workflows/ci.yml`
# fans the sandbox partition out to; `check` verifies this many shards are
# still total+disjoint over the sandbox partition.
SANDBOX_SHARDS=3

# The sandbox-execution surface. `supports_user_namespaces` and
# `require_user_namespaces_or_ci_skip` are the capability guard itself;
# `PythonKind`/`kinds::python` is the only caller that spawns sandboxed
# children today (`src/sandbox.rs`'s module doc: "python.rs is this module's
# first, and so far only, caller"). `python_kind_registry` (and its
# `_with_*` variants in tests/common/mod.rs) and the `"kind": "python"` spec
# literal are added 2026-09-07 (CI gate-job failure, run 34102334108:
# `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`): a test
# file that only calls a `tests/common/mod.rs` helper or writes the kind
# name into a JSON literal never mentions `PythonKind`/`kinds::python` in
# its OWN source, so `classify` (which greps only the test file, not the
# shared `common` module it `mod`s in) was under-including exactly the
# targets this split exists to catch -- 3 of them
# (tooltest_ac1_python_two_invocations, tooltest_ac2_python_exception_other_still_runs,
# publishfirsttry_ac03_ac04_quickstart) ran real bwrap children in the `gate`
# job, which has no userns grant, instead of `sandbox`. Under the
# over-inclusion-is-harmless rule above, matching the literal is fine even
# for a target that only asks the server to run *a* python tool without
# calling the registry helper directly (host.quickstart).
SANDBOX_SURFACE='require_user_namespaces_or_ci_skip|supports_user_namespaces|PythonKind|kinds::python|python_kind_registry|"kind":[[:space:]]*"python"'

classify() { # <file> -> prints "sandbox" or "core"
  if grep -qE "$SANDBOX_SURFACE" "$1"; then echo sandbox; else echo core; fi
}

targets() { # <sandbox|core> -> bare target names, one per line, sorted
  local want="$1" f
  for f in "$REPO_ROOT"/tests/*.rs; do
    [ -e "$f" ] || continue
    if [ "$(classify "$f")" = "$want" ]; then basename "$f" .rs; fi
  done | LC_ALL=C sort
}

emit() { # <sandbox|core> -> the cargo target-selection flags for that job
  local want="$1" name
  case "$want" in
    sandbox) printf -- '--lib' ;;   # src/sandbox.rs + src/kinds/python.rs unit
                                    # tests spawn sandboxed children too.
    core)    printf -- '--bins' ;;
  esac
  while read -r name; do
    [ -n "$name" ] || continue
    printf -- ' --test %s' "$name"
  done < <(targets "$want")
  printf '\n'
}

validate_shard_args() { # <idx> <count> -> exit 0 iff both are ints and 1<=idx<=count
  local idx="$1" count="$2"
  case "$idx$count" in
    *[!0-9]*|'')
      echo "ci-test-partition: shard index and count must be positive integers (got '$idx' '$count')" >&2
      return 2
      ;;
  esac
  if [ "$idx" -lt 1 ] || [ "$idx" -gt "$count" ]; then
    echo "ci-test-partition: shard index $idx out of range 1..$count" >&2
    return 2
  fi
}

shard_names() { # <sandbox|core> <index, 1-based> <count> -> bare target names, one per line
  local want="$1" idx="$2" count="$3" i=0 name
  validate_shard_args "$idx" "$count" || return "$?"
  while read -r name; do
    [ -n "$name" ] || continue
    if [ $(( i % count )) -eq $(( idx - 1 )) ]; then
      printf '%s\n' "$name"
    fi
    i=$((i + 1))
  done < <(targets "$want")
}

shard() { # <sandbox|core> <index, 1-based> <count> -> this shard's cargo flags
  local want="$1" idx="$2" count="$3" name

  validate_shard_args "$idx" "$count" || return "$?"

  # Only shard 1 carries the unit-test surface, so `--lib` runs once per CI
  # run rather than once per shard.
  if [ "$idx" -eq 1 ]; then
    case "$want" in
      sandbox) printf -- '--lib' ;;
      core)    printf -- '--bins' ;;
    esac
  fi
  while read -r name; do
    [ -n "$name" ] || continue
    printf -- ' --test %s' "$name"
  done < <(shard_names "$want" "$idx" "$count")
  printf '\n'
}

check() {
  local rc=0 f all sandbox core union

  if grep -qE '^\[\[test\]\]' "$REPO_ROOT/Cargo.toml"; then
    echo "ci-test-partition: Cargo.toml declares an explicit [[test]] target;" >&2
    echo "  this script assumes cargo's tests/*.rs auto-discovery. Update it." >&2
    rc=1
  fi

  all="$(for f in "$REPO_ROOT"/tests/*.rs; do [ -e "$f" ] && basename "$f" .rs; done | LC_ALL=C sort)"
  sandbox="$(targets sandbox)"
  core="$(targets core)"
  union="$(printf '%s\n%s\n' "$sandbox" "$core" | sed '/^$/d' | LC_ALL=C sort)"

  if [ "$union" != "$(printf '%s\n' "$all" | sed '/^$/d')" ]; then
    echo "ci-test-partition: partition is not total over tests/*.rs" >&2
    diff <(printf '%s\n' "$all") <(printf '%s\n' "$union") >&2
    rc=1
  fi

  # Disjointness: `classify` returns exactly one label per file, so an overlap
  # can only come from a future edit to `targets`. Assert it anyway -- a
  # double-run target would inflate the very wall time this split exists to cut.
  if [ "$(printf '%s\n' "$union" | LC_ALL=C uniq -d)" != "" ]; then
    echo "ci-test-partition: a target is in BOTH partitions:" >&2
    printf '%s\n' "$union" | LC_ALL=C uniq -d >&2
    rc=1
  fi

  if [ -z "$sandbox" ]; then
    echo "ci-test-partition: sandbox partition is empty -- the split would be" >&2
    echo "  vacuous and the sandbox job would assert nothing." >&2
    rc=1
  fi

  # The matrix split within the sandbox partition (AC6's second job-split) has
  # the same total/disjoint obligation as the sandbox/core split itself: a
  # target that fell out of every shard would silently stop running in CI.
  local n shard_union shard_targets
  shard_union=""
  for n in $(seq 1 "$SANDBOX_SHARDS"); do
    shard_targets="$(shard_names sandbox "$n" "$SANDBOX_SHARDS" | LC_ALL=C sort)"
    if [ -z "$shard_targets" ]; then
      echo "ci-test-partition: sandbox shard $n/$SANDBOX_SHARDS is empty -- reduce" >&2
      echo "  SANDBOX_SHARDS or the matrix will run a vacuous job." >&2
      rc=1
    fi
    shard_union="$(printf '%s\n%s\n' "$shard_union" "$shard_targets" | sed '/^$/d')"
  done
  shard_union="$(printf '%s\n' "$shard_union" | LC_ALL=C sort)"
  if [ "$shard_union" != "$(printf '%s\n' "$sandbox" | sed '/^$/d')" ]; then
    echo "ci-test-partition: the $SANDBOX_SHARDS sandbox shards are not total+disjoint over the sandbox partition" >&2
    diff <(printf '%s\n' "$sandbox") <(printf '%s\n' "$shard_union") >&2
    rc=1
  fi

  [ "$rc" -eq 0 ] && echo "ci-test-partition: ok ($(printf '%s\n' "$sandbox" | wc -l) sandbox across $SANDBOX_SHARDS shards, $(printf '%s\n' "$core" | wc -l) core)"
  return "$rc"
}

case "${1:-}" in
  sandbox|core) emit "$1" ;;
  sandbox-shard) shard sandbox "${2:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" "${3:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" ;;
  list)         targets "${2:?usage: ci-test-partition.sh list <sandbox|core>}" ;;
  check)        check ;;
  *) echo "usage: $(basename "$0") <sandbox|core|sandbox-shard <n> <of>|list <p>|check>" >&2; exit 2 ;;
esac
