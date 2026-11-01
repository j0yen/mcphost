#!/usr/bin/env bash
# check-example-synthetic-header.sh -- the example and recipe scripts that sign
# up a tenant on a (possibly prod) hub must stamp it synthetic.
#
# The hub stamps a tenant synthetic at creation only when the signup request
# (an explicit `signup`, or a first keyless call that triggers implicit signup)
# carries the header `x-mcphost-synthetic: <label>` (control.rs
# validate_synthetic_header; regex ^[a-z0-9][a-z0-9:_-]{0,63}$). The `source`
# argument attributes but does not classify. A script that signs up without the
# header leaves a tenant counted as a real user on every prod run.
#
# Checks (grep only, no network, no cargo):
#   1. Every file under examples/ and scripts/ that both names `signup` and
#      drives a network client (curl, urllib, requests) either carries the
#      header or is listed in ALLOW below with the reason it does not.
#      Discovery is by the word `signup`, so a script that reaches implicit
#      signup through a bare keyless first call, with no `signup` anywhere in
#      it, is not found -- review such a script by hand.
#   2. Every `mcp_call "signup" ...` line (the helper style the proof scripts
#      share, in examples/, tests/golden/recipes/ and recipes/*.toml) carries
#      the header on that same line.
#   3. examples/docs-qa/docs-qa.sh sends the header on its signup call.
#   4. tests/common/mod.rs (the test client) defines the non-loopback label.
#
# Usage: scripts/check-example-synthetic-header.sh [repo-root]
# Exit 0 clean, 1 on any violation (each printed as `FAIL: ...`), 2 on a bad root.
set -uo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$root" || { echo "check-example-synthetic-header: no such root: $root" >&2; exit 2; }
self="scripts/check-example-synthetic-header.sh"
header_re='x-mcphost-synthetic'
fails=0

fail() { echo "FAIL: $1"; fails=$((fails + 1)); }

# Files that sign up (or name signup) with a network client and deliberately do
# not stamp. One `path|reason` per entry.
ALLOW=(
  "scripts/ops/recipe-counter-live.sh|tests the funnel's clientInfo-prefix (synthorg) classification, so the tenant must not also carry the header"
)
allowed() {
  local entry
  for entry in "${ALLOW[@]}"; do
    [[ "${entry%%|*}" == "$1" ]] && return 0
  done
  return 1
}

# 1. discovery
checked=0
while IFS= read -r f; do
  [[ "$f" == "$self" ]] && continue
  grep -qiE 'signup' "$f" || continue
  grep -qE 'curl|urllib|requests\.|urlopen' "$f" || continue
  checked=$((checked + 1))
  if grep -qi "$header_re" "$f"; then
    continue
  fi
  allowed "$f" && continue
  fail "$f signs up over the network but never sends $header_re"
done < <(find examples scripts -type f \( -name '*.sh' -o -name '*.py' \) | sort)

# 2. the shared mcp_call helper style: signup lines carry the header
while IFS= read -r f; do
  while IFS=: read -r n line; do
    [[ -n "$n" ]] || continue
    if ! grep -qi "$header_re" <<<"$line"; then
      fail "$f:$n mcp_call \"signup\" without $header_re on the call"
    fi
  done < <(grep -nE 'mcp_call +"signup"' "$f" || true)
done < <(find examples tests/golden/recipes recipes -type f \( -name '*.sh' -o -name '*.toml' \) | sort)

# 3. docs-qa.sh builds its signup over several lines
if ! grep -A4 '"signup",' examples/docs-qa/docs-qa.sh | grep -qi "$header_re"; then
  fail "examples/docs-qa/docs-qa.sh: signup call does not send $header_re"
fi

# 4. the test client's defensive label
if ! grep -q 'harness:mcphost-test' tests/common/mod.rs; then
  fail "tests/common/mod.rs: McpClient does not define the harness:mcphost-test label"
fi

if [[ "$fails" -gt 0 ]]; then
  echo "check-example-synthetic-header: $fails violation(s)"
  exit 1
fi
echo "check-example-synthetic-header: ok ($checked network signup script(s) checked)"
