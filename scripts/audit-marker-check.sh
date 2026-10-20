#!/usr/bin/env bash
# audit-marker-check.sh -- cargo-free contract check (run by `pin-tests.sh --cargo-free`).
#
# The `fail_marker` of the bad-rust-audit [[check]] in wm-build-ci.toml and the BLOCKING
# print line in scripts/audit.sh are one string shared by two files. This reads the marker
# from the toml (the one source) and asserts audit.sh prints its blocking findings with
# exactly that literal prefix, so the gate's fail-marker match cannot drift from the script.
set -euo pipefail
cd "$(dirname "$0")/.."

toml=${AUDIT_MARKER_TOML:-wm-build-ci.toml}
script=${AUDIT_MARKER_SCRIPT:-scripts/audit.sh}

# The bad-rust-audit [[check]] block: from its name line to the next [[check]] header.
block=$(awk '/^\[\[check\]\]/ { inblk = 0 } /^name = "bad-rust-audit"/ { inblk = 1 } inblk' "$toml")
[ -n "$block" ] || { echo "audit-marker-check: FAILED no bad-rust-audit [[check]] in $toml"; exit 1; }

marker=$(printf '%s\n' "$block" | sed -n 's/^fail_marker = "\(.*\)"[[:space:]]*\(#.*\)\{0,1\}$/\1/p' | head -1)
receipt=$(printf '%s\n' "$block" | sed -n 's/^receipt = "\(.*\)"[[:space:]]*\(#.*\)\{0,1\}$/\1/p' | head -1)
[ -n "$marker" ] || { echo "audit-marker-check: FAILED bad-rust-audit declares no fail_marker in $toml"; exit 1; }
[ -n "$receipt" ] || { echo "audit-marker-check: FAILED bad-rust-audit declares no receipt in $toml"; exit 1; }

# The print line: the jq filter string must open with the marker, followed by the rule_id.
if ! grep -qF "\"$marker \\(.rule_id)" "$script"; then
  echo "audit-marker-check: FAILED $script does not print a line starting with the toml fail_marker \"$marker\""
  exit 1
fi
# The receipt the check declares is the file audit.sh writes.
if ! grep -qF "> $receipt" "$script"; then
  echo "audit-marker-check: FAILED $script does not write the toml receipt $receipt"
  exit 1
fi
echo "audit-marker-check: ok (fail_marker \"$marker\", receipt $receipt)"
