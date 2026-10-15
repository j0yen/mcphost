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

"$AUDIT" . > target/autobuilder/receipts/risk-gate.json
