#!/usr/bin/env bash
# Test for `scripts/ci-test-partition.sh check --land` (spec b, ctl 5991): bash only, no network, no cargo.
# Builds a temp git fixture from this repo's committed tree (so the real `check` runs end to end), rewrites
# scripts/ci-suite-measured.json to carry ONE pending (`_provisional`) entry whose `since` is a known
# commit, and moves a fake `origin/main` to chosen ages. Age arithmetic: `since` = B0; main = B0 -> B1 (the
# commit that writes the pending entry) -> k filler commits, so the base tip is 1+k first-parent commits
# past `since`. pr mode reports that + 1 (the squash the land adds); merge_group counts to HEAD, no +1.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
T="$(mktemp -d "${TMPDIR:-/tmp}/ptland.XXXXXX")" || exit 2
trap 'rm -rf "$T"' EXIT
FX="$T/fx"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fails=0 passes=0
ok()   { passes=$((passes + 1)); echo "ok   - $1"; }
fail() { fails=$((fails + 1)); echo "FAIL - $1"; [ -n "${OUT:-}" ] && printf '%s\n' "$OUT" | sed 's/^/       | /' | tail -12; }
g() { git -C "$FX" "$@"; }

git clone -q --local --no-checkout "$ROOT" "$FX" || exit 2
g checkout -q --detach "$(git -C "$ROOT" rev-parse HEAD)" || exit 2
cp "$ROOT/scripts/ci-test-partition.sh" "$FX/scripts/ci-test-partition.sh"   # the working-tree script under test
g add -A && g commit -q -m "fixture: script under test" || exit 2
B0="$(g rev-parse HEAD)"
SUITE="$(jq -r 'keys[] | select(startswith("suite_"))' "$FX/scripts/ci-suite-measured.json" | head -1)"
[ -n "$SUITE" ] || { echo "fixture: no suite_* key in ci-suite-measured.json"; exit 2; }

set_pending() { # <since> : the one pending entry at the suite's own measured value
  jq --arg k "$SUITE" --arg s "$1" '._provisional = {($k): {pending: true, derived_s: .[$k], since: $s}}' \
    "$FX/scripts/ci-suite-measured.json" > "$T/m.json" && cp "$T/m.json" "$FX/scripts/ci-suite-measured.json"
}
mk_main() { # <k> : branch main = B0 -> B1(pending, since B0) -> k fillers; origin/main = its tip
  g checkout -q -B main "$B0" && set_pending "$B0" && g commit -q -am "pending entry" || exit 2
  local i; for i in $(seq 1 "$1"); do g commit -q --allow-empty -m "filler $i" || exit 2; done
  g update-ref refs/remotes/origin/main "$(g rev-parse main)"
}
mk_pr() { # <branch> <from-rev> : a one-commit PR branch that leaves measured.json alone
  g checkout -q -B "$1" "$2" && echo "$1" > "$FX/pr-$1.txt" && g add -A && g commit -q -m "pr $1" || exit 2
}
run() { # <env assignments...> -- <check args...> : OUT/RC from the fixture's own script
  local envs=() ; while [ "$1" != -- ]; do envs+=("$1"); shift; done; shift
  OUT="$(cd "$FX" && env -u GITHUB_REF_NAME -u GITHUB_BASE_REF -u GITHUB_EVENT_NAME "${envs[@]}" bash scripts/ci-test-partition.sh check "$@" 2>&1)"; RC=$?
}
want_pass() { # <label> <needle>
  if [ "$RC" -eq 0 ] && grep -qF -- "$2" <<<"$OUT"; then ok "$1"; else fail "$1 (rc=$RC, wanted pass with: $2)"; fi
}
want_refuse() { # <label> <needle>
  if [ "$RC" -ne 0 ] && grep -qF -- "$2" <<<"$OUT"; then ok "$1"; else fail "$1 (rc=$RC, wanted refusal with: $2)"; fi
}

# --- pr mode ---------------------------------------------------------------------------------------------
mk_main 1; mk_pr pr-a main                       # base tip is 2 past since -> post-merge 3
run -- --land --land-mode=pr
want_pass "pr: post-merge age 3 passes" "post-merge age 3 (max 3)"
run -- --land                                    # default mode is pr
want_pass "pr is the default mode" "--land (pr)"

mk_main 2; mk_pr pr-b main                       # tip 3 past since -> post-merge 4
run -- --land --land-mode=pr
want_refuse "pr: post-merge age 4 refuses with the new message" "(post-merge age 4, PR would turn main red)"

# STALE PR branch: cut 2 behind the base tip. Its merge-base is 1 past since, so a merge-base anchor would
# say 1+1=2 and pass; the base-tip anchor says 3+1=4 and refuses.
mk_main 2; mk_pr pr-stale "main~2"
run -- --land --land-mode=pr
want_refuse "pr: stale branch (merge-base 2 behind tip) refuses on tip age + 1" "(post-merge age 4, PR would turn main red)"

# GITHUB_BASE_REF picks the base: release branch `rel` sits 1 past since (post-merge 2) while main is 3 past.
mk_main 2; mk_pr pr-rel main; g update-ref refs/remotes/origin/rel "main~2"
run GITHUB_BASE_REF=rel -- --land --land-mode=pr
want_pass "pr: GITHUB_BASE_REF=rel is the anchor (passes where main would refuse)" "post-merge age 2 (max 3)"
run -- --land --land-mode=pr
want_refuse "pr: same branch against origin/main refuses" "(post-merge age 4, PR would turn main red)"

# HEAD's measured.json no longer pending -> skipped at any age.
mk_main 8
g checkout -q -B pr-done main && jq 'del(._provisional)' "$FX/scripts/ci-suite-measured.json" > "$T/m.json" \
  && cp "$T/m.json" "$FX/scripts/ci-suite-measured.json" && g commit -q -am "refresh: measured" || exit 2
run -- --land --land-mode=pr
if [ "$RC" -eq 0 ] && ! grep -qF "post-merge" <<<"$OUT"; then ok "pr: entry no longer pending passes at age 9"; else fail "pr: entry no longer pending (rc=$RC)"; fi

# --- fail closed -----------------------------------------------------------------------------------------
mk_main 0; mk_pr pr-c main
run GITHUB_BASE_REF=nosuchbase -- --land --land-mode=pr
want_refuse "fail closed: missing base ref" "::error::--land needs the base branch history (fetch-depth 0): cannot verify origin/nosuchbase"

git clone -q --no-local --depth 1 "file://$FX" "$T/shallow" 2>/dev/null
SAVED_FX="$FX"; FX="$T/shallow"; git -C "$FX" update-ref refs/remotes/origin/main HEAD
g config user.name t; g config user.email t@t
run -- --land --land-mode=pr
want_refuse "fail closed: shallow repo" "::error::--land needs the base branch history (fetch-depth 0)"
FX="$SAVED_FX"

mk_main 0; g checkout -q -B pr-d main && set_pending "0123456789abcdef0123456789abcdef01234567" && g commit -q -am "bogus since" || exit 2
run -- --land --land-mode=pr
if [ "$RC" -ne 0 ] && ! grep -qF "has post-merge age" <<<"$OUT"; then ok "fail closed: unverifiable since never reads as age 0"; else fail "fail closed: unverifiable since (rc=$RC)"; fi

mk_main 0; g checkout -q -B pr-e main && g commit -q --allow-empty -m "pr-only commit" && set_pending "$(g rev-parse HEAD)" && g commit -q -am "since on the PR branch only" || exit 2
run -- --land --land-mode=pr
want_refuse "fail closed: since not an ancestor of the base" "is not an ancestor of origin/main, so its post-merge age cannot be counted"

# --- merge_group mode: a 2-PR batch, count runs to HEAD, no +1 ------------------------------------------
batch() { # <k> : main with k fillers, then the queue head carries two batched PR commits
  mk_main "$1"; g checkout -q -B queue main
  g commit -q --allow-empty -m "batched PR 1" && g commit -q --allow-empty -m "batched PR 2" || exit 2
}
batch 0                                          # since..HEAD = B1 + 2 batched = 3
run -- --land --land-mode=merge_group
want_pass "merge_group: 2-PR batch at age 3 passes (no +1, counts to HEAD)" "post-merge age 3 (max 3)"
batch 1                                          # 4
run -- --land --land-mode=merge_group
want_refuse "merge_group: 2-PR batch at age 4 refuses" "(post-merge age 4, PR would turn main red)"

# --- main mode unchanged ---------------------------------------------------------------------------------
mk_main 3                                        # HEAD = tip, 4 past since
run -- --main
want_refuse "main: --main refuses with the original message" "is 4 first-parent commits behind HEAD (max 3) and was never measured -- run scripts/ci-refresh-weights.sh <CI run id> from a green run's shard logs"
if grep -qF "post-merge" <<<"$OUT"; then fail "main: --main output must not mention post-merge"; else ok "main: --main output has no --land text"; fi
run --
want_pass "main: bare check off the default branch stays info only" "4 first-parent commits behind HEAD, max 3, info only: not the default branch"
run GITHUB_REF_NAME=main --
want_refuse "main: GITHUB_REF_NAME=main still enforces" "is 4 first-parent commits behind HEAD (max 3)"

run -- --bogus
if [ "$RC" -eq 2 ]; then ok "unknown check argument exits 2"; else fail "unknown check argument exits 2 (rc=$RC)"; fi

echo "ci-test-partition-land: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
