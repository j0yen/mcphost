#!/usr/bin/env bash
# ci-suite-seed-selftest.sh -- proves a suite created from ONLY new test files gets a PENDING measured
# entry that carries its derived value and the commit it was derived at, and that
# `ci-test-partition.sh check` bounds how long it may live.
#
# PR 495 / CI run 39913385513: gen-test-suites.sh seeded a created suite's measured entry only from
# member files that came from OLD suites, so suite_core_12 (all new files) had a recorded weight and no
# measured entry, and the completeness guard of `check` failed every core, sandbox and regen job
# before any shard could print the `suite=<name> measured=` lines scripts/ci-refresh-weights.sh reads.
# #497 seeded it under a date-valued `_provisional` mark; a date ties the seed to nothing, so a stale or
# hand-edited value passed forever. The mark is now {"pending": true, "derived_s": <seed>, "since": <merge-base
# of the generated tree with origin/main>}.
#
# Every test runs in a `mktemp -d` copy (a git repo with a local refs/remotes/origin/main); nothing outside it
# is written. Prints one PASS/FAIL line per test and exits 1 on any FAIL.
#   (a) the run-39913385513 fixture (suite_core_12 made of only new files): gen no longer leaves it unmeasured,
#       the entry has the {pending, derived_s, since = fixture HEAD} shape, `check` prints `pending (derived)`
#   (b) a pending entry whose measured value is edited away from derived_s fails `check` naming both numbers;
#       2.5 vs 2.50 (numeric, not textual) still passes; a legacy date-valued mark fails
#   (c) age from the base: 3 first-parent commits passes, 4 fails on the default branch (--main or
#       GITHUB_REF_NAME=main); on a PR ref the count is info only; a `since` that is no ancestor fails
#   (d) ci-refresh-weights.sh over shard-log lines measuring the suite removes the object; one omitting it exits 1
#   (e) the unmodified tree: gen-test-suites.sh --check is ok, a plain run writes nothing, `check` is ok
#   (g) a shallow clone: `check` deepens once and finds `since`; a `since` that is not on main fails with
#       `since unreachable: fetch-depth too shallow or since not on main`
#   (h) a resized suite is not marked; a resized pending suite keeps its `since` and re-derives derived_s;
#       a no-op regen is "already up to date"
#   (i) a suite that ALREADY EXISTS (tests/suite_*.rs + recorded weight) with NO measured entry -- a branch cut before
#       #497 merged -- is seeded pending by gen (recorded weight x R, since = merge-base with origin/main) and `check`
#       passes; (i-moved) the same when one member came from a measured suite: ratio path, not marked
#   (ii) the rebase path: the measured file resolved to main's side lost the branch's own entry (main moved 5
#       commits) -> same outcome; then the branch gets 4 own commits and main 5 more: `check --main` still passes at
#       age 0, because the age ends at merge-base(HEAD, origin/main), never at main's tip and never at HEAD
#   (iii) a pre-existing suite WITH an entry is untouched (value and `_provisional` object byte-identical, no new
#       `_provisional` key); (iii-stale) a stale `_provisional` mark for a suite with no entry is ignored (fresh seed)
#   (iv) idempotency: a second gen run prints "already up to date" and writes nothing
# Env: SEED_SELFTEST_SCRIPTS=<dir> runs the three scripts from <dir> instead of this checkout's scripts/
#   (red-at-base (f): point it at origin/main's scripts and (a), (b), (c) must FAIL; for the pre-existing-suite
#   cases, (i), (i-moved), (ii) and (iii-stale) must FAIL there too).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${SEED_SELFTEST_SCRIPTS:-$HERE/scripts}"
WORK="$(mktemp -d)"
case "$WORK" in /tmp/*|"${TMPDIR:-/nonexistent}"/*|/var/tmp/*|"$HOME"/.cache/tmp/*) ;; *) echo "selftest: refusing odd scratch root $WORK" >&2; exit 2 ;; esac
trap 'rm -rf "$WORK"' EXIT
export CORE_SHARDS=1 SANDBOX_SHARDS=1
# the scratch repos use a local origin/main ref and never the caller's git config, hooks or branch name
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset GITHUB_REF_NAME CI_DEFAULT_BRANCH GEN_TEST_SUITES_BASE_REF
fails=0 passes=0
ok()   { passes=$((passes + 1)); echo "PASS $1"; }
bad()  { fails=$((fails + 1)); echo "FAIL $1${2:+: $2}"; }
g()    { local d="$1"; shift; git -C "$d" -c core.hooksPath=/dev/null "$@"; }
# settle <dir> <msg>: commit the tree as a main commit and point the local origin/main at it
settle() { g "$1" add -A && g "$1" commit -q --allow-empty -m "$2" && g "$1" update-ref refs/remotes/origin/main HEAD; }
# bump <dir> <n>: n more first-parent commits on main
bump() { local i; for ((i = 0; i < $2; i++)); do g "$1" commit -q --allow-empty -m "main $i"; done; g "$1" update-ref refs/remotes/origin/main HEAD; }
jget() { python3 -I -c 'import json,sys; print(json.dumps(eval(sys.argv[2], {"d": json.load(open(sys.argv[1]))})))' "$@"; }
# setj <file> <jq filter>: rewrite a json file inside a scratch repo
setj() { jq "$2" "$1" > "$1.tmp" && mv "$1.tmp" "$1"; }

# scratch <name> [core_areas=1] -> a git repo (local refs/remotes/origin/main) with core files in <core_areas> areas
# (3 files each, one suite at cap 3) and sandbox files in sa (3), gen-run once, then "CI-measured": every suite
# measured 1.25x its recorded weight (inside the 1.5x stale line). HEAD == origin/main on return.
scratch() {
  local d="$WORK/$1" areas="${2:-1}" maxs=6
  [ "$areas" -gt 1 ] && maxs=20
  mkdir -p "$d/tests" "$d/scripts"
  printf '[package]\nname = "scratch"\nversion = "0.1.0"\n' > "$d/Cargo.toml"
  printf '{\n  "lib": 5\n}\n' > "$d/scripts/ci-suite-weights.json"
  echo '{}' > "$d/scripts/ci-suite-measured.json"
  local caps max
  caps="$(grep '^MAX_PER_SUITE = ' "$SRC/gen-test-suites.sh")" max="$(grep '^MAX_SUITES = ' "$SRC/gen-test-suites.sh")"
  python3 -I - "$SRC/gen-test-suites.sh" "$d/scripts/gen-test-suites.sh" "$caps" "$max" "$maxs" <<'PY'
import sys
src, dst, caps, mx, n = sys.argv[1:6]
s = open(src).read().replace(caps, 'MAX_PER_SUITE = {"core": 3, "sandbox": 3}').replace(mx, f"MAX_SUITES = {n}")
open(dst, "w").write(s)
PY
  cp "$SRC/ci-test-partition.sh" "$SRC/ci-refresh-weights.sh" "$d/scripts/"
  chmod +x "$d"/scripts/*.sh
  local i a
  for a in $(printf 'c%s ' $(echo a b c d e f g h i j k | cut -d' ' -f1-"$areas")); do
    for i in 1 2 3; do printf '#[test]\nfn t() {}\n' > "$d/tests/${a}_t$i.rs"; done
  done
  for i in 1 2 3; do printf '// Python%s marks this file sandbox-classified\n#[test]\nfn t() {}\n' "Kind" > "$d/tests/sa_t$i.rs"; done
  g "$d" init -q && g "$d" checkout -q -b main && settle "$d" base || { echo "scratch git failed" >&2; return 1; }
  (cd "$d" && scripts/gen-test-suites.sh >/dev/null) || { echo "scratch gen failed" >&2; return 1; }
  # a CI refresh: weights 4 s, measured 5 s (1.25x) for every generated suite
  python3 -I - "$d" <<'PY'
import json, sys, glob, os
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
  (cd "$d" && scripts/ci-test-partition.sh check >/dev/null 2>&1) || { echo "scratch baseline check failed" >&2; return 1; }
  settle "$d" measured || return 1
  echo "$d"
}
add_new_area() { # <dir> [prefix=cz]: three brand-new core files in a new area -> a suite made only of new files
  local d="$1" p="${2:-cz}" i
  for i in 1 2 3; do printf '#[test]\nfn t() {}\n' > "$d/tests/${p}_t$i.rs"; done
}
# pending <name> [core_areas] [prefix]: scratch + a new-only area, gen-run; echoes the dir (new suite = the last core suite)
pending() {
  local d
  d="$(scratch "$1" "${2:-1}")" || return 1
  add_new_area "$d" "${3:-cz}"
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/$1.gen" 2>&1) || { echo "pending gen failed: $(tail -3 "$WORK/$1.gen")" >&2; return 1; }
  echo "$d"
}
newest_core() { (cd "$1" && ls tests/suite_core_*.rs | sed 's#tests/##; s#\.rs$##' | tail -1); }
# branch_commit <dir>: a commit on the branch only (origin/main stays where it is)
branch_commit() { g "$1" add -A && g "$1" commit -q --allow-empty -m "${2:-branch}"; }
# advance_main <dir> <n>: n more commits on origin/main that the checked-out branch does NOT have (main moved on)
advance_main() {
  local d="$1" tip tree i
  tip="$(g "$d" rev-parse refs/remotes/origin/main)"; tree="$(g "$d" rev-parse "${tip}^{tree}")"
  for ((i = 0; i < $2; i++)); do tip="$(g "$d" commit-tree -p "$tip" -m "main $i" "$tree")"; done
  g "$d" update-ref refs/remotes/origin/main "$tip"
}
# drop_measured <dir> <suite>: the measured file as a branch has it before the seed exists (or after a rebase took main's side)
drop_measured() {
  local m="$1/scripts/ci-suite-measured.json"
  jq --arg s "$2" 'del(.[$s]) | if (._provisional // {}) | has($s) then del(._provisional[$s]) else . end | if ._provisional == {} then del(._provisional) else . end' "$m" > "$m.tmp" && mv "$m.tmp" "$m"
}

# ---- (a) the run-39913385513 fixture: suite_core_12 made of only new files
FIXTURE_LINE='ci-test-partition: unmeasured suite: suite_core_12 has a recorded weight in scripts/ci-suite-weights.json but no entry in scripts/ci-suite-measured.json'
d="$(pending a 11 cl)" || { bad "(a) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  new="$(newest_core "$d")"; m="$d/scripts/ci-suite-measured.json"; head="$(g "$d" rev-parse HEAD)"
  w="$(jget "$d/scripts/ci-suite-weights.json" "d['$new']")"
  want="$(python3 -I -c "print(round($w * 1.25, 2))")"
  # the tree that produced the fixture line: the same plan without the entry gen now writes
  cp "$m" "$WORK/a.m"; setj "$m" 'del(._provisional) | del(.suite_core_12)'
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/a.pre" 2>&1); rc_pre=$?
  cp "$WORK/a.m" "$m"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/a.check" 2>&1); rc=$?
  entry="$(jget "$m" "d.get('_provisional', {}).get('$new')")"
  shape="$(jq -c --arg s "$head" --argjson w "$want" '._provisional.suite_core_12 == {pending: true, derived_s: $w, since: $s} and .suite_core_12 == $w' "$m")"
  if [ "$new" = suite_core_12 ] && [ "$rc_pre" != 0 ] && grep -qF "$FIXTURE_LINE" "$WORK/a.pre" \
     && ! grep -qF "$FIXTURE_LINE" "$WORK/a.check" && [ "$shape" = true ] && [ "$rc" = 0 ] \
     && grep -qF "pending (derived) suite_core_12 since ${head:0:10}" "$WORK/a.check"; then
    ok "(a) fixture: without the entry check prints the observed line; gen now writes $entry and check rc=0 prints 'pending (derived) suite_core_12 since ${head:0:10}'"
  else
    bad "(a) fixture" "new=$new rc_pre=$rc_pre fixture_line_pre=$(grep -cF "$FIXTURE_LINE" "$WORK/a.pre") fixture_line_now=$(grep -cF "$FIXTURE_LINE" "$WORK/a.check") shape=$shape entry=$entry want_derived=$want head=$head check_rc=$rc :: $(grep -m2 -E 'unmeasured|pending|WARNING|FAILED' "$WORK/a.check" | tr '\n' ' ')"
  fi
fi

# ---- (b) a pending entry whose measured value is edited away from derived_s fails check; textual 2.50 still passes
d="$(pending b)" || { bad "(b) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  m="$d/scripts/ci-suite-measured.json"; cp "$m" "$WORK/b.m"
  setj "$m" '.suite_core_02 = 5.5'
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/b.edit" 2>&1); rc_edit=$?
  cp "$WORK/b.m" "$m"
  sed -i 's/"suite_core_02": 5.0/"suite_core_02": 5.00/' "$m"
  num_same="$(grep -c '"suite_core_02": 5.00' "$m")"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/b.same" 2>&1); rc_same=$?
  cp "$WORK/b.m" "$m"; setj "$m" '._provisional = {suite_core_02: "2026-10-23"}'
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/b.legacy" 2>&1); rc_leg=$?
  if [ "$rc_edit" != 0 ] && grep -q 'pending entry suite_core_02: measured value 5.5 != derived_s 5' "$WORK/b.edit" \
     && [ "$num_same" = 1 ] && [ "$rc_same" = 0 ] && grep -q 'pending (derived) suite_core_02' "$WORK/b.same" \
     && [ "$rc_leg" != 0 ] && grep -q 'provisional entry for suite_core_02 is not {pending, derived_s, since}' "$WORK/b.legacy"; then
    ok "(b) measured 5.5 != derived_s 5 fails check naming suite and both numbers; 5.00 vs 5.0 passes (numeric); a date-valued mark fails"
  else
    bad "(b) edited value" "rc_edit=$rc_edit num_same=$num_same rc_same=$rc_same rc_leg=$rc_leg :: $(grep -m1 'pending entry' "$WORK/b.edit") :: $(grep -m1 -E 'unmeasured|pending|provisional' "$WORK/b.same") :: $(grep -m1 -E 'provisional|WARNING' "$WORK/b.legacy")"
  fi
fi

# ---- (c) age from the base: 3 first-parent commits pass, 4 fail on the default branch; a PR ref only prints the count
d="$(pending c)" || { bad "(c) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  settle "$d" land            # the land commit counts as 1
  bump "$d" 2
  (cd "$d" && scripts/ci-test-partition.sh check --main >"$WORK/c.age3" 2>&1); rc3=$?
  bump "$d" 1
  (cd "$d" && scripts/ci-test-partition.sh check --main >"$WORK/c.age4" 2>&1); rc4=$?
  (cd "$d" && GITHUB_REF_NAME=main scripts/ci-test-partition.sh check >"$WORK/c.ref_main" 2>&1); rc_ref_main=$?
  (cd "$d" && GITHUB_REF_NAME=496/merge scripts/ci-test-partition.sh check >"$WORK/c.ref_pr" 2>&1); rc_ref_pr=$?
  orphan="$(g "$d" commit-tree -m orphan "$(g "$d" mktree </dev/null)")"
  m="$d/scripts/ci-suite-measured.json"; setj "$m" ".\"_provisional\".suite_core_02.since = \"$orphan\""
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/c.orphan" 2>&1); rc_orphan=$?
  if [ "$rc3" = 0 ] && grep -q 'pending (derived) suite_core_02 since .* (3 first-parent commits' "$WORK/c.age3" \
     && [ "$rc4" != 0 ] && grep -q 'suite_core_02 since .* is 4 first-parent commits behind HEAD (max 3)' "$WORK/c.age4" && grep -q 'scripts/ci-refresh-weights.sh' "$WORK/c.age4" \
     && [ "$rc_ref_main" != 0 ] && [ "$rc_ref_pr" = 0 ] && grep -q 'info only' "$WORK/c.ref_pr" \
     && [ "$rc_orphan" != 0 ] && grep -q 'is not an ancestor of HEAD' "$WORK/c.orphan"; then
    ok "(c) age 3 passes, age 4 fails with the refresh command (--main and GITHUB_REF_NAME=main); on GITHUB_REF_NAME=496/merge age 4 is info only rc=0; a since that is no ancestor fails"
  else
    bad "(c) age bound" "rc3=$rc3 rc4=$rc4 rc_ref_main=$rc_ref_main rc_ref_pr=$rc_ref_pr rc_orphan=$rc_orphan :: $(grep -m1 pending "$WORK/c.age3") :: $(grep -m1 pending "$WORK/c.age4")"
  fi
fi

# ---- (d) refresh removes the object when the log measures the suite; exits 1 naming it when the log omits it
d="$(pending d)" || { bad "(d) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  required="$(cd "$d" && scripts/ci-test-partition.sh list measured-required)"
  log_all="$WORK/d.all.log"; log_part="$WORK/d.part.log"
  : > "$log_all"; : > "$log_part"
  for k in $required; do
    echo "2026-10-23T01:00:00Z suite=$k measured=7.00s recorded=4s" >> "$log_all"
    [ "$k" = suite_core_02 ] || echo "2026-10-23T01:00:00Z suite=$k measured=7.00s recorded=4s" >> "$log_part"
  done
  cp "$d/scripts/ci-suite-measured.json" "$WORK/d.before.m"; cp "$d/scripts/ci-suite-weights.json" "$WORK/d.before.w"
  (cd "$d" && CI_REFRESH_ROOT="$d" CI_REFRESH_ALLOW_LOCAL=1 scripts/ci-refresh-weights.sh "$log_part") >"$WORK/d.part.out" 2>&1; rc_part=$?
  same=0; cmp -s "$WORK/d.before.m" "$d/scripts/ci-suite-measured.json" && cmp -s "$WORK/d.before.w" "$d/scripts/ci-suite-weights.json" && same=1
  (cd "$d" && CI_REFRESH_ROOT="$d" CI_REFRESH_ALLOW_LOCAL=1 scripts/ci-refresh-weights.sh "$log_all") >"$WORK/d.all.out" 2>&1; rc_all=$?
  m="$d/scripts/ci-suite-measured.json"
  has_prov="$(jget "$m" "'_provisional' in d")"; seeded="$(jget "$m" "d.get('suite_core_02')")"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/d.check" 2>&1); rc_chk=$?
  if [ "$rc_part" != 0 ] && grep -q 'suite_core_02' "$WORK/d.part.out" && [ "$same" = 1 ] \
     && [ "$rc_all" = 0 ] && [ "$has_prov" = false ] && [ "$seeded" = 7.0 ] && [ "$rc_chk" = 0 ] && ! grep -q 'pending (derived)' "$WORK/d.check"; then
    ok "(d) a log omitting suite_core_02 exits $rc_part naming it and writes nothing; a log measuring it removes the _provisional object (suite_core_02=7.0), check rc=0 without a pending line"
  else
    bad "(d) refresh" "rc_part=$rc_part same=$same rc_all=$rc_all has_prov=$has_prov seeded=$seeded rc_chk=$rc_chk :: $(tail -2 "$WORK/d.part.out" | tr '\n' ' ')"
  fi
fi

# ---- (e) the unmodified tree: --check ok, a plain run writes nothing (no drift), check ok
t="$WORK/e"; mkdir -p "$t"
cp -r "$HERE/tests" "$HERE/scripts" "$HERE/Cargo.toml" "$t/"
cp "$SRC/gen-test-suites.sh" "$SRC/ci-test-partition.sh" "$SRC/ci-refresh-weights.sh" "$t/scripts/"  # base scripts when SEED_SELFTEST_SCRIPTS is set
(cd "$t" && scripts/gen-test-suites.sh --check >"$WORK/e.check" 2>&1); rc_c=$?
(cd "$t" && find . -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/e.before")
(cd "$t" && scripts/gen-test-suites.sh >"$WORK/e.gen" 2>&1); rc_g=$?
(cd "$t" && find . -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/e.after")
# a date-valued mark committed on main would turn main red at merge: the tree's own _provisional must be empty or pending-shaped
dated="$(jq -r '(._provisional // {}) | [to_entries[] | select(.value | type != "object")] | length' "$HERE/scripts/ci-suite-measured.json")"
if [ "$rc_c" = 0 ] && grep -q 'gen-test-suites.sh --check: ok' "$WORK/e.check" && [ "$rc_g" = 0 ] \
   && grep -q 'already up to date' "$WORK/e.gen" && cmp -s "$WORK/e.before" "$WORK/e.after" && [ "$dated" = 0 ]; then
  ok "(e) unmodified tree: gen-test-suites.sh --check ok, plain run 'already up to date', 0 files changed, 0 date-valued marks"
else
  bad "(e) no drift" "check_rc=$rc_c gen_rc=$rc_g dated=$dated :: $(tail -2 "$WORK/e.check" | tr '\n' ' ') :: $(diff "$WORK/e.before" "$WORK/e.after" | head -3 | tr '\n' ' ')"
fi

# ---- (g) shallow clone: check deepens once and finds `since`; a since that is nowhere fails distinctly
d="$(pending g)" || { bad "(g) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  since="$(g "$d" rev-parse HEAD)"
  settle "$d" land; bump "$d" 2                      # age 3: since is 3 behind a depth-1 clone's only commit
  rm -rf "$WORK/g.clone"
  git clone -q --depth 1 "file://$d" "$WORK/g.clone" 2>"$WORK/g.clone.err" || bad "(g) clone" "$(cat "$WORK/g.clone.err")"
  have_before=0; g "$WORK/g.clone" cat-file -e "${since}^{commit}" 2>/dev/null && have_before=1
  (cd "$WORK/g.clone" && scripts/ci-test-partition.sh check --main >"$WORK/g.deep" 2>&1); rc_deep=$?
  setj "$WORK/g.clone/scripts/ci-suite-measured.json" '._provisional.suite_core_02.since = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"'
  (cd "$WORK/g.clone" && scripts/ci-test-partition.sh check --main >"$WORK/g.gone" 2>&1); rc_gone=$?
  if [ "$have_before" = 0 ] && [ "$rc_deep" = 0 ] && grep -q 'pending (derived) suite_core_02' "$WORK/g.deep" \
     && [ "$rc_gone" != 0 ] && grep -q 'since unreachable: fetch-depth too shallow or since not on main' "$WORK/g.gone"; then
    ok "(g) depth-1 clone lacked since (3 behind), check --main deepened and passed; a since not on main fails 'since unreachable: fetch-depth too shallow or since not on main'"
  else
    bad "(g) shallow clone" "have_before=$have_before rc_deep=$rc_deep rc_gone=$rc_gone :: $(grep -m2 -E 'pending|unreachable|FAILED' "$WORK/g.deep" | tr '\n' ' ') :: $(grep -m1 -E 'unreachable|pending' "$WORK/g.gone")"
  fi
fi

# ---- (h) a resized suite is NOT marked; a resized pending suite keeps `since` and re-derives derived_s; no-op regen is quiet
d="$(scratch h)" || { bad "(h) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  rm "$d/tests/ca_t3.rs"
  (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  m="$d/scripts/ci-suite-measured.json"
  r_prov="$(jget "$m" "'_provisional' in d")"; r_val="$(jget "$m" "d['suite_core_01']")"
  settle "$d" resized
  add_new_area "$d"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  since0="$(jget "$m" "d['_provisional']['suite_core_02']['since']")"
  settle "$d" land; bump "$d" 1
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/h.noop" 2>&1)
  rm "$d/tests/cz_t1.rs"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  p_after="$(jq -c '._provisional | keys' "$m")"
  keeps="$(jq -c --argjson s "$since0" '._provisional.suite_core_02 as $p | $p.pending == true and $p.since == $s and $p.derived_s == .suite_core_02' "$m")"
  (cd "$d" && scripts/ci-test-partition.sh check --main >"$WORK/h.check" 2>&1); rc_chk=$?
  if [ "$r_prov" = false ] && [ "$r_val" = 3.75 ] && grep -q 'already up to date' "$WORK/h.noop" \
     && [ "$p_after" = '["suite_core_02"]' ] && [ "$keeps" = true ] && [ "$rc_chk" = 0 ]; then
    ok "(h) resized suite_core_01 -> 3.75 s unmarked; no-op regen 'already up to date'; resized pending suite_core_02 keeps since and derived_s == its new value; check rc=0"
  else
    bad "(h) resize/no-op" "r_prov=$r_prov r_val=$r_val p_after=$p_after keeps=$keeps rc_chk=$rc_chk :: $(grep -m1 pending "$WORK/h.check")"
  fi
fi

# ---- (i) a suite that already exists with a weight but NO measured entry (a branch cut before #497 merged) is seeded pending
d="$(scratch i)" || { bad "(i) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)   # suite_core_02: file + weight + entry
  drop_measured "$d" suite_core_02; branch_commit "$d"                          # the branch before the seed existed: file + weight, no entry
  m="$d/scripts/ci-suite-measured.json"; base="$(g "$d" rev-parse refs/remotes/origin/main)"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/i.pre" 2>&1); rc_pre=$?
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/i.gen" 2>&1)
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/i.check" 2>&1); rc=$?
  w="$(jget "$d/scripts/ci-suite-weights.json" "d['suite_core_02']")"; want="$(python3 -I -c "print(round($w * 1.25, 2))")"
  shape="$(jq -c --arg s "$base" --argjson w "$want" '._provisional.suite_core_02 == {pending: true, derived_s: $w, since: $s} and .suite_core_02 == $w' "$m")"
  if [ "$rc_pre" != 0 ] && grep -q 'unmeasured suite: suite_core_02 has a recorded weight' "$WORK/i.pre" \
     && ! grep -q 'already up to date' "$WORK/i.gen" && [ "$shape" = true ] && [ "$rc" = 0 ] \
     && grep -qF "pending (derived) suite_core_02 since ${base:0:10}" "$WORK/i.check"; then
    ok "(i) pre-existing suite_core_02 (file + weight $w, no entry): check fails 'unmeasured suite: suite_core_02'; gen seeds {pending, derived_s $want = w x 1.25, since = merge-base}; check rc=0"
  else
    bad "(i) pre-existing unmeasured suite" "rc_pre=$rc_pre shape=$shape want=$want check_rc=$rc :: gen: $(tail -1 "$WORK/i.gen") :: pre: $(grep -m1 -E 'unmeasured' "$WORK/i.pre") :: now: $(grep -m2 -E 'unmeasured|pending|FAILED' "$WORK/i.check" | tr '\n' ' ')"
  fi
fi

# ---- (i-moved) a pre-existing unmeasured suite with a member that came from a MEASURED suite takes the ratio path (not marked)
d="$(scratch imoved)" || { bad "(i-moved) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  # ca_t3 (suite_core_01, measured 5.0 = 1.25 x 4) becomes sandbox-classified: it joins suite_sandbox_01, whose entry the branch lacks
  printf '// Python%s marks this file sandbox-classified\n#[test]\nfn t() {}\n' "Kind" > "$d/tests/ca_t3.rs"
  rm -f "$d/tests/sa_t3.rs"
  # the old plan, as the branch has it: suite_sandbox_01 = {sa_t1..3} unmeasured; suite_core_01 = {ca_t1..3} measured
  drop_measured "$d" suite_sandbox_01
  branch_commit "$d"
  m="$d/scripts/ci-suite-measured.json"
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/imoved.gen" 2>&1)
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/imoved.check" 2>&1); rc=$?
  sb="$(grep -l 'ca_t3' "$d"/tests/suite_sandbox_*.rs | head -1 | sed 's#.*/##; s#\.rs$##')"
  w="$(jget "$d/scripts/ci-suite-weights.json" "d.get('$sb')")"; val="$(jget "$m" "d.get('$sb')")"
  mark="$(jget "$m" "'$sb' in d.get('_provisional', {})")"
  if [ "$sb" = suite_sandbox_01 ] && [ "$val" = "$(python3 -I -c "print(round($w * 1.25, 2))")" ] && [ "$mark" = false ] && [ "$rc" = 0 ] \
     && ! grep -q 'already up to date' "$WORK/imoved.gen"; then
    ok "(i-moved) pre-existing $sb (no entry; ca_t3 moved in from measured suite_core_01) seeded $val = w $w x 1.25 by the ratio path, unmarked; check rc=0"
  else
    bad "(i-moved) ratio path" "sb=$sb w=$w val=$val mark=$mark check_rc=$rc :: gen: $(tail -2 "$WORK/imoved.gen" | tr '\n' ' ') :: $(grep -m2 -E 'unmeasured|FAILED' "$WORK/imoved.check" | tr '\n' ' ')"
  fi
fi

# ---- (ii) the rebase path: main's side of the measured file lost the branch's entry; the age never runs to main's tip
d="$(scratch ii)" || { bad "(ii) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1); branch_commit "$d"   # the branch: suite_core_02 + weight + entry
  advance_main "$d" 5                                                                              # main moves on without it
  g "$d" rebase -q refs/remotes/origin/main >/dev/null 2>&1 || bad "(ii) rebase" "failed"
  g "$d" checkout -q refs/remotes/origin/main -- scripts/ci-suite-measured.json                       # the land rebase resolves the measured file to main's side
  branch_commit "$d" "resolve measured to main's side"
  m="$d/scripts/ci-suite-measured.json"; tip="$(g "$d" rev-parse refs/remotes/origin/main)"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/ii.pre" 2>&1); rc_pre=$?
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/ii.gen" 2>&1)
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/ii.check" 2>&1); rc=$?
  w="$(jget "$d/scripts/ci-suite-weights.json" "d['suite_core_02']")"; want="$(python3 -I -c "print(round($w * 1.25, 2))")"
  shape="$(jq -c --arg s "$tip" --argjson w "$want" '._provisional.suite_core_02 == {pending: true, derived_s: $w, since: $s} and .suite_core_02 == $w' "$m")"
  # then: 4 own commits on the branch and main 5 further on; HEAD..tip is nowhere near the seed's age
  for i in 1 2 3 4; do g "$d" commit -q --allow-empty -m "own $i"; done; advance_main "$d" 5
  (cd "$d" && scripts/ci-test-partition.sh check --main >"$WORK/ii.age" 2>&1); rc_age=$?
  if [ "$rc_pre" != 0 ] && grep -q 'unmeasured suite: suite_core_02 has a recorded weight' "$WORK/ii.pre" \
     && ! grep -q 'already up to date' "$WORK/ii.gen" && [ "$shape" = true ] && [ "$rc" = 0 ] \
     && [ "$rc_age" = 0 ] && grep -q 'pending (derived) suite_core_02 since .* (0 first-parent commits behind HEAD' "$WORK/ii.age"; then
    ok "(ii) rebase path: main's measured file lacked suite_core_02 (check fails unmeasured); gen seeds {pending, derived_s $want, since = merge-base}; after 4 own commits + main 5 further 'check --main' rc=0 at age 0"
  else
    bad "(ii) rebase path" "rc_pre=$rc_pre shape=$shape check_rc=$rc age_rc=$rc_age :: gen: $(tail -1 "$WORK/ii.gen") :: pre: $(grep -m1 unmeasured "$WORK/ii.pre") :: age: $(grep -m2 -E 'pending|FAILED' "$WORK/ii.age" | tr '\n' ' ')"
  fi
fi

# ---- (iii) a pre-existing suite WITH an entry is untouched; (iii-stale) a stale _provisional mark for a suite with no entry is ignored
d="$(scratch iii)" || { bad "(iii) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d" cz; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1); settle "$d" "land cz"    # suite_core_02 pending, since = the measured commit
  old_since="$(jq -r '._provisional.suite_core_02.since' "$d/scripts/ci-suite-measured.json")"
  add_new_area "$d" cz2; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)                        # suite_core_03
  m="$d/scripts/ci-suite-measured.json"
  drop_measured "$d" suite_core_03
  jq --arg s "$old_since" '._provisional.suite_core_03 = {pending: true, derived_s: 99, since: $s}' "$m" > "$m.tmp" && mv "$m.tmp" "$m"   # a stale mark, no value
  branch_commit "$d"
  cur="$(g "$d" rev-parse refs/remotes/origin/main)"
  before="$(jq -c '[.suite_core_01, .suite_sandbox_01, .suite_core_02, ._provisional.suite_core_02]' "$m")"
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/iii.gen" 2>&1)
  after="$(jq -c '[.suite_core_01, .suite_sandbox_01, .suite_core_02, ._provisional.suite_core_02]' "$m")"
  keys="$(jq -c '._provisional | keys' "$m")"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/iii.check" 2>&1); rc=$?
  stale="$(jq -c --arg s "$cur" '._provisional.suite_core_03 as $p | $p.since == $s and $p.derived_s == .suite_core_03 and $p.derived_s != 99' "$m")"
  if [ "$before" = "$after" ] && [ "$keys" = '["suite_core_02","suite_core_03"]' ] && [ "$old_since" != "$cur" ] && [ "$rc" = 0 ]; then
    ok "(iii) suite_core_01, suite_sandbox_01 and pending suite_core_02 byte-identical across the regen ($after); _provisional keys $keys (no new key for them); check rc=0"
  else
    bad "(iii) untouched" "before=$before after=$after keys=$keys old_since=${old_since:0:10} cur=${cur:0:10} check_rc=$rc :: $(tail -1 "$WORK/iii.gen")"
  fi
  if [ "$stale" = true ]; then
    ok "(iii-stale) stale _provisional.suite_core_03 {derived_s 99, since ${old_since:0:10}} with no measured entry ignored: fresh seed, since = merge-base ${cur:0:10}"
  else
    bad "(iii-stale) stale mark" "stale=$stale :: $(jq -c '._provisional.suite_core_03, .suite_core_03' "$m")"
  fi
fi

# ---- (iv) idempotency: a second gen run after the seed prints "already up to date" and writes nothing
d="$(scratch iv)" || { bad "(iv) setup" "$d"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1); drop_measured "$d" suite_core_02; branch_commit "$d"
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/iv.first" 2>&1)
  (cd "$d" && find . -path ./.git -prune -o -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/iv.before")
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/iv.second" 2>&1); rc2=$?
  (cd "$d" && find . -path ./.git -prune -o -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/iv.after")
  (cd "$d" && scripts/gen-test-suites.sh --check >"$WORK/iv.chk" 2>&1); rc_c=$?
  if ! grep -q 'already up to date' "$WORK/iv.first" && [ "$rc2" = 0 ] && grep -q 'already up to date' "$WORK/iv.second" \
     && cmp -s "$WORK/iv.before" "$WORK/iv.after" && [ "$rc_c" = 0 ]; then
    ok "(iv) first gen seeds, the second prints 'already up to date' with 0 files changed, gen --check ok"
  else
    bad "(iv) idempotency" "rc2=$rc2 check_rc=$rc_c :: first: $(tail -1 "$WORK/iv.first") :: second: $(tail -1 "$WORK/iv.second") :: $(diff "$WORK/iv.before" "$WORK/iv.after" | head -3 | tr '\n' ' ')"
  fi
fi

echo "ci-suite-seed-selftest: $passes passed, $fails failed (scripts from $SRC)"
[ "$fails" -eq 0 ]
