#!/usr/bin/env bash
# ci-test-partition.sh -- split this crate's cargo test TARGETS (suite
# binaries, since PRD-mcphost-test-suite-consolidation) into the two jobs
# `.github/workflows/ci.yml` runs in parallel.
#
# PRD-mcphost-ci-sandbox-coverage AC6 (P1): once the sandbox suites actually
# execute in CI instead of skipping, `cargo test --workspace` measured 313-336s
# on the hosted runner -- over the PRD's own 300s budget. AC6 offers two ways
# out; this is the second one ("the suites run as a parallel job").
#
# The partition is DERIVED, never hand-listed, from per-FILE classification: a
# `tests/*.rs` file is `sandbox` iff its source mentions the sandbox-execution
# surface (the capability guard or `PythonKind`), because those are the files
# that spawn a real `bwrap`/`unshare` child and therefore need the `sandbox`
# job's userns grant. Any other file is `core`.
#
# PRD-mcphost-test-suite-consolidation (2026-09-12): `tests/*.rs` stopped
# being cargo's unit of test-binary discovery -- `gen-test-suites.sh` groups
# them into a handful of `tests/suite_<core|sandbox>_NN.rs` binaries
# (`autotests = false` + explicit `[[test]]` entries in Cargo.toml), keyed by
# filename-prefix area, SEPARATELY within each partition so no suite binary
# ever mixes a sandbox-needing file with a core one (mixing would silently
# hand a privileged capability requirement to the unprivileged `gate` job, or
# vice versa). This script's unit of CI-job routing is therefore now the
# SUITE BINARY (`suite_core_NN`/`suite_sandbox_NN`), not the individual file --
# `classify()` (per-file) still exists and is now gen-test-suites.sh's own
# source of truth too (via the `classify-file` subcommand below), so the two
# scripts can never disagree about which partition a file belongs to.
#
# Deriving it matters in one direction only, and the asymmetry is deliberate:
#   * over-inclusion (a file that merely NAMES `PythonKind` in a comment
#     lands in the sandbox partition) is harmless -- it just runs somewhere
#     that happens to have more capability than it needs;
#   * under-inclusion (a genuinely userns-dependent file left in `core`)
#     would skip silently under `$CI` and re-create the exact false-green this
#     PRD exists to remove -- so BOTH jobs assert zero capability-skips in
#     their logs. A misfiled file turns the core job red, it never passes
#     vacuously.
#
# `check` proves: gen-test-suites.sh's own suites are not drifted/incomplete
# (delegated -- see that script for the file-level total/disjoint proof over
# `tests/*.rs`), that Cargo.toml is in the post-consolidation shape
# (`autotests = false` + explicit `[[test]]`s, the OPPOSITE of the pre-PRD
# assumption this script used to assert), that every suite's membership
# agrees with `classify()` (no sandbox file inside a core suite or vice
# versa), and that the sandbox shard split is total+disjoint over the sandbox
# SUITE list.
#
# Doctests are NOT in either list: cargo rejects `--doc` combined with any
# other target selector, so the workflow runs `cargo test --doc` as its own
# step in the core job.
#
# Usage:
#   ci-test-partition.sh sandbox   # --lib --test suite_sandbox_01 ...
#   ci-test-partition.sh core      # --bins --test suite_core_01 ...
#   ci-test-partition.sh sandbox-shard <n> <of>   # this shard's slice of the
#                                                  # sandbox suite list, same
#                                                  # flag shape as `sandbox`
#   ci-test-partition.sh list <sandbox|core>   # bare SUITE names, one per line
#   ci-test-partition.sh classify-file <path>  # "sandbox" or "core" for one file
#   ci-test-partition.sh check     # exit 0 iff total, disjoint, and unshadowed
#
# AC6 follow-up: even run as its own job, the sandbox partition alone measured
# 313s (v0.13.3) against the 300s budget -- one job wasn't enough, so the
# sandbox job is further split into SANDBOX_SHARDS matrix jobs, each running
# `sandbox-shard <n> <SANDBOX_SHARDS>`. Suites are assigned to shards by
# `index-in-the-sorted-suite-list mod SANDBOX_SHARDS`. `--lib` (the
# sandbox-surface unit tests) only ships in shard 1, so it runs once per CI
# run, not once per shard.
#
# SANDBOX_SHARDS dropped from 3 to 2 with the consolidation PRD: consolidation
# groups the (now capped-at-60-files-per-suite) sandbox files into exactly 2
# suite binaries (see gen-test-suites.sh's MAX_PER_SUITE), and `check` below
# refuses a shard count that would leave any shard without a suite to run
# (same rule as before, just measured in suites instead of files). If a
# future `tests/` growth pushes gen-test-suites.sh past 2 sandbox suites,
# widen SANDBOX_SHARDS here AND the matrix in `.github/workflows/ci.yml` to
# match -- this is a manual sync, same as it was pre-consolidation.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SANDBOX_SHARDS=2

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

# suite_names <sandbox|core> -> bare suite binary names, one per line, sorted
# (tests/suite_core_NN.rs / tests/suite_sandbox_NN.rs, as written by
# gen-test-suites.sh -- never tests/*.rs member files anymore).
suite_names() {
  local want="$1" f
  for f in "$REPO_ROOT"/tests/suite_"$want"_*.rs; do
    [ -e "$f" ] || continue
    basename "$f" .rs
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
  done < <(suite_names "$want")
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

shard_names() { # <sandbox|core> <index, 1-based> <count> -> bare suite names, one per line
  local want="$1" idx="$2" count="$3" i=0 name
  validate_shard_args "$idx" "$count" || return "$?"
  while read -r name; do
    [ -n "$name" ] || continue
    if [ $(( i % count )) -eq $(( idx - 1 )) ]; then
      printf '%s\n' "$name"
    fi
    i=$((i + 1))
  done < <(suite_names "$want")
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
  local rc=0 f all_files sandbox_files core_files union
  local gen="$REPO_ROOT/scripts/gen-test-suites.sh"

  # Delegate the file-level total/disjoint/drift proof to gen-test-suites.sh
  # -- it owns the actual bucketing and is the only thing that knows what
  # "not drifted" means for tests/suite_*.rs content.
  if [ -x "$gen" ]; then
    if ! "$gen" --check; then
      echo "ci-test-partition: gen-test-suites.sh --check failed (see above)" >&2
      rc=1
    fi
  else
    echo "ci-test-partition: $gen missing or not executable" >&2
    rc=1
  fi

  if ! grep -qE '^\s*autotests\s*=\s*false' "$REPO_ROOT/Cargo.toml"; then
    echo "ci-test-partition: Cargo.toml is missing 'autotests = false' -- this" >&2
    echo "  script assumes gen-test-suites.sh's explicit [[test]] suites, not" >&2
    echo "  cargo's tests/*.rs auto-discovery. Run gen-test-suites.sh." >&2
    rc=1
  fi
  if ! grep -qE '^\[\[test\]\]' "$REPO_ROOT/Cargo.toml"; then
    echo "ci-test-partition: Cargo.toml declares no [[test]] entries -- run" >&2
    echo "  gen-test-suites.sh to generate the consolidated suites." >&2
    rc=1
  fi

  # Cross-check: every top-level tests/*.rs member file's classify() verdict
  # must agree with which partition's suite file(s) actually include it. This
  # is the guard against a suite silently mixing sandbox-needing and
  # core-only files (which would hand one job's job a capability mismatch).
  for f in "$REPO_ROOT"/tests/*.rs; do
    [ -e "$f" ] || continue
    case "$(basename "$f")" in
      suite_*.rs) continue ;;
    esac
    local base want in_core in_sandbox
    base="$(basename "$f")"
    want="$(classify "$f")"
    in_core=0; in_sandbox=0
    grep -qF "#[path = \"$base\"]" "$REPO_ROOT"/tests/suite_core_*.rs 2>/dev/null && in_core=1
    grep -qF "#[path = \"$base\"]" "$REPO_ROOT"/tests/suite_sandbox_*.rs 2>/dev/null && in_sandbox=1
    if [ "$in_core" -eq 1 ] && [ "$in_sandbox" -eq 1 ]; then
      echo "ci-test-partition: $base is included in BOTH a core and a sandbox suite" >&2
      rc=1
    elif [ "$in_core" -eq 0 ] && [ "$in_sandbox" -eq 0 ]; then
      echo "ci-test-partition: $base is not included in any suite" >&2
      rc=1
    elif [ "$want" = "sandbox" ] && [ "$in_core" -eq 1 ]; then
      echo "ci-test-partition: $base needs the sandbox capability (classify() says" >&2
      echo "  sandbox) but is filed into a core suite -- it would run unprivileged." >&2
      rc=1
    elif [ "$want" = "core" ] && [ "$in_sandbox" -eq 1 ]; then
      echo "ci-test-partition: $base does not need the sandbox capability (classify()" >&2
      echo "  says core) but is filed into a sandbox suite -- harmless but check" >&2
      echo "  gen-test-suites.sh's bucketing if this is unexpected." >&2
      rc=1
    fi
  done

  sandbox_files="$(suite_names sandbox)"
  core_files="$(suite_names core)"
  if [ -z "$sandbox_files" ]; then
    echo "ci-test-partition: no sandbox suites exist -- the split would be" >&2
    echo "  vacuous and the sandbox job would assert nothing." >&2
    rc=1
  fi
  if [ -z "$core_files" ]; then
    echo "ci-test-partition: no core suites exist." >&2
    rc=1
  fi

  # The matrix split within the sandbox partition (AC6's second job-split) has
  # the same total/disjoint obligation as the sandbox/core split itself: a
  # suite that fell out of every shard would silently stop running in CI.
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
  if [ "$shard_union" != "$(printf '%s\n' "$sandbox_files" | sed '/^$/d')" ]; then
    echo "ci-test-partition: the $SANDBOX_SHARDS sandbox shards are not total+disjoint over the sandbox suite list" >&2
    diff <(printf '%s\n' "$sandbox_files") <(printf '%s\n' "$shard_union") >&2
    rc=1
  fi

  [ "$rc" -eq 0 ] && echo "ci-test-partition: ok ($(printf '%s\n' "$sandbox_files" | wc -l) sandbox suites across $SANDBOX_SHARDS shards, $(printf '%s\n' "$core_files" | wc -l) core suites)"
  return "$rc"
}

case "${1:-}" in
  sandbox|core)  emit "$1" ;;
  sandbox-shard) shard sandbox "${2:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" "${3:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" ;;
  list)          suite_names "${2:?usage: ci-test-partition.sh list <sandbox|core>}" ;;
  classify-file) classify "${2:?usage: ci-test-partition.sh classify-file <path>}" ;;
  check)         check ;;
  *) echo "usage: $(basename "$0") <sandbox|core|sandbox-shard <n> <of>|list <p>|classify-file <path>|check>" >&2; exit 2 ;;
esac
