#!/usr/bin/env bash
# rules/tests/run.sh — fixture self-test for rules/audit-checks.sh detectors.
# Each case writes a tiny src/x.rs under a mktemp -d root it created itself and
# runs grep_secret_in_debug_impl, sourced from the repo's rules/audit-checks.sh
# (everything before the final `main "$@"` driver line), then compares the
# finding count. Exit non-zero, naming the case, on any mismatch.
# audit-checks.sh main() runs this first, so a broken detector cannot pass the gate.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AUDIT="$HERE/../audit-checks.sh"

TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP:?}"; }
trap cleanup EXIT

# Library form of the audit script: drop the `main "$@"` driver and the exit tail.
sed '/^main "\$@"$/,$d' "$AUDIT" > "$TMP/lib.sh"

fails=0
n=0

# run_case <name> <expected finding count> ; fixture source on stdin
run_case() {
  local name="$1" want="$2" root got
  n=$((n + 1))
  root="$(mktemp -d "$TMP/case.XXXXXX")"
  mkdir -p "$root/src"
  cat > "$root/src/x.rs"
  got=$(
    set -- "$root"
    # shellcheck disable=SC1091
    source "$TMP/lib.sh"
    trap - EXIT
    grep_secret_in_debug_impl >/dev/null 2>&1
    c=$(grep -c "\"rule_id\"" "$FINDINGS_FILE" 2>/dev/null || true)
    rm -f "$FINDINGS_FILE"
    echo "${c:-0}"
  )
  if [ "$got" = "$want" ]; then
    echo "ok   $name (findings=$got)"
  else
    echo "FAIL $name: expected $want finding(s), got $got"
    fails=$((fails + 1))
  fi
}

run_case "case1 derive(Debug) enum must not taint a later underived struct (HLT-010 false positive)" 0 <<'EOF2'
#[derive(Debug)]
pub enum E {
    A,
}

pub struct S<'a> {
    pub token: Option<&'a str>,
}
EOF2

run_case "case2 REGRESSION GUARD: derive(Debug) struct with token field is flagged" 1 <<'EOF2'
#[derive(Debug)]
pub struct S {
    pub token: String,
}
EOF2

run_case "case3 REGRESSION GUARD: derive(Debug) + stacked cfg_attr + doc comment still flagged" 1 <<'EOF2'
#[derive(Debug)]
#[cfg_attr(feature = "x", derive(Serialize))]
/// doc comment between attributes and the struct
pub struct S {
    pub token: String,
}
EOF2

if [ "$fails" -gt 0 ]; then
  echo "rules/tests/run.sh: $fails of $n case(s) FAILED"
  exit 1
fi
echo "rules/tests/run.sh: all $n cases passed"
