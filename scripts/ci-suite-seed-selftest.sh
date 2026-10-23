#!/usr/bin/env bash
# ci-suite-seed-selftest.sh -- proves a suite created from ONLY new test files gets a provisional
# measured entry, so `ci-test-partition.sh check` passes before any shard can be measured.
#
# PR 495 / CI run 39913385513: gen-test-suites.sh seeded a created suite's measured entry only from
# member files that came from OLD suites, so suite_core_12 (all new files) had a recorded weight and no
# measured entry, and the completeness guard of `check` failed every core, sandbox and regen job
# before any shard could print the `suite=<name> measured=` lines scripts/ci-refresh-weights.sh reads.
#
# Every test runs in a `mktemp -d` copy; nothing outside it is written. Prints one PASS/FAIL line per
# test and exits 1 on any FAIL.
#   (a) new-only suite -> measured key + `_provisional` entry; `check` passes and prints the WARNING
#   (b) ci-refresh-weights.sh over shard-log lines clears the mark; one that omits the suite exits 1
#   (c) the unmodified tree: gen-test-suites.sh --check is ok and a plain run writes nothing
#   (d) a resized suite and a no-op regen add no mark and keep an existing one only for its own suite
# Env: SEED_SELFTEST_SCRIPTS=<dir> runs the three scripts from <dir> instead of this checkout's scripts/
#   (red-at-base: point it at origin/main's scripts and (a) must FAIL).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${SEED_SELFTEST_SCRIPTS:-$HERE/scripts}"
WORK="$(mktemp -d)"
case "$WORK" in /tmp/*|"${TMPDIR:-/nonexistent}"/*|/var/tmp/*|"$HOME"/.cache/tmp/*) ;; *) echo "selftest: refusing odd scratch root $WORK" >&2; exit 2 ;; esac
trap 'rm -rf "$WORK"' EXIT
export CORE_SHARDS=1 SANDBOX_SHARDS=1 GEN_TEST_SUITES_TODAY=2026-10-23
fails=0 passes=0
ok()   { passes=$((passes + 1)); echo "PASS $1"; }
bad()  { fails=$((fails + 1)); echo "FAIL $1${2:+: $2}"; }

# scratch <name> -> a repo with core files in area ca (3 files, one suite at cap 3) and sandbox files in sa (3),
# gen-run once, then "CI-measured": every suite measured 1.25x its recorded weight (inside the 1.5x stale line).
scratch() {
  local d="$WORK/$1"
  mkdir -p "$d/tests" "$d/scripts"
  printf '[package]\nname = "scratch"\nversion = "0.1.0"\n' > "$d/Cargo.toml"
  printf '{\n  "lib": 5\n}\n' > "$d/scripts/ci-suite-weights.json"
  echo '{}' > "$d/scripts/ci-suite-measured.json"
  local caps max
  caps="$(grep '^MAX_PER_SUITE = ' "$SRC/gen-test-suites.sh")" max="$(grep '^MAX_SUITES = ' "$SRC/gen-test-suites.sh")"
  python3 -I - "$SRC/gen-test-suites.sh" "$d/scripts/gen-test-suites.sh" "$caps" "$max" <<'PY'
import sys
src, dst, caps, mx = sys.argv[1:5]
s = open(src).read().replace(caps, 'MAX_PER_SUITE = {"core": 3, "sandbox": 3}').replace(mx, "MAX_SUITES = 6")
open(dst, "w").write(s)
PY
  cp "$SRC/ci-test-partition.sh" "$SRC/ci-refresh-weights.sh" "$d/scripts/"
  chmod +x "$d"/scripts/*.sh
  local i
  for i in 1 2 3; do
    printf '#[test]\nfn t() {}\n' > "$d/tests/ca_t$i.rs"
    printf '// Python%s marks this file sandbox-classified\n#[test]\nfn t() {}\n' "Kind" > "$d/tests/sa_t$i.rs"
  done
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
  echo "$d"
}
add_new_area() { # <dir>: three brand-new core files in a new area -> a suite made only of new files
  local d="$1" i
  for i in 1 2 3; do printf '#[test]\nfn t() {}\n' > "$d/tests/cb_t$i.rs"; done
}
jget() { python3 -I -c 'import json,sys; print(json.dumps(eval(sys.argv[2], {"d": json.load(open(sys.argv[1]))})))' "$@"; }

# ---- (a) a suite made only of new files gets a measured key + provisional mark; check passes with WARNING
d="$(scratch a)" || { bad "(a) setup"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d"
  if ! (cd "$d" && scripts/gen-test-suites.sh >"$WORK/a.gen" 2>&1); then bad "(a) gen" "$(tail -3 "$WORK/a.gen")"
  else
    new="$(cd "$d" && ls tests/suite_core_*.rs | sed 's#tests/##; s#\.rs$##' | tail -1)"
    m="$d/scripts/ci-suite-measured.json"
    w="$(jget "$d/scripts/ci-suite-weights.json" "d['$new']")"
    key="$(jget "$m" "'$new' in d")"; prov="$(jget "$m" "d.get('_provisional')")"
    # R = 5/4 = 1.25 over the old normal core bucket (suite_core_01); sandbox is a different partition
    want="$(python3 -I -c "print(round($w * 1.25, 2))")"
    got="$(jget "$m" "d.get('$new')")"
    out="$WORK/a.check"
    (cd "$d" && scripts/ci-test-partition.sh check >"$out" 2>&1); rc=$?
    if [ "$new" = suite_core_02 ] && [ "$key" = true ] && [ "$prov" = '{"suite_core_02": "2026-10-23"}' ] \
       && [ "$got" = "$want" ] && [ "$rc" = 0 ] && grep -q 'WARNING provisional measured entry: suite_core_02 was seeded on 2026-10-23' "$out"; then
      ok "(a) new-only suite $new seeded at $got (= $w x 1.25), marked provisional; check rc=0 with WARNING"
    else
      bad "(a) new-only suite" "new=$new key=$key prov=$prov got=$got want=$want check_rc=$rc :: $(grep -m2 -E 'unmeasured|WARNING|FAILED' "$out" | tr '\n' ' ')"
    fi
  fi
fi

# ---- (b) refresh clears the mark when the log measures the suite; exits 1 naming it when the log omits it
d="$(scratch b)" || { bad "(b) setup"; d=""; }
if [ -n "$d" ]; then
  add_new_area "$d"
  (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  required="$(cd "$d" && scripts/ci-test-partition.sh list measured-required)"
  log_all="$WORK/b.all.log"; log_part="$WORK/b.part.log"
  : > "$log_all"; : > "$log_part"
  for k in $required; do
    echo "2026-10-23T01:00:00Z suite=$k measured=7.00s recorded=4s" >> "$log_all"
    [ "$k" = suite_core_02 ] || echo "2026-10-23T01:00:00Z suite=$k measured=7.00s recorded=4s" >> "$log_part"
  done
  cp "$d/scripts/ci-suite-measured.json" "$WORK/b.before.m"; cp "$d/scripts/ci-suite-weights.json" "$WORK/b.before.w"
  (cd "$d" && CI_REFRESH_ROOT="$d" CI_REFRESH_ALLOW_LOCAL=1 scripts/ci-refresh-weights.sh "$log_part") >"$WORK/b.part.out" 2>&1; rc_part=$?
  same=0; cmp -s "$WORK/b.before.m" "$d/scripts/ci-suite-measured.json" && cmp -s "$WORK/b.before.w" "$d/scripts/ci-suite-weights.json" && same=1
  (cd "$d" && CI_REFRESH_ROOT="$d" CI_REFRESH_ALLOW_LOCAL=1 scripts/ci-refresh-weights.sh "$log_all") >"$WORK/b.all.out" 2>&1; rc_all=$?
  m="$d/scripts/ci-suite-measured.json"
  has_prov="$(jget "$m" "'_provisional' in d")"; seeded="$(jget "$m" "d.get('suite_core_02')")"
  (cd "$d" && scripts/ci-test-partition.sh check >"$WORK/b.check" 2>&1); rc_chk=$?
  if [ "$rc_part" != 0 ] && grep -q 'suite_core_02' "$WORK/b.part.out" && [ "$same" = 1 ] \
     && [ "$rc_all" = 0 ] && [ "$has_prov" = false ] && [ "$seeded" = 7.0 ] && [ "$rc_chk" = 0 ] && ! grep -q WARNING "$WORK/b.check"; then
    ok "(b) a log omitting suite_core_02 exits $rc_part naming it and writes nothing; a log measuring it clears _provisional (suite_core_02=7.0), check rc=0 without WARNING"
  else
    bad "(b) refresh" "rc_part=$rc_part same=$same rc_all=$rc_all has_prov=$has_prov seeded=$seeded rc_chk=$rc_chk :: $(tail -2 "$WORK/b.part.out" | tr '\n' ' ')"
  fi
fi

# ---- (c) the unmodified tree: --check ok, a plain run writes nothing (no drift)
t="$WORK/c"; mkdir -p "$t"
cp -r "$HERE/tests" "$HERE/scripts" "$HERE/Cargo.toml" "$t/"
cp "$SRC/gen-test-suites.sh" "$SRC/ci-test-partition.sh" "$SRC/ci-refresh-weights.sh" "$t/scripts/"  # base scripts when SEED_SELFTEST_SCRIPTS is set
(cd "$t" && scripts/gen-test-suites.sh --check >"$WORK/c.check" 2>&1); rc_c=$?
(cd "$t" && find . -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/c.before")
(cd "$t" && scripts/gen-test-suites.sh >"$WORK/c.gen" 2>&1); rc_g=$?
(cd "$t" && find . -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/c.after")
if [ "$rc_c" = 0 ] && grep -q 'gen-test-suites.sh --check: ok' "$WORK/c.check" && [ "$rc_g" = 0 ] \
   && grep -q 'already up to date' "$WORK/c.gen" && cmp -s "$WORK/c.before" "$WORK/c.after"; then
  ok "(c) unmodified tree: gen-test-suites.sh --check ok, plain run 'already up to date', 0 files changed"
else
  bad "(c) no drift" "check_rc=$rc_c gen_rc=$rc_g :: $(tail -2 "$WORK/c.check" | tr '\n' ' ') :: $(diff "$WORK/c.before" "$WORK/c.after" | head -3 | tr '\n' ' ')"
fi

# ---- (d) a resized suite is NOT marked; an existing mark survives a no-op regen and a resize of its own suite only
d="$(scratch d)" || { bad "(d) setup"; d=""; }
if [ -n "$d" ]; then
  rm "$d/tests/ca_t3.rs"
  (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  m="$d/scripts/ci-suite-measured.json"
  r_prov="$(jget "$m" "'_provisional' in d")"; r_val="$(jget "$m" "d['suite_core_01']")"
  add_new_area "$d"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  (cd "$d" && scripts/gen-test-suites.sh >"$WORK/d.noop" 2>&1)
  rm "$d/tests/ca_t1.rs"; (cd "$d" && scripts/gen-test-suites.sh >/dev/null 2>&1)
  p_after="$(jget "$m" "d.get('_provisional')")"
  if [ "$r_prov" = false ] && [ "$r_val" = 3.75 ] \
     && grep -q 'already up to date' "$WORK/d.noop" && [ "$p_after" = '{"suite_core_02": "2026-10-23"}' ]; then
    ok "(d) resized suite_core_01 -> 3.75 s unmarked; no-op regen 'already up to date'; the mark stays on suite_core_02 only"
  else
    bad "(d) resize/no-op" "r_prov=$r_prov r_val=$r_val p_after=$p_after"
  fi
fi

echo "ci-suite-seed-selftest: $passes passed, $fails failed (scripts from $SRC)"
[ "$fails" -eq 0 ]
