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
# versa), that the sandbox shard split is total+disjoint over the sandbox
# SUITE list, and that every weights key (minus lib/bin:*) has an entry in
# scripts/ci-suite-measured.json (a suite nobody measured is a weight nobody checked).
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
#   ci-test-partition.sh shards    # the sandbox shard count (SANDBOX_SHARDS)
#   ci-test-partition.sh shards core  # the core shard count (CORE_SHARDS)
#   ci-test-partition.sh core-shard <n> <of>  # this shard's slice of the core
#                                              # suite list (`--bins` on shard 1)
#   ci-test-partition.sh list <sandbox|core>   # bare SUITE names, one per line
#   ci-test-partition.sh list measured-required  # the suite keys that MUST have a measured
#                                                # entry: the weights file's keys minus `lib`
#                                                # and `bin:*` (one emitter; ci-refresh-weights.sh,
#                                                # `check` and the tests all read it)
#   ci-test-partition.sh classify-file <path>  # "sandbox" or "core" for one file
#   ci-test-partition.sh classify-files <path>...  # the same, one line per path, one grep
#   ci-test-partition.sh classify-agree  # exit 0 iff classify-files == classify-file over tests/*.rs
#   ci-test-partition.sh check     # exit 0 iff total, disjoint, and unshadowed
#
# AC6 follow-up: even run as its own job, the sandbox partition alone measured
# 313s (v0.13.3) against the 300s budget -- one job wasn't enough, so the
# sandbox job is further split into SANDBOX_SHARDS matrix jobs, each running
# `sandbox-shard <n> <SANDBOX_SHARDS>`.
#
# PRD-mcphost-ci-sandbox-shards-balanced: the split is by COST, not position.
# `scripts/ci-suite-weights.json` records seconds per sandbox suite binary and
# for `--lib` (from the latest green CI run's `finished in` lines); suites are
# placed heaviest first into the lightest shard (ties: sorted name, lowest
# shard), then `--lib` goes to the lightest shard. `check` fails when a suite
# has no weight or when the predicted heaviest shard exceeds
# SHARD_PREDICT_LIMIT seconds. SANDBOX_SHARDS below is the ONE definition of
# the shard count: `shards` prints it, ci.yml's plan job and
# gen-test-suites.sh read that command, and a test fails if a literal count
# reappears in either file.
#
# PRD-mcphost-ci-core-suites-sharded: the core partition is split the same way.
# One serial `cargo test` step ran every core suite binary (13 to 16 minutes
# behind ~5 minutes of setup) and main's runs of the required check sat at 20
# to 30 minutes against its cap. `core-shard <n> <of>` places the core suites
# heaviest first into the lightest shard from the same weights file, CORE_SHARDS
# is the ONE core count (`shards core` prints it; ci.yml's plan job emits the
# core matrix from that), and `check` fails when a core suite has no weight or
# the predicted heaviest core shard exceeds CORE_FAIL_ABOVE (it warns above CORE_WARN_AT).
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SANDBOX_SHARDS="${SANDBOX_SHARDS:-4}"
CORE_SHARDS="${CORE_SHARDS:-5}"

# Recorded seconds per sandbox suite (and `lib`); overridable for tests.
WEIGHTS_FILE="${CI_SUITE_WEIGHTS_FILE:-$REPO_ROOT/scripts/ci-suite-weights.json}"
WEIGHTS_REL="scripts/ci-suite-weights.json"
# Max MEASURED CI seconds per suite, written by scripts/ci-refresh-weights.sh (and
# rescaled by gen-test-suites.sh when a re-pack resizes a suite, like its weight)
# (the `suite=<name> measured=<s>s` lines of a CI run log; the same script also
# rewrites the recorded weights from them). `check` reads this committed file, so
# it needs no network and runs on every PR.
MEASURED_FILE="${CI_SUITE_MEASURED_FILE:-$REPO_ROOT/scripts/ci-suite-measured.json}"
MEASURED_REL="scripts/ci-suite-measured.json"
# A recorded weight is STALE when the measured seconds reach this multiple of it.
# 2026-10-18: suite_core_07/08 were recorded at 341/375 s while CI measured
# 586-735/427-711 s, and `check` kept predicting 375 s from the old numbers.
STALE_RATIO=1.5
# THE shard run-wall limits (seconds): the ONE definition. `limits <partition>`
# prints "<warn_at> <fail_above>" and ci-shard-report.sh reads its CI run-wall
# lines from there, so `check` and CI can never disagree.
SANDBOX_WARN_AT=240
SANDBOX_FAIL_ABOVE=300
# 10-19: core lines 540/720 -> 720/900. The fail line must cover the largest SOLO
# suite's own contract budget: suite_core_04 is ctrs_ac06 alone (re-runs every contract
# suite through pin-tests.sh, BUDGET_SECS 900) and measured 770 s on run 39196582126
# (585 s on main 39203293144), so a 720 s wall failed a suite its own contract passes.
CORE_WARN_AT=720
CORE_FAIL_ABOVE=900
# Predicted heaviest sandbox shard above this fails `check` (the CI run budget warns
# at 240 s and fails above 300 s; see ci.yml's sandbox step).
SHARD_PREDICT_LIMIT=$SANDBOX_WARN_AT
# Core: the predicted heaviest core shard WARNS above CORE_WARN_AT (stderr,
# exit 0) and FAILS only above CORE_FAIL_ABOVE -- the same two lines CI's
# run wall uses, so `check` can never block a shard CI would run green. The recorded
# core weights are the MAX measured hosted-runner seconds (refreshed 2026-10-18 from
# CI runs 39129276291, 39133346672, 39133722354; the old 420 bound belonged to
# fast-runner weights that understated suite_core_07/08 by 1.7-2x), so a predicted
# shard in the warn-fail band is real but still inside CI's budget (ryzen7 runs 410 and
# 404 were gate-blocked at a predicted 608 s that CI runs fine).
CORE_SHARD_PREDICT_LIMIT=$CORE_FAIL_ABOVE

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

# classify_files <file>... -> one "sandbox"/"core" line per argument, in order:
# classify() for many files with ONE grep. 2026-10-19: `check` and
# gen-test-suites.sh ran classify() once per tests/*.rs file (2220 files,
# three subprocess greps each), 58 s per `check` on a 4-CPU builder; a dozen
# contract tests call `check`, which took suite_contracts_core_01 to 905 s on
# CI run 39349170227 and past core-static's 20-minute cap.
classify_files() {
  local -A hit=()
  local f
  while IFS= read -r f; do
    [ -n "$f" ] && hit["$f"]=1
  done < <(grep -lE "$SANDBOX_SURFACE" "$@" 2>/dev/null)
  for f in "$@"; do
    if [ -n "${hit[$f]:-}" ]; then echo sandbox; else echo core; fi
  done
}

# classify_agree -> exit 0 iff classify_files gives, for every non-suite
# tests/*.rs file, the verdict classify() gives for that file alone. The
# batched path is what `check` and gen-test-suites.sh use; classify() is the
# per-file definition. A contract test runs this so the two cannot drift.
classify_agree() {
  local -a files=() batch=() single=()
  local f i bad=0
  for f in "$REPO_ROOT"/tests/*.rs; do
    [ -e "$f" ] || continue
    case "${f##*/}" in
      suite_*.rs) continue ;;
    esac
    files+=("$f")
  done
  mapfile -t batch < <(classify_files "${files[@]}")
  mapfile -t single < <(for f in "${files[@]}"; do classify "$f"; done)
  if [ "${#batch[@]}" -ne "${#files[@]}" ] || [ "${#single[@]}" -ne "${#files[@]}" ]; then
    echo "classify-agree: ${#files[@]} files, classify_files gave ${#batch[@]} verdicts, classify() gave ${#single[@]}" >&2
    return 1
  fi
  for i in "${!files[@]}"; do
    if [ "${batch[$i]}" != "${single[$i]}" ]; then
      echo "classify-agree: ${files[$i]#"$REPO_ROOT"/}: classify() says ${single[$i]}, classify_files says ${batch[$i]}" >&2
      bad=1
    fi
  done
  [ "$bad" -eq 0 ] || return 1
  echo "classify-agree: ok (${#files[@]} files)"
}

# path_members <suite file>... -> the "#[path = \"<base>\"]" bases those files
# include, one per line (the literal `check` used to grep for, per file).
path_members() {
  cat "$@" 2>/dev/null | grep -oE '#\[path = "[^"]*"\]' | sed -E 's/^#\[path = "(.*)"\]$/\1/'
}

# suite_names <sandbox|core> -> bare suite binary names, one per line, sorted
# (tests/suite_core_NN.rs / tests/suite_sandbox_NN.rs, PLUS
# tests/suite_env_core_NN.rs / tests/suite_env_sandbox_NN.rs, PLUS
# tests/suite_contracts_core_NN.rs / tests/suite_contracts_sandbox_NN.rs, PLUS
# tests/suite_cargo_core_NN.rs / tests/suite_cargo_sandbox_NN.rs
# (PRD-mcphost-contract-suite-registry-axis-b: files that spawn cargo, kept out
# of any binary that execs the built bin)
# (PRD-mcphost-contract-suite-generated: `//! contract:`-marked files) --
# PRD-mcphost-test-suite-flake-lints requirement 5's env-guarded-only
# binaries, capability-partitioned the same as every other suite -- as
# written by gen-test-suites.sh. Never tests/*.rs member files themselves.
suite_names() {
  local want="$1" f
  for f in "$REPO_ROOT"/tests/suite_"$want"_*.rs "$REPO_ROOT"/tests/suite_env_"$want"_*.rs \
           "$REPO_ROOT"/tests/suite_contracts_"$want"_*.rs "$REPO_ROOT"/tests/suite_cargo_"$want"_*.rs; do
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

weight_of() { # <name> -> recorded seconds, empty when absent
  jq -r --arg k "$1" '.[$k] // empty' "$WEIGHTS_FILE" 2>/dev/null
}

# measured_required -> the suite keys scripts/ci-suite-measured.json must carry one entry
# for: every key of the weights file except `lib` and `bin:*`. DERIVED from the weights
# file (which gen-test-suites.sh keeps in sync with the suite files), never a literal
# list; ci-refresh-weights.sh's completeness rule, `check` and the tests all call
# `ci-test-partition.sh list measured-required`, so they cannot drift.
measured_required() {
  jq -r 'keys[] | select(. != "lib" and (startswith("bin:") | not))' "$WEIGHTS_FILE"
}

# assign_weighted <sandbox|core> <count> -> "<shard> <name> <weight>" lines for
# every suite of that partition (plus, for sandbox only, the pseudo-name `lib`):
# suites heaviest first into the lightest shard, then lib into the lightest.
# Core has no lib entry: its `--bins` ride on shard 1 and weigh nothing.
# Deterministic: ties by sorted name, then lowest shard index. Exit 1 (naming
# each) when a weight is missing.
assign_weighted() {
  local want="$1" count="$2" name missing=0 rows=""
  while read -r name; do
    [ -n "$name" ] || continue
    if [ -z "$(weight_of "$name")" ]; then
      echo "ci-test-partition: $want suite '$name' has no weight in $WEIGHTS_REL -- record its measured seconds there" >&2
      missing=1
    else
      rows="$rows$(weight_of "$name") $name"$'\n'
    fi
  done < <(suite_names "$want")
  if [ "$want" = sandbox ] && [ -z "$(weight_of lib)" ]; then
    echo "ci-test-partition: 'lib' has no weight in $WEIGHTS_REL -- record the seconds of the --lib run there" >&2
    missing=1
  fi
  [ "$missing" -eq 0 ] || return 1
  {
    printf '%s' "$rows" | LC_ALL=C sort -k1,1nr -k2,2
    if [ "$want" = sandbox ]; then printf '%s lib\n' "$(weight_of lib)"; fi
  } | awk -v n="$count" '
    { w[NR] = $1; nm[NR] = $2 }
    END {
      for (s = 1; s <= n; s++) load[s] = 0
      # lib (sandbox only) is always the last line: placed after every suite.
      for (i = 1; i <= NR; i++) {
        best = 1
        for (s = 2; s <= n; s++) if (load[s] < load[best]) best = s
        load[best] += w[i]
        print best, nm[i], w[i]
      }
    }'
}

shard_names() { # <sandbox|core> <index, 1-based> <count> -> bare suite names, one per line
  local want="$1" idx="$2" count="$3" name sh weight plan
  validate_shard_args "$idx" "$count" || return "$?"
  plan="$(assign_weighted "$want" "$count")" || return 1
  while read -r sh name weight; do
    if [ "$sh" = "$idx" ] && [ "$name" != lib ]; then printf '%s\n' "$name"; fi
  done <<<"$plan"
}

shard() { # <sandbox|core> <index, 1-based> <count> -> this shard's cargo flags
  local want="$1" idx="$2" count="$3" name names plan sh nm weight

  validate_shard_args "$idx" "$count" || return "$?"

  # `--lib` runs once per CI run: sandbox puts it on the lightest shard
  # (assign_weighted); core keeps `--bins` on shard 1.
  case "$want" in
    sandbox)
      plan="$(assign_weighted sandbox "$count")" || return 1
      while read -r sh nm weight; do
        if [ "$nm" = lib ] && [ "$sh" = "$idx" ]; then printf -- '--lib'; fi
      done <<<"$plan"
      ;;
    core) if [ "$idx" -eq 1 ]; then printf -- '--bins'; fi ;;
  esac
  names="$(shard_names "$want" "$idx" "$count")" || return 1
  while read -r name; do
    [ -n "$name" ] || continue
    printf -- ' --test %s' "$name"
  done <<<"$names"
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

  # Same P0 cap CI's suite_ac1_ten_binaries_and_names_preserved enforces: read
  # from the ONE definition (MAX_SUITES in gen-test-suites.sh), never a copy.
  local max_suites n_suites
  max_suites="$(grep -oE '^MAX_SUITES *= *[0-9]+' "$gen" | grep -oE '[0-9]+$')"
  n_suites="$(ls "$REPO_ROOT"/tests/suite_core_*.rs "$REPO_ROOT"/tests/suite_sandbox_*.rs 2>/dev/null | wc -l)"
  if [ -z "$max_suites" ]; then
    echo "ci-test-partition: cannot read MAX_SUITES from $gen" >&2
    rc=1
  elif [ "$n_suites" -gt "$max_suites" ]; then
    echo "ci-test-partition: $n_suites tests/suite_core_*.rs + tests/suite_sandbox_*.rs files exceed MAX_SUITES=$max_suites (the P0 cap CI enforces); regenerate with scripts/gen-test-suites.sh" >&2
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
  # One classify_files grep and one #[path] read per partition, then lookups
  # (was three subprocess greps per member file).
  local -a members=() verdicts=()
  local -A core_member=() sandbox_member=() want_of=()
  local m i
  for f in "$REPO_ROOT"/tests/*.rs; do
    [ -e "$f" ] || continue
    case "$(basename "$f")" in
      suite_*.rs) continue ;;
    esac
    members+=("$f")
  done
  if [ "${#members[@]}" -gt 0 ]; then
    mapfile -t verdicts < <(classify_files "${members[@]}")
    for i in "${!members[@]}"; do want_of["${members[$i]}"]="${verdicts[$i]}"; done
  fi
  while IFS= read -r m; do [ -n "$m" ] && core_member["$m"]=1; done < <(path_members "$REPO_ROOT"/tests/suite_core_*.rs "$REPO_ROOT"/tests/suite_env_core_*.rs "$REPO_ROOT"/tests/suite_contracts_core_*.rs "$REPO_ROOT"/tests/suite_cargo_core_*.rs)
  while IFS= read -r m; do [ -n "$m" ] && sandbox_member["$m"]=1; done < <(path_members "$REPO_ROOT"/tests/suite_sandbox_*.rs "$REPO_ROOT"/tests/suite_env_sandbox_*.rs "$REPO_ROOT"/tests/suite_contracts_sandbox_*.rs "$REPO_ROOT"/tests/suite_cargo_sandbox_*.rs)
  for f in "${members[@]}"; do
    local base want in_core in_sandbox
    base="${f##*/}"
    want="${want_of[$f]}"
    in_core=0; in_sandbox=0
    [ -n "${core_member[$base]:-}" ] && in_core=1
    [ -n "${sandbox_member[$base]:-}" ] && in_sandbox=1
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

  # Cost model: every sandbox suite (and lib) needs a recorded weight, and the
  # predicted heaviest shard must leave headroom under the CI run budget.
  local plan heaviest
  if plan="$(assign_weighted sandbox "$SANDBOX_SHARDS" 2>&1)"; then
    heaviest="$(printf '%s\n' "$plan" | awk '{ l[$1] += $3 } END { m = 0; for (s in l) if (l[s] > m) m = l[s]; print m }')"
    if awk -v h="$heaviest" -v l="$SHARD_PREDICT_LIMIT" 'BEGIN { exit !(h > l) }'; then
      echo "ci-test-partition: predicted heaviest sandbox shard is ${heaviest}s (> ${SHARD_PREDICT_LIMIT}s) from the weights in $WEIGHTS_REL -- widen SANDBOX_SHARDS or refresh the weights" >&2
      rc=1
    fi
  else
    printf '%s\n' "$plan" >&2
    rc=1
  fi

  # The core partition has the same obligations (PRD-mcphost-ci-core-suites-
  # sharded): the CORE_SHARDS shards are total and disjoint over the core suite
  # list (a suite that fell out of every shard would silently stop running in
  # CI), none is empty, every core suite has a weight, and the predicted
  # heaviest core shard stays at or under CORE_FAIL_ABOVE (warns above CORE_WARN_AT).
  local core_heaviest="?" core_union core_plan
  if core_plan="$(assign_weighted core "$CORE_SHARDS" 2>&1)"; then
    core_union=""
    for n in $(seq 1 "$CORE_SHARDS"); do
      shard_targets="$(shard_names core "$n" "$CORE_SHARDS" | LC_ALL=C sort)"
      if [ -z "$shard_targets" ]; then
        echo "ci-test-partition: core shard $n/$CORE_SHARDS is empty -- reduce" >&2
        echo "  CORE_SHARDS or the matrix will run a vacuous job." >&2
        rc=1
      fi
      core_union="$(printf '%s\n%s\n' "$core_union" "$shard_targets" | sed '/^$/d')"
    done
    core_union="$(printf '%s\n' "$core_union" | LC_ALL=C sort)"
    if [ "$core_union" != "$(printf '%s\n' "$core_files" | sed '/^$/d')" ]; then
      echo "ci-test-partition: the $CORE_SHARDS core shards are not total+disjoint over the core suite list" >&2
      diff <(printf '%s\n' "$core_files") <(printf '%s\n' "$core_union") >&2
      rc=1
    fi
    core_heaviest="$(printf '%s\n' "$core_plan" | awk '{ l[$1] += $3 } END { m = 0; for (s in l) if (l[s] > m) m = l[s]; print m }')"
    if awk -v h="$core_heaviest" -v l="$CORE_SHARD_PREDICT_LIMIT" 'BEGIN { exit !(h > l) }'; then
      echo "ci-test-partition: predicted heaviest core shard is ${core_heaviest}s (> ${CORE_SHARD_PREDICT_LIMIT}s) from the weights in $WEIGHTS_REL -- widen CORE_SHARDS or refresh the weights" >&2
      rc=1
    elif awk -v h="$core_heaviest" -v l="$CORE_WARN_AT" 'BEGIN { exit !(h > l) }'; then
      echo "ci-test-partition: WARNING predicted heaviest core shard is ${core_heaviest}s (> ${CORE_WARN_AT}s warn line, fail line ${CORE_FAIL_ABOVE}s) from the weights in $WEIGHTS_REL -- CI runs it, but refresh the weights or widen CORE_SHARDS before it nears ${CORE_FAIL_ABOVE}s" >&2
    fi
  else
    printf '%s\n' "$core_plan" >&2
    rc=1
  fi

  # Staleness guard (one source, one reader): scripts/ci-suite-measured.json holds
  # the max measured CI seconds that scripts/ci-refresh-weights.sh also derives the
  # recorded weights from. A suite measured at >= STALE_RATIO x its recorded weight
  # fails here, so the weights can never again pass `check` on old numbers.
  local stale
  if [ ! -f "$MEASURED_FILE" ]; then
    echo "ci-test-partition: $MEASURED_REL is missing -- run scripts/ci-refresh-weights.sh <ci-log>..." >&2
    rc=1
  elif ! stale="$(jq -r --argjson r "$STALE_RATIO" --slurpfile w "$WEIGHTS_FILE" \
        'to_entries[] | select(($w[0][.key] // null) != null and .value >= $r * $w[0][.key])
         | "\(.key) measured \(.value)s >= \($r)x recorded \($w[0][.key])s"' "$MEASURED_FILE" 2>&1)"; then
    echo "ci-test-partition: cannot read $MEASURED_REL: $stale" >&2
    rc=1
  elif [ -n "$stale" ]; then
    while IFS= read -r line; do
      echo "ci-test-partition: stale weight: $line in $WEIGHTS_REL -- run scripts/ci-refresh-weights.sh on a CI run log" >&2
    done <<<"$stale"
    rc=1
  fi

  # Completeness guard: a suite with a recorded weight but no measurement is exactly what hid
  # suite_sandbox_02 recorded at 152 s while it ran ~300 s (a shard under its warn line used to
  # report nothing, and the guard above only compares keys that are present). The required key
  # set is `list measured-required` (the weights file's keys minus lib/bin:*), never a literal list.
  local unmeasured
  if [ -f "$MEASURED_FILE" ] && jq -e . "$MEASURED_FILE" >/dev/null 2>&1; then
    unmeasured="$(measured_required | jq -R -r --slurpfile m "$MEASURED_FILE" '. as $k | select(($m[0] | has($k)) | not)')"
    if [ -n "$unmeasured" ]; then
      while IFS= read -r line; do
        echo "ci-test-partition: unmeasured suite: $line has a recorded weight in $WEIGHTS_REL but no entry in $MEASURED_REL -- run scripts/ci-refresh-weights.sh <CI run id> (every shard reports every suite)" >&2
      done <<<"$unmeasured"
      rc=1
    fi
  fi

  # Provisional seeds: gen-test-suites.sh gives a suite created from only new test files a measured
  # entry at its recorded weight x the partition's measured/recorded ratio and lists it under the
  # reserved `_provisional` key ({suite: seed date}) until scripts/ci-refresh-weights.sh measures it.
  # A WARNING, never a failure: main must not go red on a date.
  if [ -f "$MEASURED_FILE" ]; then
    jq -r '(._provisional // {}) | if type == "object" then to_entries[] | "\(.key) \(.value)" else empty end' "$MEASURED_FILE" 2>/dev/null \
      | while read -r pname pdate; do
          echo "ci-test-partition: WARNING provisional measured entry: $pname was seeded on $pdate from its recorded weight, not measured -- run scripts/ci-refresh-weights.sh <CI run id> to replace it" >&2
        done
  fi

  [ "$rc" -eq 0 ] && echo "ci-test-partition: ok ($(printf '%s\n' "$sandbox_files" | wc -l) sandbox suites across $SANDBOX_SHARDS shards, predicted heaviest shard ${heaviest}s, $(printf '%s\n' "$core_files" | wc -l) core suites across $CORE_SHARDS shards, predicted heaviest core shard ${core_heaviest}s)"
  return "$rc"
}

case "${1:-}" in
  sandbox|core)  emit "$1" ;;
  shards)        case "${2:-sandbox}" in
                   sandbox) echo "$SANDBOX_SHARDS" ;;
                   core)    echo "$CORE_SHARDS" ;;
                   *) echo "usage: ci-test-partition.sh shards [sandbox|core]" >&2; exit 2 ;;
                 esac ;;
  sandbox-shard) shard sandbox "${2:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" "${3:?usage: ci-test-partition.sh sandbox-shard <n> <of>}" ;;
  core-shard)    shard core "${2:?usage: ci-test-partition.sh core-shard <n> <of>}" "${3:?usage: ci-test-partition.sh core-shard <n> <of>}" ;;
  list)          case "${2:-}" in
                   measured-required) measured_required ;;
                   *) suite_names "${2:?usage: ci-test-partition.sh list <sandbox|core|measured-required>}" ;;
                 esac ;;
  classify-file) classify "${2:?usage: ci-test-partition.sh classify-file <path>}" ;;
  classify-files) shift; classify_files "$@" ;;
  classify-agree) classify_agree ;;
  limits)        case "${2:-}" in
                   sandbox) echo "$SANDBOX_WARN_AT $SANDBOX_FAIL_ABOVE" ;;
                   core)    echo "$CORE_WARN_AT $CORE_FAIL_ABOVE" ;;
                   *) echo "usage: ci-test-partition.sh limits <sandbox|core>" >&2; exit 2 ;;
                 esac ;;
  check)         check ;;
  *) echo "usage: $(basename "$0") <sandbox|core|shards [sandbox|core]|sandbox-shard <n> <of>|core-shard <n> <of>|list <p|measured-required>|limits <p>|classify-file <path>|classify-files <path>...|classify-agree|check>" >&2; exit 2 ;;
esac
