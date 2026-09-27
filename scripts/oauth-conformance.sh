#!/usr/bin/env bash
# oauth-conformance.sh -- PRD-mcphost-oauth-demand-signal P1 requirement 5
# (AC6): runs the MCP OAuth conformance suite against a running host's
# `/mcp` endpoint and writes the report to
# docs/receipts/oauth-conformance-<date>.md.
#
# Usage: scripts/oauth-conformance.sh <mcp-url>
#   e.g. scripts/oauth-conformance.sh http://127.0.0.1:8080/mcp
#
# Exit codes: 0 every scenario passed; the conformance tool's own non-zero
# exit code if any scenario failed (report still written either way, so a
# failure is inspectable); 2 for a usage error or a missing `npx` (a
# one-line reason on stderr, nothing run, no report written).
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$#" -lt 1 ]; then
  echo "oauth-conformance.sh: usage: oauth-conformance.sh <mcp-url>" >&2
  exit 2
fi
mcp_url="$1"

if ! command -v npx >/dev/null 2>&1; then
  echo "oauth-conformance.sh: npx not found on PATH; install Node.js to run the MCP conformance suite" >&2
  exit 2
fi

date_stamp=$(date -u +%Y-%m-%d)
mkdir -p docs/receipts
report="docs/receipts/oauth-conformance-${date_stamp}.md"

set +e
output=$(npx @modelcontextprotocol/conformance server \
  --url "$mcp_url" \
  --scenario auth/basic-metadata-var1 \
  --scenario auth/basic-dcr \
  --requirements 2026-07-28 2>&1)
status=$?
set -e

{
  echo "# OAuth conformance report -- ${date_stamp}"
  echo
  echo "- Target: \`${mcp_url}\`"
  echo "- Requirements: 2026-07-28"
  echo "- Scenarios: auth/basic-metadata-var1, auth/basic-dcr"
  echo "- Exit code: ${status}"
  echo
  echo '```'
  echo "$output"
  echo '```'
} > "$report"

if [ "$status" -ne 0 ]; then
  echo "oauth-conformance.sh: conformance run failed (exit ${status}); see ${report}" >&2
  exit "$status"
fi

echo "oauth-conformance.sh: every scenario passed; wrote ${report}"
