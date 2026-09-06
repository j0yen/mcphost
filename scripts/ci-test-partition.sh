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
#   ci-test-partition.sh list <sandbox|core>   # bare target names, one per line
#   ci-test-partition.sh check     # exit 0 iff total, disjoint, and unshadowed
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The sandbox-execution surface. `supports_user_namespaces` and
# `require_user_namespaces_or_ci_skip` are the capability guard itself;
# `PythonKind`/`kinds::python` is the only caller that spawns sandboxed
# children today (`src/sandbox.rs`'s module doc: "python.rs is this module's
# first, and so far only, caller").
SANDBOX_SURFACE='require_user_namespaces_or_ci_skip|supports_user_namespaces|PythonKind|kinds::python'

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

  [ "$rc" -eq 0 ] && echo "ci-test-partition: ok ($(printf '%s\n' "$sandbox" | wc -l) sandbox, $(printf '%s\n' "$core" | wc -l) core)"
  return "$rc"
}

case "${1:-}" in
  sandbox|core) emit "$1" ;;
  list)         targets "${2:?usage: ci-test-partition.sh list <sandbox|core>}" ;;
  check)        check ;;
  *) echo "usage: $(basename "$0") <sandbox|core|list <p>|check>" >&2; exit 2 ;;
esac
