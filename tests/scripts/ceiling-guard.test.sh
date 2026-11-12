#!/usr/bin/env bash
# Hermetic test for scripts/ceiling-guard.sh: real git in a throwaway repo, no network, no cargo.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GUARD_SRC="$ROOT/scripts/ceiling-guard.sh"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/ceiling-guard-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

pass=0
failed=0
out=""
rc=0

# new_repo <name> <with-origin-ref: yes|no>: base commit with the guard installed and two sources.
new_repo() {
  local d="$WORK/$1"
  mkdir -p "$d/scripts" "$d/src" "$d/tests"
  git -C "$d" init -q -b main
  git -C "$d" config user.email t@example.invalid
  git -C "$d" config user.name t
  cp "$GUARD_SRC" "$d/scripts/ceiling-guard.sh"
  printf 'pub const FORWARD_STACK_BUDGET: usize = 1_280_000;\nfn a() {}\n' > "$d/src/handler.rs"
  printf 'const MAX_CEILING: usize = 1_048_576;\nconst BUDGET_CEILING: usize = 1_310_720;\n' > "$d/tests/x.rs"
  printf 'fn y() {}\n' > "$d/src/y.rs"
  printf '[[check]]\nname = "ceiling-guard"\ncmd = "./scripts/ceiling-guard.sh"\nwhere = "host"\nwall_minutes = 2\n' > "$d/wm-build-ci.toml"
  git -C "$d" add -A
  git -C "$d" commit -q -m base
  if [ "$2" = yes ]; then
    git -C "$d" update-ref refs/remotes/origin/main "$(git -C "$d" rev-parse HEAD)"
  fi
  echo "$d"
}

commit() { git -C "$1" add -A && git -C "$1" commit -q -m change; }

run_guard() { out=$(cd "$1" && bash scripts/ceiling-guard.sh 2>&1); rc=$?; }

# check <label> <want-rc> <substring...>
check() {
  local label=$1 want=$2 ok=1
  shift 2
  [ "$rc" -eq "$want" ] || ok=0
  for s in "$@"; do
    case "$out" in *"$s"*) ;; *) ok=0 ;; esac
  done
  if [ "$ok" -eq 1 ]; then
    pass=$((pass + 1)); echo "ok   $label (rc=$rc)"
  else
    failed=$((failed + 1)); echo "FAIL $label (rc=$rc, want $want)"
  fi
  printf '%s\n' "$out" | sed 's/^/     | /'
}

# (a) the 022d2d19 replay: budget re-measured, ceilings untouched
d=$(new_repo a yes)
sed -i 's/1_280_000/1_285_120/' "$d/src/handler.rs"
commit "$d"; run_guard "$d"
check "a: budget re-measure under unchanged ceilings" 0 "CEILING-GUARD PASS"

# (b) ceiling raise
d=$(new_repo b yes)
sed -i 's/BUDGET_CEILING: usize = 1_310_720/BUDGET_CEILING: usize = 1_400_000/' "$d/tests/x.rs"
commit "$d"; run_guard "$d"
check "b: ceiling raise" 1 "CEILING-GUARD tests/x.rs:" "1_310_720 -> 1_400_000"

# (c) unrelated .rs edit
d=$(new_repo c yes)
printf 'fn z() {}\n' >> "$d/src/y.rs"
commit "$d"; run_guard "$d"
check "c: unrelated edit" 0 "CEILING-GUARD PASS"

# (d) self-touch
d=$(new_repo d yes)
printf '# edited\n' >> "$d/scripts/ceiling-guard.sh"
commit "$d"; run_guard "$d"
check "d: self-touch" 1 "self-touch scripts/ceiling-guard.sh"

# (d2) self-touch of the guard's check block (wall_minutes raised)
d=$(new_repo d2 yes)
sed -i 's/wall_minutes = 2/wall_minutes = 99/' "$d/wm-build-ci.toml"
commit "$d"; run_guard "$d"
check "d2: toml block self-touch" 1 "self-touch wm-build-ci.toml"

# (e) no origin/main ref
d=$(new_repo e no)
printf 'fn z() {}\n' >> "$d/src/y.rs"
commit "$d"; run_guard "$d"
check "e: no origin/main" 1 "no-base"

# (f) brand-new ceiling in a new file
d=$(new_repo f yes)
printf 'const NEW_CEILING: usize = 4096;\n' > "$d/tests/new.rs"
commit "$d"; run_guard "$d"
check "f: pure add" 0 "add tests/new.rs" "CEILING-GUARD PASS"

# (g) ceiling moved from tests/ into src/ with a larger value
d=$(new_repo g yes)
sed -i '/BUDGET_CEILING/d' "$d/tests/x.rs"
printf 'const BUDGET_CEILING: usize = 2_000_000;\n' >> "$d/src/y.rs"
commit "$d"; run_guard "$d"
check "g: ceiling moved" 1 "CEILING-GUARD tests/x.rs:" "1_310_720 -> 2_000_000"

# (h) HEAD == origin/main
d=$(new_repo h yes)
run_guard "$d"
check "h: no diff" 0 "no-diff PASS"

echo "ceiling-guard.test: $pass passed, $failed failed"
[ "$failed" -eq 0 ]
