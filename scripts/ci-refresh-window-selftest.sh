#!/usr/bin/env bash
# ci-refresh-window-selftest.sh -- proves the weights refresh records every suite at its MAX over a window of
# green main runs, not at whatever the last run happened to measure, and that a measurement only counts for the
# member set CI actually ran.
#
# Main CI run 45038989292 (head 74263ea8, all tests passed) went red on core shard 5/5: run wall 909 s > 900 s.
# The refresh before it (#601) took ONE run, 45034667949, where suite_core_12 measured 326.72 s; the green run
# before that, 44757115005, measured it at 600.56 s (its nested `cargo test` in tlint_ac06 retries a strict run up
# to three times, so the suite swings 270..610 s across main runs). Recorded at 327, suite_core_12 was packed with
# suite_core_11 (305) for a predicted 632 s, and ran 307.23 + 599.65 s. ci-refresh-weights.sh already takes the MAX
# over its inputs; the auto wrapper fed it one run, and the loop guard (skip=self) discards every refresh commit's
# own run, so half of main's green measurements were never read.
#
# Window (ci-refresh-weights-auto.sh): the source run plus every green push run of the same workflow on the same
# branch created from the PREVIOUS refresh's source run (named by the newest `ci: refresh suite weights from run
# <id>` subject on HEAD's first-parent history) through the source run. No prior refresh subject: the source alone.
# Membership (ci-refresh-weights.sh): a run id's measurement of a suite counts only when that suite's member files at
# the run's head commit are a subset of its members now; a run whose head commit is not in the checkout fails closed.
#
# Every case runs in a `mktemp -d` scratch with a stub `gh` on PATH (no network, no cargo). The measured seconds are
# the verbatim `suite=<name> measured=<s>s` readouts of CI runs 44757115005, 45034667949 and 45038989292 (core and
# sandbox shard annotations). One PASS/FAIL line per case; exit 1 on any FAIL.
#   (w1) a refresh of 45034667949 above a `from run 44757115005` refresh commit passes both green runs in the window
#        (a later run and a failed run the list returns are not passed); the list query names branch, event, status
#   (w2) no refresh subject in history: the source run alone
#   (w3) the run list cannot be read: non-zero exit, no result line, the refresh never runs
#   (w4) the previous refresh named a RED run (45038989292, absent from the green-only list): it is still measured
#   (m1) two run ids at the same membership: each suite is recorded at its MAX (suite_core_12 600.56 -> 601)
#   (m2) a run whose suite held a file since moved out of it is not counted for that suite (and says so)
#   (m3) a run whose suite held a SUBSET of today's files is counted
#   (m4) a run whose head commit is not in the checkout: exit 1, nothing written
#   (s1) scheduling invariant on the real numbers: weights from 45034667949 alone put suite_core_11 + suite_core_12 in
#        one core shard that run 45038989292's measurements take over the core fail line (the 909 s class); weights
#        from the window keep every core shard of run 45038989292 at or under it
# Env: WINDOW_SELFTEST_SCRIPTS=<dir> runs the scripts from <dir> instead of this checkout's scripts/ (red-at-base: point
#   it at 74263ea8's scripts and (w1), (w3), (w4), (m2) and (m4) must FAIL).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${WINDOW_SELFTEST_SCRIPTS:-$HERE/scripts}"
WORK="$(mktemp -d)"
case "$WORK" in /tmp/*|"${TMPDIR:-/nonexistent}"/*|/var/tmp/*|"$HOME"/.cache/tmp/*) ;; *) echo "selftest: refusing odd scratch root $WORK" >&2; exit 2 ;; esac
trap 'rm -rf "$WORK"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset CI_REFRESH_ALLOW_LOCAL CI_REFRESH_REPO CI_SUITE_WEIGHTS_FILE CI_SUITE_MEASURED_FILE
fails=0 passes=0
ok()  { passes=$((passes + 1)); echo "PASS $1"; }
bad() { fails=$((fails + 1)); echo "FAIL $1${2:+: $2}"; }
g()   { local d="$1"; shift; git -C "$d" -c core.hooksPath=/dev/null "$@"; }

# ---- the real readouts (core + sandbox shard annotations of each run) ------------------------------------------
# 44757115005 (dfc679e7, success, 2026-11-20T14:49:10Z): the slow suite_core_12
R_SLOW='lib 6.21|suite_cargo_core_01 0.56|suite_contracts_core_01 681.21|suite_contracts_core_02 210.36|suite_contracts_sandbox_01 47.46|suite_core_01 1.25|suite_core_02 0.80|suite_core_03 1.04|suite_core_04 0.01|suite_core_05 0.93|suite_core_06 8.01|suite_core_07 0.80|suite_core_08 0.96|suite_core_09 313.67|suite_core_10 267.02|suite_core_11 187.51|suite_core_12 600.56|suite_core_13 138.86|suite_env_core_01 266.74|suite_env_sandbox_01 26.06|suite_sandbox_01 1.27|suite_sandbox_02 99.60|suite_sandbox_03 120.05|suite_sandbox_04 106.07'
# 45034667949 (9ae68a23, success, 2026-11-22T14:46:51Z): the fast suite_core_12 the #601 refresh recorded
R_FAST='lib 6.13|suite_cargo_core_01 0.71|suite_contracts_core_01 646.85|suite_contracts_core_02 367.30|suite_contracts_sandbox_01 51.46|suite_core_01 1.11|suite_core_02 0.80|suite_core_03 1.33|suite_core_04 0.01|suite_core_05 1.42|suite_core_06 9.31|suite_core_07 1.16|suite_core_08 1.45|suite_core_09 348.92|suite_core_10 267.83|suite_core_11 304.38|suite_core_12 326.72|suite_core_13 172.22|suite_env_core_01 386.95|suite_env_sandbox_01 26.65|suite_sandbox_01 1.24|suite_sandbox_02 129.89|suite_sandbox_03 135.53|suite_sandbox_04 111.99'
# 45038989292 (74263ea8, failure: core shard 5/5 run wall 909 s, every test passed, 2026-11-22T15:33:38Z)
R_NEXT='lib 7.20|suite_cargo_core_01 0.71|suite_contracts_core_01 698.38|suite_contracts_core_02 366.95|suite_contracts_sandbox_01 54.77|suite_core_01 1.11|suite_core_02 0.80|suite_core_03 1.37|suite_core_04 0.01|suite_core_05 1.43|suite_core_06 8.60|suite_core_07 1.17|suite_core_08 1.47|suite_core_09 343.62|suite_core_10 209.61|suite_core_11 307.23|suite_core_12 599.65|suite_core_13 205.71|suite_env_core_01 383.01|suite_env_sandbox_01 28.31|suite_sandbox_01 1.47|suite_sandbox_02 134.12|suite_sandbox_03 115.99|suite_sandbox_04 107.58'
pairs() { tr '|' '\n' <<<"$1"; }
readout() { pairs "$1" | while read -r n s; do echo "suite=$n measured=${s}s recorded=1s"; done; }

# ---- stub gh: `gh api [--paginate] <path>` served from $GH_FIX; every call appended to $GH_CALLS -----------------
mkdir -p "$WORK/bin"
cat > "$WORK/bin/gh" <<'SH'
#!/usr/bin/env bash
echo "gh $*" >> "$GH_CALLS"
path=""
for a in "$@"; do case "$a" in api|-*) ;; *) path="$a" ;; esac; done
if [ -n "${GH_STUB_FAIL:-}" ]; then case "$path" in *"$GH_STUB_FAIL"*) echo "stub: HTTP 502" >&2; exit 1 ;; esac; fi
case "$path" in
  */actions/workflows/*/runs*) cat "$GH_FIX/list.json" ;;
  */actions/runs/*/logs) id="${path%/logs}"; cat "$GH_FIX/${id##*/}.zip" ;;
  */actions/runs/*) cat "$GH_FIX/run-${path##*/}.json" ;;
  *) echo "stub: no route for $path" >&2; exit 1 ;;
esac
SH
chmod +x "$WORK/bin/gh"
export PATH="$WORK/bin:$PATH" GH_CALLS="$WORK/gh.calls"

# fix_run <fixdir> <id> <sha> <created_at> [<readout>]: run metadata (+ a log ZIP when a readout is given)
fix_run() {
  local f="$1" id="$2" sha="$3" at="$4" ro="${5:-}"
  mkdir -p "$f"
  printf '{"id": %s, "workflow_id": 349050476, "head_branch": "main", "event": "push", "head_sha": "%s", "created_at": "%s"}\n' \
    "$id" "$sha" "$at" > "$f/run-$id.json"
  if [ -n "$ro" ]; then
    readout "$ro" > "$f/$id.txt"
    python3 -I -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1], "w"); z.write(sys.argv[2], "5_core shard.txt"); z.close()' "$f/$id.zip" "$f/$id.txt"
  fi
}

# ---- (w) the window the auto wrapper hands to ci-refresh-weights.sh ---------------------------------------------
W="$WORK/w"; WB="$WORK/wbin"; WF="$WORK/wfix"
mkdir -p "$W/scripts" "$W/tests" "$WB"
cp "$SRC/ci-refresh-weights-auto.sh" "$W/scripts/"
for s in ci-refresh-weights.sh gen-test-suites.sh ci-test-partition.sh; do
  printf '#!/usr/bin/env bash\necho "%s $*" >> "%s"\nexit 0\n' "$s" "$WORK/inner.calls" > "$WB/$s"; chmod +x "$WB/$s"
done
g "$W" init -q && g "$W" add -A && g "$W" commit -q -m "loop: base"
g "$W" commit -q --allow-empty -m "ci: refresh suite weights from run 44757115005 (#599)"
g "$W" commit -q --allow-empty -m "hotfix(deps): url 2.5.8 -> 2.5.9 (#600)"
fix_run "$WF" 45034667949 9ae68a23bc19cdf239734df9a9dd8df85a0495be 2026-11-22T14:46:51Z
fix_run "$WF" 44757115005 dfc679e7dc7801d8fd9cd736c312ce60877168bc 2026-11-20T14:49:10Z
# what a list query could return: the two green runs in range, a failed run in range, a green run after the source
cat > "$WF/list.json" <<'JSON'
{"total_count": 4, "workflow_runs": [
 {"id": 45038989292, "head_branch": "main", "event": "push", "conclusion": "success", "created_at": "2026-11-22T15:33:38Z"},
 {"id": 45034667949, "head_branch": "main", "event": "push", "conclusion": "success", "created_at": "2026-11-22T14:46:51Z"},
 {"id": 44765049352, "head_branch": "main", "event": "push", "conclusion": "failure", "created_at": "2026-11-20T15:36:57Z"},
 {"id": 44757115005, "head_branch": "main", "event": "push", "conclusion": "success", "created_at": "2026-11-20T14:49:10Z"}]}
JSON
auto() { GH_FIX="$WF" CI_REFRESH_AUTO_BIN_DIR="$WB" CI_REFRESH_AUTO_ROOT="$W" "$W/scripts/ci-refresh-weights-auto.sh" "$@"; }
refresh_args() { sed -n 's/^ci-refresh-weights.sh //p' "$WORK/inner.calls"; }

: > "$WORK/inner.calls"; : > "$GH_CALLS"
out="$(auto 45034667949 2>"$WORK/w1.err")"; rc=$?
got="$(refresh_args | tr ' ' '\n' | sort -n | tr '\n' ' ')"
if [ "$rc" = 0 ] && [ "$out" = "changed=0 skip=none" ] && [ "$got" = "44757115005 45034667949 " ] \
   && grep -q 'actions/workflows/349050476/runs?' "$GH_CALLS" && grep 'workflows/349050476/runs' "$GH_CALLS" | grep -q 'branch=main' \
   && grep 'workflows/349050476/runs' "$GH_CALLS" | grep -q 'event=push' && grep 'workflows/349050476/runs' "$GH_CALLS" | grep -q 'status=success'; then
  ok "(w1) window = green push runs from the previous refresh's source (44757115005) through the source (45034667949)"
else
  bad "(w1) window" "rc=$rc out='$out' refresh args='$got' gh calls: $(tr '\n' ';' < "$GH_CALLS") err: $(tr '\n' ' ' < "$WORK/w1.err")"
fi

W2="$WORK/w2"; mkdir -p "$W2/scripts"; cp "$SRC/ci-refresh-weights-auto.sh" "$W2/scripts/"
g "$W2" init -q && g "$W2" add -A && g "$W2" commit -q -m "loop: base" && g "$W2" commit -q --allow-empty -m "ci: refresh test-partition weights (by hand)"
: > "$WORK/inner.calls"
out="$(GH_FIX="$WF" CI_REFRESH_AUTO_BIN_DIR="$WB" CI_REFRESH_AUTO_ROOT="$W2" "$W2/scripts/ci-refresh-weights-auto.sh" 45034667949 2>/dev/null)"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = "changed=0 skip=none" ] && [ "$(refresh_args)" = "45034667949" ]; then
  ok "(w2) no refresh subject in history: the source run alone"
else
  bad "(w2) no prior refresh" "rc=$rc out='$out' refresh args='$(refresh_args)'"
fi

: > "$WORK/inner.calls"
out="$(GH_STUB_FAIL="/runs?" auto 45034667949 2>/dev/null)"; rc=$?
if [ "$rc" != 0 ] && [ -z "$out" ] && [ ! -s "$WORK/inner.calls" ]; then
  ok "(w3) an unreadable run list fails closed: rc $rc, no result line, no refresh"
else
  bad "(w3) fail closed" "rc=$rc out='$out' inner calls: $(tr '\n' ';' < "$WORK/inner.calls")"
fi

# (w4) a hand refresh named the red run 45038989292; the next refresh (source: a later green run) must still read it
W4="$WORK/w4"; WF4="$WORK/wfix4"; mkdir -p "$W4/scripts"; cp "$SRC/ci-refresh-weights-auto.sh" "$W4/scripts/"
g "$W4" init -q && g "$W4" add -A && g "$W4" commit -q -m "loop: base"
g "$W4" commit -q --allow-empty -m "ci: refresh suite weights from run 45038989292 over the window from run 45034667949"
g "$W4" commit -q --allow-empty -m "loop: next feature"
fix_run "$WF4" 45038989292 74263ea80621e0a978d0d935f43e7ac8f5cf3d97 2026-11-22T15:33:38Z
fix_run "$WF4" 45099999999 0000000000000000000000000000000000000001 2026-11-24T08:00:00Z
cat > "$WF4/list.json" <<'JSON'
{"total_count": 1, "workflow_runs": [
 {"id": 45099999999, "head_branch": "main", "event": "push", "conclusion": "success", "created_at": "2026-11-24T08:00:00Z"}]}
JSON
: > "$WORK/inner.calls"
out="$(GH_FIX="$WF4" CI_REFRESH_AUTO_BIN_DIR="$WB" CI_REFRESH_AUTO_ROOT="$W4" "$W4/scripts/ci-refresh-weights-auto.sh" 45099999999 2>/dev/null)"; rc=$?
got="$(refresh_args | tr ' ' '\n' | sed '/^$/d' | sort -n | tr '\n' ' ')"
if [ "$rc" = 0 ] && [ "$out" = "changed=0 skip=none" ] && [ "$got" = "45038989292 45099999999 " ]; then
  ok "(w4) a red previous source (45038989292) is still in the window"
else
  bad "(w4) red previous source" "rc=$rc out='$out' refresh args='$got'"
fi

# ---- (m) ci-refresh-weights.sh over run ids: MAX, and the membership rule -----------------------------------------
# scratch <dir>: a git repo with the two scripts, one empty tests/suite_<name>.rs per readout suite, weights and
# measured at the fast run's values
scratch() {
  local d="$1" n s
  mkdir -p "$d/scripts" "$d/tests"
  cp "$SRC/ci-refresh-weights.sh" "$SRC/ci-test-partition.sh" "$d/scripts/"
  pairs "$R_FAST" | while read -r n s; do [ "$n" = lib ] || echo "// scratch suite" > "$d/tests/$n.rs"; done
  pairs "$R_FAST" | python3 -I -c '
import json, math, sys
rows = [l.split() for l in sys.stdin if l.strip()]
json.dump({n: max(1, math.ceil(float(s))) for n, s in rows}, open(sys.argv[1], "w"), indent=2)
json.dump({n: float(s) for n, s in rows}, open(sys.argv[2], "w"), indent=2)' "$d/scripts/ci-suite-weights.json" "$d/scripts/ci-suite-measured.json"
  g "$d" init -q && g "$d" add -A && g "$d" commit -q -m "loop: base"
}
weight() { python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1/scripts/ci-suite-weights.json" "$2"; }
refresh() { local d="$1"; shift; (cd "$d" && GH_FIX="$d.fix" CI_REFRESH_ROOT="$d" scripts/ci-refresh-weights.sh "$@"); }

M1="$WORK/m1"; scratch "$M1"; head1="$(g "$M1" rev-parse HEAD)"
fix_run "$M1.fix" 44757115005 "$head1" 2026-11-20T14:49:10Z "$R_SLOW"
fix_run "$M1.fix" 45034667949 "$head1" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$M1" 44757115005 45034667949 >"$WORK/m1.out" 2>&1; rc=$?
if [ "$rc" = 0 ] && [ "$(weight "$M1" suite_core_12)" = 601 ] && [ "$(weight "$M1" suite_core_11)" = 305 ] && [ "$(weight "$M1" suite_contracts_core_01)" = 682 ]; then
  ok "(m1) two runs at one membership: each suite recorded at its max (suite_core_12 601, suite_core_11 305)"
else
  bad "(m1) max over runs" "rc=$rc core_12=$(weight "$M1" suite_core_12) $(tr '\n' ' ' < "$WORK/m1.out")"
fi

# membership history: at commit X suite_core_12 ran a.rs + b.rs; b.rs has since moved to suite_core_11 (HEAD)
M2="$WORK/m2"; scratch "$M2"
echo '// member' > "$M2/tests/a.rs"; echo '// member' > "$M2/tests/b.rs"; echo '// member' > "$M2/tests/c.rs"
printf 'mod common;\n#[path = "support/x.rs"]\nmod x;\n#[path = "a.rs"]\nmod a;\n#[path = "b.rs"]\nmod b;\n' > "$M2/tests/suite_core_12.rs"
printf '#[path = "c.rs"]\nmod c;\n' > "$M2/tests/suite_core_11.rs"
g "$M2" add -A && g "$M2" commit -q -m "loop: X"; shaX="$(g "$M2" rev-parse HEAD)"
printf 'mod common;\n#[path = "support/x.rs"]\nmod x;\n#[path = "a.rs"]\nmod a;\n' > "$M2/tests/suite_core_12.rs"
printf '#[path = "b.rs"]\nmod b;\n#[path = "c.rs"]\nmod c;\n' > "$M2/tests/suite_core_11.rs"
g "$M2" add -A && g "$M2" commit -q -m "loop: move b.rs"; shaH="$(g "$M2" rev-parse HEAD)"
fix_run "$M2.fix" 44757115005 "$shaX" 2026-11-20T14:49:10Z "$R_SLOW"
fix_run "$M2.fix" 45034667949 "$shaH" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$M2" 44757115005 45034667949 >"$WORK/m2.out" 2>&1; rc=$?
if [ "$rc" = 0 ] && [ "$(weight "$M2" suite_core_12)" = 327 ] && grep -q 'suite_core_12: run 44757115005 .*b\.rs.* not counted' "$WORK/m2.out" \
   && [ "$(weight "$M2" suite_core_11)" = 305 ] && [ "$(weight "$M2" suite_core_09)" = 349 ]; then
  ok "(m2) a run whose suite held a file since moved out is not counted for it (suite_core_12 327, not 601); a subset suite still is"
else
  bad "(m2) membership" "rc=$rc core_12=$(weight "$M2" suite_core_12) core_11=$(weight "$M2" suite_core_11) $(tr '\n' ' ' < "$WORK/m2.out")"
fi

# (m3) the old run held a SUBSET of today's members: counted
M3="$WORK/m3"; scratch "$M3"
echo '// member' > "$M3/tests/a.rs"; echo '// member' > "$M3/tests/b.rs"
printf '#[path = "a.rs"]\nmod a;\n' > "$M3/tests/suite_core_12.rs"; touch "$M3/tests/b.rs"
g "$M3" add -A && g "$M3" commit -q -m "loop: X"; shaX="$(g "$M3" rev-parse HEAD)"
printf '#[path = "a.rs"]\nmod a;\n#[path = "b.rs"]\nmod b;\n' > "$M3/tests/suite_core_12.rs"
g "$M3" add -A && g "$M3" commit -q -m "loop: add b.rs"; shaH="$(g "$M3" rev-parse HEAD)"
fix_run "$M3.fix" 44757115005 "$shaX" 2026-11-20T14:49:10Z "$R_SLOW"
fix_run "$M3.fix" 45034667949 "$shaH" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$M3" 44757115005 45034667949 >"$WORK/m3.out" 2>&1; rc=$?
if [ "$rc" = 0 ] && [ "$(weight "$M3" suite_core_12)" = 601 ]; then
  ok "(m3) a run that ran a subset of today's members is counted (suite_core_12 601)"
else
  bad "(m3) subset" "rc=$rc core_12=$(weight "$M3" suite_core_12) $(tr '\n' ' ' < "$WORK/m3.out")"
fi

# (m4) the old run's head commit is not in the checkout: fail closed, nothing written
M4="$WORK/m4"; scratch "$M4"; head4="$(g "$M4" rev-parse HEAD)"
before="$(cat "$M4/scripts/ci-suite-weights.json" "$M4/scripts/ci-suite-measured.json" | sha256sum)"
fix_run "$M4.fix" 44757115005 dfc679e7dc7801d8fd9cd736c312ce60877168bc 2026-11-20T14:49:10Z "$R_SLOW"
fix_run "$M4.fix" 45034667949 "$head4" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$M4" 44757115005 45034667949 >"$WORK/m4.out" 2>&1; rc=$?
after="$(cat "$M4/scripts/ci-suite-weights.json" "$M4/scripts/ci-suite-measured.json" | sha256sum)"
if [ "$rc" != 0 ] && [ "$before" = "$after" ] && grep -q 'dfc679e7dc' "$WORK/m4.out"; then
  ok "(m4) a run whose head commit is not in the checkout fails closed (rc $rc, nothing written)"
else
  bad "(m4) unknown head commit" "rc=$rc unchanged=$([ "$before" = "$after" ] && echo yes || echo no) $(tr '\n' ' ' < "$WORK/m4.out")"
fi

# ---- (s1) the scheduling invariant on the real numbers -------------------------------------------------------------
read -r _ core_fail < <("$SRC/ci-test-partition.sh" limits core)
core_shards="$("$SRC/ci-test-partition.sh" shards core)"
# worst_shard <scratch>: the heaviest core shard of its plan, summed from run 45038989292's measurements
worst_shard() {
  local d="$1" n
  for ((n = 1; n <= core_shards; n++)); do
    "$d/scripts/ci-test-partition.sh" core-shard "$n" "$core_shards" | tr ' ' '\n' | sed -n 's/^suite_/suite_/p' \
      | while read -r s; do pairs "$R_NEXT" | awk -v s="$s" '$1 == s { print $2 }'; done | awk -v n="$n" '{ t += $1 } END { printf "%d %.2f\n", n, t }'
  done | sort -k2,2nr | head -1
}
S0="$WORK/s0"; scratch "$S0"; h="$(g "$S0" rev-parse HEAD)"
fix_run "$S0.fix" 45034667949 "$h" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$S0" 45034667949 >/dev/null 2>&1; rc0=$?
read -r s0n s0t < <(worst_shard "$S0")
s0names="$("$S0/scripts/ci-test-partition.sh" core-shard "$s0n" "$core_shards")"
S1="$WORK/s1"; scratch "$S1"; h="$(g "$S1" rev-parse HEAD)"
fix_run "$S1.fix" 44757115005 "$h" 2026-11-20T14:49:10Z "$R_SLOW"
fix_run "$S1.fix" 45034667949 "$h" 2026-11-22T14:46:51Z "$R_FAST"
refresh "$S1" 44757115005 45034667949 >/dev/null 2>&1; rc1=$?
read -r s1n s1t < <(worst_shard "$S1")
if [ "$rc0" = 0 ] && [ "$rc1" = 0 ] && awk -v t="$s0t" -v l="$core_fail" 'BEGIN { exit !(t > l) }' \
   && [[ "$s0names" == *suite_core_11* ]] && [[ "$s0names" == *suite_core_12* ]] \
   && awk -v t="$s1t" -v l="$core_fail" 'BEGIN { exit !(t <= l) }'; then
  ok "(s1) one fast run packs suite_core_11+12 into shard $s0n: ${s0t}s of run 45038989292 > ${core_fail}s; the window's heaviest is ${s1t}s <= ${core_fail}s"
else
  bad "(s1) scheduling invariant" "rc0=$rc0 rc1=$rc1 single-run worst shard $s0n ${s0t}s [$s0names]; window worst shard $s1n ${s1t}s; fail line ${core_fail}s"
fi

echo "ci-refresh-window-selftest: $passes passed, $fails failed (scripts from $SRC)"
[ "$fails" -eq 0 ]
