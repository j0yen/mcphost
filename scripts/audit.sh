#!/usr/bin/env bash
# audit.sh-trace: emits ::audit-start marker for harness instrumentation
# audit.sh — runs the BAD_RUST audit against this project.
# READ-ONLY: the edit-agent must not modify this file.
#
# ONE SOURCE OF TRUTH: the vendored rules/audit-checks.sh — the same file CI runs.
# (10-15: a hosts /home/jsy/.claude/skills/rustbuild copy had drifted from the repos, so a
# coders local audit passed while CIs `bad-rust audit` blocked; the skill copy is
# never consulted any more, only reported when it differs.)
# Output: target/autobuilder/receipts/risk-gate.json (the gate's expected
# location — eliminates the previous manual `cp audit.json risk-gate.json`
# step every standalone build needed).
# Exit 0 if no BLOCKING findings, 1 otherwise.

set -euo pipefail
cd "$(dirname "$0")/.."

AUDIT="rules/audit-checks.sh"
SKILL_COPY="$HOME/.claude/skills/rustbuild/rules/audit-checks.sh"
if [ -f "$SKILL_COPY" ] && ! cmp -s "$SKILL_COPY" "$AUDIT"; then
  echo "audit: NOTE host skill copy $SKILL_COPY differs from the repo rules; the repo rules decide (same as CI)" >&2
fi
mkdir -p target/autobuilder/receipts

if [ ! -x "$AUDIT" ]; then
  echo "audit: no vendored rules/audit-checks.sh" >&2
  exit 1
fi

# Capture the exit code without letting set -e abort, so the BLOCKING findings can be
# named on stderr (rule_id path:line snippet): the gate keeps only the output tail, and
# the JSON receipt alone left the 10-20 run 1011 finding as the NOTE line above.
# The marker below is the `fail_marker` of the bad-rust-audit check in wm-build-ci.toml;
# scripts/audit-marker-check.sh asserts the two cannot drift.
rc=0
"$AUDIT" . > target/autobuilder/receipts/risk-gate.json || rc=$?
if [ "$rc" -ne 0 ]; then
  jq -r '.findings[]? | select(.severity == "blocking") | "audit: BLOCKING \(.rule_id) \(.path):\(.line) \(.snippet)"' \
    target/autobuilder/receipts/risk-gate.json >&2 || true
fi
exit "$rc"
