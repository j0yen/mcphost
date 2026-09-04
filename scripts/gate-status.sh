#!/usr/bin/env bash
# gate-status.sh — one-liner wrapper around `autobuilder gate`: prints the
# gate's own summary line and exits with the gate's verdict (0 for pass,
# 1 for block) so this script is usable directly in a CI step or a
# pre-push hook without parsing autobuilder's per-receipt breakdown.
set -uo pipefail
cd "$(dirname "$0")/.."

OUT="$(autobuilder gate --project . 2>&1)"
STATUS=$?
printf '%s\n' "$OUT" | head -1
exit "$STATUS"
