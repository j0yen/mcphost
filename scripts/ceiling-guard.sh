#!/usr/bin/env bash
# ceiling-guard.sh -- wm-build gate check: a run may not change a *CEILING* constant.
#
# Run 506 (2026-10-30) went red -> green by re-measuring FORWARD_STACK_BUDGET under the
# tests/ ceilings, which stayed put. That is the repo contract. A coder that RAISES a ceiling
# instead would turn the same red green and hide it, so any change to a *CEILING* definition
# in any .rs file between the merge-base with origin/main and HEAD fails here. Hand PRs are the
# only legitimate way to change a ceiling (this check is wm-build only, not mirrored in ci.yml).
#
#   *CEILING* const/static, '-' line (raise, lower, rename, delete, move)  -> FAIL
#   *CEILING* const/static, '+' line with no '-' of the same name (new)    -> ok
#   *BUDGET*  consts                                                       -> pass anywhere
#
# No env overrides, by design. The base is origin/main; with no such ref the check fails closed.
# self-touch = advisory until wm-build reads wm-build-ci.toml and check scripts from the
# merge-base tree (git show <base>:<path>), not HEAD.
set -euo pipefail

SELF=scripts/ceiling-guard.sh
SELF_TEST=tests/scripts/ceiling-guard.test.sh
SELF_TOML=wm-build-ci.toml

if ! git rev-parse --verify -q refs/remotes/origin/main >/dev/null; then
  echo "CEILING-GUARD no-base"
  exit 1
fi
head=$(git rev-parse HEAD)
if [ "$head" = "$(git rev-parse refs/remotes/origin/main)" ]; then
  echo "CEILING-GUARD base=$head no-diff PASS"
  exit 0
fi
if ! base=$(git merge-base HEAD refs/remotes/origin/main); then
  echo "CEILING-GUARD no-base"
  exit 1
fi

fail=0

# Self-touch: only once the guard is installed at the base (the run that introduces it is exempt).
if git cat-file -e "$base:$SELF" 2>/dev/null; then
  for p in "$SELF" "$SELF_TEST"; do
    if ! git diff --quiet "$base" "$head" -- "$p"; then
      echo "CEILING-GUARD self-touch $p"
      fail=1
    fi
  done
  block() {
    awk '
      function flush() { if (buf ~ /name = "ceiling-guard"/) printf "%s", buf; buf = "" }
      /^\[\[/ { flush(); buf = $0 "\n"; next }
      /^[[:space:]]*$/ { flush(); next }
      buf != "" { buf = buf $0 "\n" }
      END { flush() }'
  }
  old_block=""
  new_block=""
  if git cat-file -e "$base:$SELF_TOML" 2>/dev/null; then old_block=$(git show "$base:$SELF_TOML" | block); fi
  if git cat-file -e "$head:$SELF_TOML" 2>/dev/null; then new_block=$(git show "$head:$SELF_TOML" | block); fi
  if [ "$old_block" != "$new_block" ]; then
    echo "CEILING-GUARD self-touch $SELF_TOML"
    fail=1
  fi
fi

# Ceiling definitions: scan every .rs line changed since the base.
if ! scan=$(git -c color.ui=never diff --no-ext-diff --no-renames -U0 "$base" "$head" -- '*.rs' |
  awk '
    function trim(s) { sub(/^[[:space:]]+/, "", s); sub(/[[:space:]]+$/, "", s); return s }
    /^--- / { oldf = substr($0, 7); if ($0 ~ /^--- \/dev\/null/) oldf = ""; next }
    /^\+\+\+ / { file = ($0 ~ /^\+\+\+ \/dev\/null/) ? oldf : substr($0, 7); next }
    /^@@ / {
      s = $2; sub(/^-/, "", s); split(s, a, ","); ol = a[1] + 0
      s = $3; sub(/^\+/, "", s); split(s, b, ","); nl = b[1] + 0
      next
    }
    /^[+-]/ {
      sign = substr($0, 1, 1); line = (sign == "-") ? ol++ : nl++
      if ($0 ~ /^[+-][[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?(const|static)[[:space:]]+[A-Z0-9_]*CEILING[A-Z0-9_]*[[:space:]]*:/) {
        t = $0; sub(/^[+-][[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?(const|static)[[:space:]]+/, "", t)
        name = t; sub(/[[:space:]]*:.*/, "", name)
        val = t; sub(/^[^=]*=/, "", val); sub(/;.*$/, "", val); val = trim(val)
        if (sign == "-") { n_old++; of[n_old] = file; ol_[n_old] = line; on[n_old] = name; ov[n_old] = val; oldname[name] = 1 }
        else { n_new++; nf[n_new] = file; nl_[n_new] = line; nn[n_new] = name; nv[n_new] = val; used[n_new] = 0 }
      }
    }
    END {
      for (i = 1; i <= n_old; i++) {
        nw = "<none>"
        for (j = 1; j <= n_new; j++) if (!used[j] && nn[j] == on[i]) { nw = nv[j]; used[j] = 1; break }
        printf "CEILING-GUARD %s:%d %s -> %s\n", of[i], ol_[i], ov[i], nw
      }
      for (j = 1; j <= n_new; j++)
        if (!(nn[j] in oldname)) printf "CEILING-GUARD add %s:%d %s ok\n", nf[j], nl_[j], nn[j]
        else if (!used[j]) printf "CEILING-GUARD %s:%d <none> -> %s\n", nf[j], nl_[j], nv[j]
    }'); then
  echo "CEILING-GUARD scan-error"
  exit 1
fi
if [ -n "$scan" ]; then
  echo "$scan"
  nonadd=$(printf '%s\n' "$scan" | awk '!/^CEILING-GUARD add /')
  if [ -n "$nonadd" ]; then fail=1; fi
fi

if [ "$fail" -ne 0 ]; then
  echo "CEILING-GUARD FAIL base=$base head=$head host=$(hostname)"
  exit 1
fi
echo "CEILING-GUARD PASS base=$base head=$head host=$(hostname)"
