#!/usr/bin/env bash
# registry-submit.sh -- PRD-mcphost-registry-entry-one-source.
#
#   registry-submit.sh --dry-run   check + validate + live comparison, then
#                                   print the exact `mcp-publisher` command
#                                   (key shown as $MCPHOST_REGISTRY_PRIVATE_KEY,
#                                   never read or echoed). Exits 0 whatever
#                                   the drift says; 1 only if check/validate fail.
#   registry-submit.sh             publishes only when PUBLISH-OK exists
#                                   (override path: $MCPHOST_PUBLISH_OK) AND
#                                   $MCPHOST_REGISTRY_PRIVATE_KEY is set;
#                                   otherwise prints `would publish ...` and
#                                   exits 0 having done no network call.
#
# Binary: $MCPHOST_BIN, else `mcphost` on PATH, else target/{release,debug}.
set -uo pipefail
cd "$(dirname "$0")/.."

ARTIFACT="registry/server.json"
BIN="${MCPHOST_BIN:-}"
if [ -z "$BIN" ]; then
  if command -v mcphost >/dev/null 2>&1; then BIN="mcphost"
  elif [ -x target/release/mcphost ]; then BIN="target/release/mcphost"
  elif [ -x target/debug/mcphost ]; then BIN="target/debug/mcphost"
  else echo "registry-submit.sh: no mcphost binary (set MCPHOST_BIN)" >&2; exit 2; fi
fi

PUBLISH_CMD='mcp-publisher login dns --domain mcphost.dev --private-key "$MCPHOST_REGISTRY_PRIVATE_KEY" && mcp-publisher publish '"$ARTIFACT"

dry=0
case "${1:-}" in
  --dry-run) dry=1 ;;
  "") ;;
  *) echo "usage: registry-submit.sh [--dry-run]" >&2; exit 2 ;;
esac

out="$(env -u MCPHOST_PUBLIC_URL "$BIN" registry-manifest --check 2>&1)"; rc=$?
echo "check: $(printf '%s\n' "$out" | head -1)"
[ "$rc" -eq 0 ] || { echo "check failed" >&2; exit 1; }
out="$("$BIN" registry-manifest --validate 2>&1)"; rc=$?
echo "validate: $(printf '%s\n' "$out" | head -1)"
[ "$rc" -eq 0 ] || { echo "validate failed" >&2; exit 1; }

if [ "$dry" -eq 1 ]; then
  echo "live: $("$BIN" registry-manifest --live 2>&1 | head -1)"
  echo "publish: $PUBLISH_CMD"
  exit 0
fi

ok_file="${MCPHOST_PUBLISH_OK:-PUBLISH-OK}"
if [ ! -e "$ok_file" ] || [ -z "${MCPHOST_REGISTRY_PRIVATE_KEY:-}" ]; then
  echo "would publish: $PUBLISH_CMD (needs PUBLISH-OK and \$MCPHOST_REGISTRY_PRIVATE_KEY; nothing sent)"
  exit 0
fi

echo "live: $("$BIN" registry-manifest --live 2>&1 | head -1)"
mcp-publisher login dns --domain mcphost.dev --private-key "$MCPHOST_REGISTRY_PRIVATE_KEY" || exit 1
mcp-publisher publish "$ARTIFACT"
