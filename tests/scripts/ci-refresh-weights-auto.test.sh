#!/usr/bin/env bash
# ci-refresh-weights-auto.test.sh -- proof for scripts/ci-refresh-weights-auto.sh.
#
# Bash only, no network, no cargo. The three inner scripts are stubbed through CI_REFRESH_AUTO_BIN_DIR and the
# checkout is a throwaway git repo (CI_REFRESH_AUTO_ROOT). Prints one line per check; exits 1 on any FAIL.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 2
REPO="$PWD"
AUTO="$REPO/scripts/ci-refresh-weights-auto.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
status=0

check() {
  local desc="$1"
  shift
  if "$@"; then
    echo "ok - $desc"
  else
    echo "FAIL - $desc"
    status=1
  fi
}

# ---- the self-skip predicate -------------------------------------------------------------------
predicate_is() { [ "$("$AUTO" --selftest-predicate "$1")" = "$2" ]; }
check "refresh subject is self" predicate_is "ci: refresh suite weights from run 123" self
check "bare prefix is self" predicate_is "ci: refresh suite weights" self
check "other ci subject is none" predicate_is "ci: auto-refresh suite weights after green main CI" none
check "feature subject is none" predicate_is "loop: something (#540)" none
check "empty subject is none" predicate_is "" none

# ---- fixture: a temp git repo plus stub inner scripts -------------------------------------------
BIN="$WORK/bin"
ROOT="$WORK/root"
mkdir -p "$BIN" "$ROOT/scripts" "$ROOT/tests"
git -C "$ROOT" init -q
git -C "$ROOT" config user.name t
git -C "$ROOT" config user.email t@example.invalid
echo '{"suite_core_01": 10}' > "$ROOT/scripts/ci-suite-weights.json"
echo '{"suite_core_01": 9.5}' > "$ROOT/scripts/ci-suite-measured.json"
echo '# root' > "$ROOT/Cargo.toml"
git -C "$ROOT" add -A
git -C "$ROOT" commit -q -m "loop: base"

# Stubs: CI_REFRESH_STUB_EDIT=1 makes the refresh stub rewrite a weight; CI_REFRESH_STUB_FAIL=<name> makes that stub exit 7.
stub() {
  local name="$1" body="$2"
  printf '%s\n' '#!/usr/bin/env bash' \
    "[ \"\${CI_REFRESH_STUB_FAIL:-}\" = \"$name\" ] && exit 7" \
    "echo \"stub $name \$*\" >> \"$WORK/calls\"" \
    "$body" > "$BIN/$name"
  chmod +x "$BIN/$name"
}
stub ci-refresh-weights.sh '[ "${CI_REFRESH_STUB_EDIT:-}" = 1 ] && echo "{\"suite_core_01\": 11}" > scripts/ci-suite-weights.json; echo noise-from-inner; exit 0'
stub gen-test-suites.sh 'exit 0'
stub ci-test-partition.sh 'exit 0'

run_auto() { CI_REFRESH_AUTO_BIN_DIR="$BIN" CI_REFRESH_AUTO_ROOT="$ROOT" "$AUTO" "$@"; }

out_is() { # <expected stdout> <env assignments...> -- run id 4242
  local want="$1"
  shift
  local got
  got="$(env "$@" CI_REFRESH_AUTO_BIN_DIR="$BIN" CI_REFRESH_AUTO_ROOT="$ROOT" "$AUTO" 4242 2>/dev/null)" || return 1
  [ "$got" = "$want" ]
}

# ---- changed= detection -------------------------------------------------------------------------
: > "$WORK/calls"
check "no diff reports changed=0 skip=none" out_is "changed=0 skip=none" CI_REFRESH_STUB_EDIT=0
check "all three inner scripts ran, in order" bash -c "[ \"\$(cut -d' ' -f2 '$WORK/calls' | tr '\n' ' ')\" = 'ci-refresh-weights.sh gen-test-suites.sh ci-test-partition.sh ' ]"
check "the run id reaches the refresh script" grep -q 'stub ci-refresh-weights.sh 4242' "$WORK/calls"
check "a weights edit reports changed=1 skip=none" out_is "changed=1 skip=none" CI_REFRESH_STUB_EDIT=1
git -C "$ROOT" checkout -q -- .
echo 'x' > "$ROOT/tests/suite_core_99.rs"
check "a new file under tests/ reports changed=1" out_is "changed=1 skip=none" CI_REFRESH_STUB_EDIT=0
rm -f "$ROOT/tests/suite_core_99.rs"
echo 'x' > "$ROOT/unrelated.txt"
check "a change outside the watched paths reports changed=0" out_is "changed=0 skip=none" CI_REFRESH_STUB_EDIT=0
rm -f "$ROOT/unrelated.txt"

# stdout carries exactly the one result line (inner noise goes to stderr)
check "stdout is exactly one line" bash -c "[ \"\$(CI_REFRESH_AUTO_BIN_DIR='$BIN' CI_REFRESH_AUTO_ROOT='$ROOT' '$AUTO' 4242 2>/dev/null | wc -l)\" = 1 ]"

# ---- fail-closed: a non-zero inner exit propagates, no result line -------------------------------
for inner in ci-refresh-weights.sh gen-test-suites.sh ci-test-partition.sh; do
  fails_closed() {
    local out rc
    out="$(CI_REFRESH_STUB_FAIL="$inner" CI_REFRESH_AUTO_BIN_DIR="$BIN" CI_REFRESH_AUTO_ROOT="$ROOT" "$AUTO" 4242 2>/dev/null)"
    rc=$?
    [ "$rc" -ne 0 ] && [ -z "$out" ]
  }
  check "a failing $inner exits non-zero and prints no result" fails_closed
done

# ---- self skip: HEAD is the refresh PR's own commit ----------------------------------------------
: > "$WORK/calls"
git -C "$ROOT" commit -q --allow-empty -m "ci: refresh suite weights from run 1"
check "refresh HEAD reports changed=0 skip=self" bash -c "[ \"\$(CI_REFRESH_AUTO_BIN_DIR='$BIN' CI_REFRESH_AUTO_ROOT='$ROOT' '$AUTO' 4242 2>/dev/null)\" = 'changed=0 skip=self' ]"
check "self skip runs none of the inner scripts" bash -c "[ ! -s '$WORK/calls' ]"

# ---- argument validation -------------------------------------------------------------------------
bad_arg_exits_2() { run_auto "not-a-run-id" >/dev/null 2>&1; [ $? -eq 2 ]; }
check "a non-numeric run id exits 2" bad_arg_exits_2
no_arg_exits_2() { run_auto >/dev/null 2>&1; [ $? -eq 2 ]; }
check "no argument exits 2" no_arg_exits_2

exit "$status"
