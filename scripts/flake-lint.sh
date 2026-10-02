#!/usr/bin/env bash
# flake-lint.sh — PRD-mcphost-test-suite-flake-lints.
#
# Textual (not syntactic) scan over a tree of *.rs test files for three
# shapes that cost mcphost four hand-interventions on gate blocks in one day
# (2026-09-30): a bare `std::env::set_var`/`remove_var` outside
# `tests/common::EnvGuard` (two tests in the same consolidated suite binary
# raced the same var); a wall-clock budget asserted from one measurement
# instead of `perf_budget!`'s warm-median (a 2ms miss on a loaded builder);
# and a test that opens the gate's own receipt file (fails every later land
# once the receipt it was produced in no longer matches HEAD, the
# kindroute_ac08 shape fixed in PR #77).
#
# Deliberately conservative and line-oriented (Technical considerations: "a
# false negative is acceptable, a false positive on tests/common is not") --
# this is a grep-shaped lint, not a parser, so it only ever sees what's
# written literally in the file. `tests/common/**` is self-exempt by path:
# it is where EnvGuard/perf_budget! themselves live, and every rule below
# would otherwise fire on their own implementation.
#
# Rules:
#   env-mutation      `env::set_var(`/`env::remove_var(` outside a function
#                      preceded by a `// flake-lint: env-guarded` comment.
#                      A test that goes through `common::EnvGuard::set(...)`
#                      instead never spells `set_var`/`remove_var` in its own
#                      source, so it naturally reports nothing.
#   single-shot-perf   an `assert!` on the same line as both `.elapsed()` and
#                      a `Duration::from_*` literal -- a `perf_budget!(...)`
#                      call site never spells `elapsed()`/`Duration::from_`
#                      itself (the macro owns timing), so it naturally
#                      reports nothing either.
#   receipt-read       a string literal containing `autobuilder/receipts`,
#                      `gate-receipts`, or `.wm-build`.
#
# Findings not listed in tests/flake-lint-allow.txt (`path:rule # reason`,
# one entry per line) are blocking: exit 1 if any remain. Output is
# `path:line rule message`, one finding per line, allow-listed findings
# marked `(allowed)`, followed by a one-line summary with an `allowed: N`
# count.
#
# Usage:
#   scripts/flake-lint.sh [DIR-or-FILE]   defaults to `tests`
set -uo pipefail

# Deliberately no `cd` to a fixed repo root: ALLOW_FILE is resolved relative
# to TARGET so a test can point this at an isolated fixture tree (its own
# tests/flake-lint-allow.txt) without touching the real one. The real gate
# invocation runs `scripts/flake-lint.sh` (default TARGET=tests) from the
# repo root, where that resolves to the real tests/flake-lint-allow.txt.
TARGET="${1:-tests}"
if [ -d "$TARGET" ]; then
  ALLOW_FILE="$TARGET/flake-lint-allow.txt"
  mapfile -t FILES < <(find "$TARGET" -type f -name '*.rs' | LC_ALL=C sort)
else
  ALLOW_FILE="$(dirname "$TARGET")/flake-lint-allow.txt"
  FILES=("$TARGET")
fi

declare -A ALLOWED
if [ -f "$ALLOW_FILE" ]; then
  while IFS= read -r raw; do
    entry="${raw%%#*}"
    entry="$(printf '%s' "$entry" | sed -E 's/[[:space:]]+$//')"
    [ -n "$entry" ] && ALLOWED["$entry"]=1
  done < "$ALLOW_FILE"
fi

allowed_count=0
blocking_count=0

is_self_exempt() { # <path> -> 0 (exempt) iff under a tests/common/ directory
  case "$1" in
    */tests/common/*|tests/common/*) return 0 ;;
    *) return 1 ;;
  esac
}

# is_env_guarded <file> <lineno> -> "yes" iff the nearest enclosing `fn`
# above <lineno> is immediately preceded (attribute lines like #[test]
# skipped) by a `// flake-lint: env-guarded` comment.
is_env_guarded() {
  awk -v target="$2" '
    { lines[NR] = $0 }
    END {
      fnline = 0
      for (i = target; i >= 1; i--) {
        if (lines[i] ~ /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]/) { fnline = i; break }
      }
      if (fnline == 0) { print "no"; exit }
      i = fnline - 1
      while (i >= 1 && lines[i] ~ /^[[:space:]]*#\[/) i--
      if (i >= 1 && lines[i] ~ /flake-lint: env-guarded/) print "yes"; else print "no"
    }
  ' "$1"
}

report() { # <path> <line> <rule> <message>
  local path="$1" line="$2" rule="$3" message="$4"
  local key="$path:$rule"
  if [ -n "${ALLOWED[$key]:-}" ]; then
    allowed_count=$((allowed_count + 1))
    printf '%s:%s %s %s (allowed)\n' "$path" "$line" "$rule" "$message"
  else
    blocking_count=$((blocking_count + 1))
    printf '%s:%s %s %s\n' "$path" "$line" "$rule" "$message"
  fi
}

lint_file() {
  local file="$1"
  is_self_exempt "$file" && return 0

  while IFS=: read -r lineno rest; do
    [ -z "${lineno:-}" ] && continue
    [ "$(is_env_guarded "$file" "$lineno")" = "yes" ] && continue
    report "$file" "$lineno" "env-mutation" \
      'set_var/remove_var outside EnvGuard — wrap in EnvGuard::set("VAR", value) (see tests/common::EnvGuard)'
  done < <(grep -nE '\benv::(set_var|remove_var)\(' "$file" 2>/dev/null)

  while IFS=: read -r lineno content; do
    [ -z "${lineno:-}" ] && continue
    case "$content" in
      *assert!*) ;;
      *) continue ;;
    esac
    if [[ "$content" == *"elapsed()"* ]] && [[ "$content" == *"Duration::from_"* ]]; then
      report "$file" "$lineno" "single-shot-perf" \
        'single measurement of elapsed() against a Duration literal in assert! — wrap in perf_budget!(budget_ms, { ... }) (median of >=5 warm runs)'
    fi
  done < <(grep -nE 'assert!' "$file" 2>/dev/null)

  while IFS=: read -r lineno rest; do
    [ -z "${lineno:-}" ] && continue
    report "$file" "$lineno" "receipt-read" \
      'opens a path under the gate receipt tree — tests must assert behaviour, not the receipt the gate itself produced (kindroute_ac08 rule, mcphost PR #77: a test that reads its own gate receipt fails every later land)'
  done < <(grep -nE 'autobuilder/receipts|gate-receipts|\.wm-build' "$file" 2>/dev/null)
}

for f in "${FILES[@]:-}"; do
  [ -n "$f" ] || continue
  [ -f "$f" ] || continue
  lint_file "$f"
done

total=$((allowed_count + blocking_count))
echo "flake-lint: findings: $total allowed: $allowed_count blocking: $blocking_count"

if [ "$blocking_count" -gt 0 ]; then
  exit 1
fi
exit 0
