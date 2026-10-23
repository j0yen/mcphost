#!/usr/bin/env bash
# ci-partition-pending-age-selftest.sh -- proves the pending-entry age rungs of `ci-test-partition.sh check`:
#   age <= 3                  : rc 0, no WARN (push and --main)
#   age 4-5 (> PENDING_MAX_AGE): main PUSH (GITHUB_REF_NAME == CI_DEFAULT_BRANCH) prints a line starting
#                               `WARN pending-entry-age: ` and exits 0; `check --main` exits 1
#   age >= 6 (PENDING_HARD_MAX_AGE): rc 1 on the push and under --main
#   a non-main ref            : output and rc byte-identical to origin/main's unmodified script (the golden)
# ONE fixture git repo under its own `mktemp -d` (local refs/remotes/origin/main, built the way
# ci-suite-seed-selftest.sh builds its scratch repos); the mode is switched only by env
# (GITHUB_REF_NAME / CI_DEFAULT_BRANCH) and `--main`. Nothing outside the temp root is written.
# Env: PENDING_AGE_GOLDEN=<file> overrides the golden script (default: `git show origin/main:scripts/ci-test-partition.sh`
#   of this checkout; when neither exists the non-main case falls back to explicit expectations only).
# Prints one ok/FAIL line per case and `selftest ok` at the end; rc 0 only if every case passes.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
case "$WORK" in /tmp/*|"${TMPDIR:-/nonexistent}"/*|/var/tmp/*|"$HOME"/.cache/tmp/*) ;; *) echo "selftest: refusing odd scratch root $WORK" >&2; exit 2 ;; esac
trap 'rm -rf "$WORK"' EXIT
export CORE_SHARDS=1 SANDBOX_SHARDS=1
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset GITHUB_REF_NAME CI_DEFAULT_BRANCH GEN_TEST_SUITES_BASE_REF
fails=0
ok()  { echo "ok $1"; }
bad() { fails=$((fails + 1)); echo "FAIL $1${2:+: $2}"; }
g()   { git -C "$D" -c core.hooksPath=/dev/null "$@"; }
settle() { g add -A && g commit -q --allow-empty -m "$1" && g update-ref refs/remotes/origin/main HEAD; }
bump()   { local i; for ((i = 0; i < $1; i++)); do g commit -q --allow-empty -m "main $i"; done; g update-ref refs/remotes/origin/main HEAD; }

D="$WORK/fx"
mkdir -p "$D/tests" "$D/scripts"
printf '[package]\nname = "scratch"\nversion = "0.1.0"\n' > "$D/Cargo.toml"
printf '{\n  "lib": 5\n}\n' > "$D/scripts/ci-suite-weights.json"
echo '{}' > "$D/scripts/ci-suite-measured.json"
cp "$HERE/scripts/ci-test-partition.sh" "$D/scripts/"
# the fixture's generator caps a suite at 3 files, so three brand-new files below make a suite of their own
caps="$(grep '^MAX_PER_SUITE = ' "$HERE/scripts/gen-test-suites.sh")" max="$(grep '^MAX_SUITES = ' "$HERE/scripts/gen-test-suites.sh")"
python3 -I - "$HERE/scripts/gen-test-suites.sh" "$D/scripts/gen-test-suites.sh" "$caps" "$max" <<'PY'
import sys
src, dst, caps, mx = sys.argv[1:5]
s = open(src).read().replace(caps, 'MAX_PER_SUITE = {"core": 3, "sandbox": 3}').replace(mx, "MAX_SUITES = 6")
open(dst, "w").write(s)
PY
chmod +x "$D"/scripts/*.sh
for i in 1 2 3; do
  printf '#[test]\nfn t() {}\n' > "$D/tests/c_t$i.rs"
  printf '// PythonKind marks this file sandbox-classified\n#[test]\nfn t() {}\n' > "$D/tests/sa_t$i.rs"
done
g init -q && g checkout -q -b main && settle base || { echo "FAIL fixture git"; exit 1; }
(cd "$D" && scripts/gen-test-suites.sh >/dev/null 2>&1) || { echo "FAIL fixture gen"; exit 1; }
# a CI refresh: every generated suite weighs 4 s and measured 5 s (inside the 1.5x stale line)
python3 -I - "$D" <<'PY'
import json, sys
d = sys.argv[1]
w = json.load(open(f"{d}/scripts/ci-suite-weights.json"))
m = {}
for k in list(w):
    if k != "lib":
        w[k] = 4
        m[k] = 5.0
json.dump(w, open(f"{d}/scripts/ci-suite-weights.json", "w"), indent=2)
json.dump(m, open(f"{d}/scripts/ci-suite-measured.json", "w"), indent=2)
PY
(cd "$D" && scripts/ci-test-partition.sh check >/dev/null 2>&1) || { echo "FAIL fixture baseline check"; exit 1; }
settle measured
# three brand-new core files -> a suite made only of new files -> a pending entry (since = the fixture HEAD)
for i in 1 2 3; do printf '#[test]\nfn t() {}\n' > "$D/tests/cz_t$i.rs"; done
(cd "$D" && scripts/gen-test-suites.sh >/dev/null 2>&1) || { echo "FAIL fixture pending gen"; exit 1; }
pend="$(jq -r '(._provisional // {}) | keys[0] // empty' "$D/scripts/ci-suite-measured.json")"
[ -n "$pend" ] || { echo "FAIL fixture: no pending entry was seeded"; exit 1; }
settle land   # the land commit is age 1

# golden: origin/main's unmodified script, run from the same fixture (REPO_ROOT follows the script's own dir)
GOLD="$D/scripts/ci-test-partition.golden.sh"
if [ -n "${PENDING_AGE_GOLDEN:-}" ] && [ -f "$PENDING_AGE_GOLDEN" ]; then cp "$PENDING_AGE_GOLDEN" "$GOLD"
elif git -C "$HERE" show origin/main:scripts/ci-test-partition.sh > "$GOLD" 2>/dev/null && [ -s "$GOLD" ]; then :
else rm -f "$GOLD"; fi
[ -f "$GOLD" ] && chmod +x "$GOLD"

# run <tag> [env KEY=VAL ...] <script> check [--main]: stdout+stderr to $WORK/<tag>.out, rc to $WORK/<tag>.rc
run() {
  local tag="$1"; shift
  (cd "$D" && env "$@" >"$WORK/$tag.out" 2>&1; echo $? >"$WORK/$tag.rc")
}
PUSH=(GITHUB_REF_NAME=main CI_DEFAULT_BRANCH=main)
PR=(GITHUB_REF_NAME=496/merge CI_DEFAULT_BRANCH=main)
CHK="scripts/ci-test-partition.sh"
rc() { cat "$WORK/$1.rc"; }
warns() { grep -c '^WARN pending-entry-age: ' "$WORK/$1.out"; }

# expect <label> <push-rc> <push-warns:0|1> <main-rc> at the current age
expect() {
  local label="$1" prc="$2" pw="$3" mrc="$4" age="$5"
  run push "${PUSH[@]}" "$CHK" check
  run main "$CHK" check --main
  local gotprc gotmrc gotw
  gotprc="$(rc push)"; gotmrc="$(rc main)"; gotw="$(warns push)"
  if [ "$gotprc" = "$prc" ] && [ "$gotmrc" = "$mrc" ] && [ "$([ "$gotw" -ge 1 ] && echo 1 || echo 0)" = "$pw" ] \
     && [ "$(warns main)" = 0 ] && grep -q " is $age first-parent commits behind HEAD\| ($age first-parent commits behind HEAD" "$WORK/push.out"; then
    ok "$label (age $age: push rc=$gotprc warn=$gotw, --main rc=$gotmrc)"
  else
    bad "$label" "age=$age push rc=$gotprc (want $prc) warn=$gotw (want $pw) main rc=$gotmrc (want $mrc) :: $(grep -m1 "$pend" "$WORK/push.out")"
  fi
}

bump 2; expect "age 3 passes on push and --main, no WARN" 0 0 0 3
bump 1; expect "age 4 WARNs on push (rc 0), --main fails" 0 1 1 4
bump 1; expect "age 5 still WARNs on push (rc 0), --main fails" 0 1 1 5
bump 1; expect "age 6 fails on push and --main" 1 0 1 6

# non-main ref at age 6: byte-identical output and rc to the unmodified script
run new "${PR[@]}" "$CHK" check
if grep -q 'info only: not the default branch' "$WORK/new.out" && [ "$(rc new)" = 0 ] && [ "$(warns new)" = 0 ]; then
  if [ -f "$GOLD" ]; then
    run gold "${PR[@]}" scripts/ci-test-partition.golden.sh check
    if [ "$(rc new)" = "$(rc gold)" ] && cmp -s "$WORK/new.out" "$WORK/gold.out"; then
      ok "non-main ref at age 6 is byte-identical to origin/main's script (rc=$(rc new), output equal)"
    else
      bad "non-main ref at age 6 differs from origin/main's script" "rc new=$(rc new) gold=$(rc gold) :: $(diff "$WORK/gold.out" "$WORK/new.out" | head -4 | tr '\n' ' ')"
    fi
  else
    ok "non-main ref at age 6 is info only, rc 0, no WARN (golden unavailable: no origin/main in this checkout)"
  fi
else
  bad "non-main ref at age 6" "rc=$(rc new) :: $(head -3 "$WORK/new.out" | tr '\n' ' ')"
fi
# the unset-ref plain `check` (a developer run) is the same non-main path
run plain env -u GITHUB_REF_NAME "$CHK" check
if [ "$(rc plain)" = 0 ] && [ "$(warns plain)" = 0 ]; then ok "plain check with no ref stays info only (rc 0)"; else bad "plain check with no ref" "rc=$(rc plain)"; fi

if [ "$fails" -eq 0 ]; then echo "selftest ok"; exit 0; fi
echo "selftest FAILED: $fails case(s)"; exit 1
